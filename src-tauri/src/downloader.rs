use futures::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use sha2::{Sha256, Digest};
use std::fs::File;
use std::io::Read;
use thiserror::Error;
use tokio::sync::mpsc;
use tokio::io::{AsyncWriteExt, AsyncSeekExt, BufWriter};
use std::sync::atomic::{AtomicU64, Ordering, AtomicBool};

/// A shared token-bucket rate limiter to coordinate multiple download workers.
/// 
/// This is based on the logic used by high-performance download managers like IDM/FDM.
/// Instead of each worker throttling itself, they consume from a central pool.
/// This ensures the aggregate speed stays exactly at the limit without causing 
/// bursty traffic that could trigger server-side TCP resets or "Slow Consumer" errors.
pub struct SharedRateLimiter {
    limit: u64, // bytes per second
    tokens: AtomicU64,
    last_update: std::sync::Mutex<std::time::Instant>,
}

impl SharedRateLimiter {
    pub fn new(limit: u64) -> Self {
        Self {
            limit,
            tokens: AtomicU64::new(limit), // Start with a full bucket (1s burst)
            last_update: std::sync::Mutex::new(std::time::Instant::now()),
        }
    }

    /// Consumes tokens from the bucket. If not enough tokens are available, it sleeps.
    /// 
    /// To keep the stream "hot" and responsive, we process acquisitions in small increments.
    /// This also prevents deadlocks where a single received chunk is larger than the 1s burst cap.
    pub async fn acquire(&self, amount: u64, cancel_signal: &Option<Arc<AtomicBool>>) {
        if self.limit == 0 { return; }

        let mut remaining = amount;
        while remaining > 0 {
            if let Some(sig) = cancel_signal {
                if sig.load(Ordering::Relaxed) { return; }
            }

            // 1. Refill tokens based on elapsed time
            {
                let mut last_update = self.last_update.lock().unwrap();
                let now = std::time::Instant::now();
                let elapsed = now.duration_since(*last_update).as_secs_f64();
                
                // Refill every 10ms for even higher frequency pacing
                if elapsed >= 0.01 {
                    let refill = (self.limit as f64 * elapsed) as u64;
                    if refill > 0 {
                        let current = self.tokens.load(Ordering::Relaxed);
                        // Cap at 1s worth of tokens to prevent huge bursts after pauses
                        let new_tokens = (current + refill).min(self.limit);
                        self.tokens.store(new_tokens, Ordering::Relaxed);
                        *last_update = now;
                    }
                }
            }

            // 2. Try to consume what we can
            let current = self.tokens.load(Ordering::Relaxed);
            if current > 0 {
                let take = remaining.min(current);
                if self.tokens.compare_exchange(current, current - take, Ordering::SeqCst, Ordering::Relaxed).is_ok() {
                    remaining -= take;
                    if remaining == 0 { break; }
                }
            }

            // 3. Not enough tokens, wait a tiny bit
            if remaining > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }
    }
}
#[derive(Error, Debug, Clone, Serialize)]
pub enum DownloadError {
    /// Failure during network request or response streaming.
    #[error("Network error: {0}")]
    Network(String),

    /// Failure while writing data to the local disk.
    #[error("IO error: {0}")]
    Io(String),

    /// The remote server does not support the HTTP `Range` header, 
    /// making multi-threaded or resumed downloads impossible.
    #[error("Server does not support range requests")]
    NoRangeSupport,

    /// The transfer was stopped by the user or the system.
    #[error("Download cancelled")]
    Cancelled,

    /// The provided string could not be parsed as a valid URL.
    #[error("Invalid URL: {0}")]
    InvalidUrl(String),
}

impl From<reqwest::Error> for DownloadError {
    fn from(err: reqwest::Error) -> Self {
        DownloadError::Network(err.to_string())
    }
}

impl From<std::io::Error> for DownloadError {
    fn from(err: std::io::Error) -> Self {
        DownloadError::Io(err.to_string())
    }
}

/// Immutable configuration for an HTTP download session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadConfig {
    /// Internal Ciel UUID.
    pub id: String, 
    /// Source web URL.
    pub url: String,
    /// Absolute target path on disk.
    pub filepath: PathBuf,
    /// Maximum number of concurrent TCP connections.
    pub connections: u8,
    /// Size of each work unit in bytes.
    pub chunk_size: u64,
    /// Throttling limit (bytes/sec).
    pub speed_limit: u64,
    /// Custom User-Agent string.
    pub user_agent: Option<String>,
    /// Optional cookies for authenticated sessions.
    pub cookies: Option<String>,
}

impl Default for DownloadConfig {
    fn default() -> Self {
        Self {
            id: "default".to_string(),
            url: "".to_string(),
            filepath: PathBuf::new(),
            connections: 8,
            chunk_size: 5 * 1024 * 1024, // 5 MB
            speed_limit: 0,
            user_agent: None,
            cookies: None,
        }
    }
}

/// Snapshot of the current transfer state, emitted to the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadProgress {
    pub id: String,
    pub total: u64,
    pub downloaded: u64,
    /// Bytes per second (instantanous).
    pub speed: u64,
    /// Calculated seconds remaining based on current speed.
    pub eta: u64,
    pub connections: u8,
    pub speed_limit: u64,
    /// Detailed status message (e.g., "Initializing...", "Connecting...", "Fetching Metadata").
    pub status_text: Option<String>,
    /// Discovered filename (emitted if it differs from the initial generic one).
    pub filename: Option<String>,
}

/// Persistence model for a single byte-range segment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkRecord {
    pub download_id: String,
    pub start: i64,
    pub end: i64,
    pub downloaded: i64,
}

/// A discrete unit of work for a single worker thread.
#[derive(Debug, Clone, Copy)]
struct WorkChunk {
    /// Byte offset where the chunk starts.
    start: u64,
    /// Byte offset where the chunk ends.
    end: u64,
    /// Number of bytes already successfully transferred in this chunk.
    downloaded: u64,
    _index: usize,
}

/// A sophisticated, multi-threaded HTTP download engine.
/// 
/// It implements:
/// - **Parallel TCP Connections**: Spawns multiple workers to saturate bandwidth.
/// - **Resumable Transfers**: Uses SQLite to track chunk progress.
/// - **Speed Throttling**: A custom token-bucket-like algorithm for bandwidth management.
/// - **Cancellable Tasks**: Integrated with `tokio` cancellation signals.
pub struct Downloader {
    client: Client,
    config: DownloadConfig,
    progress: Arc<std::sync::Mutex<DownloadProgress>>,
    downloaded_atomic: Arc<AtomicU64>,
    db_path: Option<String>,
    cancel_signal: Option<Arc<std::sync::atomic::AtomicBool>>,
    last_emit: Arc<AtomicU64>,
    rate_limiter: Option<Arc<SharedRateLimiter>>,
}

impl Downloader {
    pub fn new(config: DownloadConfig) -> Self {
        let progress = Arc::new(std::sync::Mutex::new(DownloadProgress {
            id: config.id.clone(),
            total: 0,
            downloaded: 0,
            speed: 0,
            eta: 0,
            connections: config.connections,
            speed_limit: config.speed_limit,
            status_text: None,
            filename: None,
        }));

        let mut builder = Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .pool_max_idle_per_host(32)
            .pool_idle_timeout(std::time::Duration::from_secs(90))
            .tcp_keepalive(Some(std::time::Duration::from_secs(60)))
            .tcp_nodelay(true);

        if let Some(ref ua) = config.user_agent {
            builder = builder.user_agent(ua);
        } else {
            builder = builder.user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36");
        }

        if let Some(ref cookies) = config.cookies {
            use reqwest::header::{HeaderMap, HeaderValue, COOKIE};
            let mut headers = HeaderMap::new();
            if let Ok(v) = HeaderValue::from_str(cookies) {
                headers.insert(COOKIE, v);
                builder = builder.default_headers(headers);
            }
        }

        let client = builder.build().unwrap_or_default();

        let speed_limit = config.speed_limit;

        Self {
            client,
            config,
            progress,
            downloaded_atomic: Arc::new(AtomicU64::new(0)),
            db_path: None,
            cancel_signal: None,
            last_emit: Arc::new(AtomicU64::new(0)),
            rate_limiter: if speed_limit > 0 {
                Some(Arc::new(SharedRateLimiter::new(speed_limit)))
            } else {
                None
            },
        }
    }

    /// Builder: Attaches a database path to the downloader for chunk persistence/resume support.
    pub fn with_db(mut self, db_path: String) -> Self {
        self.db_path = Some(db_path);
        self
    }

    pub fn get_progress(&self) -> Arc<std::sync::Mutex<DownloadProgress>> {
        self.progress.clone()
    }

    /// Builder: Attaches an external cancellation signal.
    pub fn with_cancel_signal(mut self, signal: Arc<std::sync::atomic::AtomicBool>) -> Self {
        self.cancel_signal = Some(signal);
        self
    }

    /// Computes the SHA-256 hash of the downloaded file and compares it with the expected value.
    pub async fn verify_checksum(&self, expected_hash: &str) -> Result<bool, DownloadError> {
        let filepath = &self.config.filepath;
        let mut file = File::open(filepath)?;
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 8192];

        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 { break; }
            hasher.update(&buffer[..count]);
        }

        let result = hasher.finalize();
        let hex_result = format!("{:x}", result);
        Ok(hex_result == expected_hash.to_lowercase())
    }

    /// The primary entry point for starting a download.
    /// 
    /// This method manages the entire transfer lifecycle:
    /// 1. Metadata discovery (Range support, Total size).
    /// 2. Chunk calculation and database synchronization.
    /// 3. Worker orchestration (spawning parallel tasks).
    /// 4. Real-time progress reporting.
    pub async fn download<F>(&self, on_progress: F) -> Result<(), DownloadError>
    where
        F: Fn(DownloadProgress) + Send + Sync + 'static,
    {
        let url = self.config.url.clone();
        
        // 1. Immediate architectural feedback: Starting initialization phase.
        on_progress({
            let mut p = self.progress.lock().unwrap().clone();
            p.status_text = Some("Initializing...".to_string());
            p
        });
        

        // Optimization 1: If user only requested 1 connection, skip the HEAD check and go straight to GET.
        // This avoids one round-trip and significantly speeds up "rust-style" performance.
        if self.config.connections <= 1 {
            return self.download_single_connection(on_progress).await;
        }

        // 2. Discover metadata and verify segmented download support via HEAD request.
        let (supports_range, total_size, filename_opt) = check_range_support(&self.client, &url).await?;

        // 3. Background name resolution: update if discovered from headers.
        if let Some(new_name) = &filename_opt {
            if let Some(ref db_path) = self.db_path {
                let _ = crate::db::update_download_name(db_path, &self.config.id, new_name);
            }
        }

        {
            let mut p = self.progress.lock().unwrap();
            p.total = total_size;
            p.filename = filename_opt;
            p.status_text = Some("Downloading...".to_string());
        }

        if !supports_range || total_size == 0 {
            return self.download_single_connection(on_progress).await;
        }

        // Prepare File (don't truncate if it exists for resume)
        let file_exists = self.config.filepath.exists();
        if !file_exists {
            let f = std::fs::File::create(&self.config.filepath)?;
            f.set_len(total_size)?;
        }

        // Get chunks from DB if possible
        let mut chunks = Vec::new();
        if let Some(ref db_path) = self.db_path {
            if let Ok(db_chunks) = crate::db::get_download_chunks(db_path, &self.config.id) {
                if !db_chunks.is_empty() {
                    chunks = db_chunks.into_iter().enumerate().map(|(i, c)| WorkChunk {
                        start: c.start as u64,
                        end: c.end as u64,
                        downloaded: c.downloaded as u64,
                        _index: i,
                    }).collect();
                }
            }
        }

        // If no chunks, calculate them
        if chunks.is_empty() {
            let connections = self.config.connections as u64;
            // Use even more chunks than workers for better distribution (8x)
            let num_chunks = connections * 8; 
            let chunk_size = total_size / num_chunks;
            let mut db_chunks_to_insert = Vec::new();

            for i in 0..num_chunks {
                let mut start = i * chunk_size;
                let end = if i == num_chunks - 1 {
                    total_size - 1
                } else {
                    (i + 1) * chunk_size - 1
                };

                // Cap individual chunks at 10MB to prevent single long requests when throttled.
                // This ensures that even on slow connections, we keep cycling through requests and updating DB.
                let max_chunk = 10 * 1024 * 1024;
                while (end - start + 1) > max_chunk {
                    let sub_end = start + max_chunk - 1;
                    chunks.push(WorkChunk {
                        start,
                        end: sub_end,
                        downloaded: 0,
                        _index: chunks.len(),
                    });
                    db_chunks_to_insert.push(ChunkRecord {
                        download_id: self.config.id.clone(),
                        start: start as i64,
                        end: sub_end as i64,
                        downloaded: 0,
                    });
                    start += max_chunk;
                }

                chunks.push(WorkChunk {
                    start,
                    end,
                    downloaded: 0,
                    _index: chunks.len(),
                });
                db_chunks_to_insert.push(ChunkRecord {
                    download_id: self.config.id.clone(),
                    start: start as i64,
                    end: end as i64,
                    downloaded: 0,
                });
            }

            if let Some(ref db_path) = self.db_path {
                crate::db::insert_chunks(db_path, db_chunks_to_insert).ok();
            }
        }

        let total_downloaded = chunks.iter().map(|c| c.downloaded).sum();
        self.downloaded_atomic.store(total_downloaded, Ordering::SeqCst);
        {
            let mut p = self.progress.lock().unwrap();
            p.downloaded = total_downloaded;
        }

        
        struct SpeedState {
            last_time: std::time::Instant,
            last_bytes: u64,
        }
        let speed_state = Arc::new(std::sync::Mutex::new(SpeedState {
            last_time: std::time::Instant::now(),
            last_bytes: total_downloaded,
        }));
        
        let on_progress_arc = Arc::new(on_progress);
        let error_occurred = Arc::new(std::sync::Mutex::new(None));
        let throttled = Arc::new(std::sync::Mutex::new(false));

        // State for direct access
        let pending_chunks = Arc::new(std::sync::Mutex::new(chunks.into_iter().filter(|c| c.downloaded < (c.end - c.start + 1)).collect::<Vec<_>>()));
        let active_workers = Arc::new(std::sync::Mutex::new(0u8));
        let max_workers = self.config.connections;
        // Start with full power immediately, but cap workers if speed limit is too low
        // Rule: Each connection should ideally have ~256 KB/s to prevent "Slow Consumer" resets
        let mut current_target_workers = max_workers;
        if self.config.speed_limit > 0 {
            let min_speed_per_worker = 512 * 1024; // 512 KB/s
            let calculated_max = (self.config.speed_limit / min_speed_per_worker) as u8;
            current_target_workers = current_target_workers.min(calculated_max.max(1));
            
            if current_target_workers < max_workers {
                println!("[{}] Speed limit is low ({} bytes/s). Scaling down to {} workers for stability.", 
                    self.config.id, self.config.speed_limit, current_target_workers);
            }
        }
        
        // Channel to signal worker completion or scaling
        let (worker_tx, mut worker_rx) = mpsc::channel::<()>(32);
        let mut last_global_db_update = std::time::Instant::now();
        let start_emit_time = std::time::Instant::now();

        loop {
            // Check for errors from workers
            if let Some(err) = error_occurred.lock().unwrap().clone() {
                return Err(err);
            }

            // Spawn workers up to current target
            let mut current_active = *active_workers.lock().unwrap();
            while current_active < current_target_workers {
                let pending = pending_chunks.clone();
                let chunk = {
                    let mut p = pending.lock().unwrap();
                    if p.is_empty() { break; }
                    p.remove(0)
                };

                let active_ptr = active_workers.clone();
                let progress = self.progress.clone();
                let downloaded_atomic = self.downloaded_atomic.clone();
                let on_progress_cb = on_progress_arc.clone();
                let db_path_clone = self.db_path.clone();
                let id_clone = self.config.id.clone();
                let client = self.client.clone();
                let url = url.clone();
                let filepath = self.config.filepath.clone();
                let tx = worker_tx.clone();
                let error_ptr = error_occurred.clone();
                let throttled_ptr = throttled.clone();
                let cancel_signal = self.cancel_signal.clone();
                let last_emit_clone = self.last_emit.clone();
                let speed_state_clone = speed_state.clone();
                let rate_limiter = self.rate_limiter.clone();

                *active_workers.lock().unwrap() += 1;
                current_active += 1;

                tokio::spawn(async move {
                    let rate_limiter = rate_limiter;
                    let last_emit_clone = last_emit_clone;
                    let speed_state_clone = speed_state_clone;
                    let mut chunk = chunk;
                    let mut attempts = 0;
                    let max_retries = 10;
                    let mut final_error = None;
                    
                    'worker_mission: loop {
                        if let Some(sig) = &cancel_signal {
                            if sig.load(Ordering::Relaxed) { 
                                break; 
                            }
                        }
                        if attempts >= max_retries { 
                            eprintln!("[{}] Worker reached max retries ({}) for chunk {}-{}", id_clone, max_retries, chunk.start, chunk.end);
                            break; 
                        }

                        if attempts > 0 {
                            let backoff = 2u64.pow(attempts as u32 - 1) * 1000;
                            let backoff = backoff.min(30000); // capped at 30s
                            println!("[{}] Retry #{} for chunk {}-{}. Sleeping {}ms", id_clone, attempts, chunk.start, chunk.end, backoff);
                            
                            // Responsive sleep: check for cancellation signal during backoff
                            let sleep = tokio::time::sleep(std::time::Duration::from_millis(backoff));
                            tokio::pin!(sleep);
                            
                            loop {
                                tokio::select! {
                                    _ = &mut sleep => break,
                                    _ = tokio::time::sleep(std::time::Duration::from_millis(200)) => {
                                        if let Some(sig) = &cancel_signal {
                                            if sig.load(Ordering::Relaxed) { break 'worker_mission; }
                                        }
                                    }
                                }
                            }
                        }

                        let res = async {
                            let chunk_file_raw = tokio::fs::OpenOptions::new().write(true).open(&filepath).await?;
                            let mut chunk_file = BufWriter::with_capacity(128 * 1024, chunk_file_raw);
                            let current_start = chunk.start + chunk.downloaded;
                            chunk_file.seek(tokio::io::SeekFrom::Start(current_start)).await?;

                            let range = format!("bytes={}-{}", current_start, chunk.end);
                            let response = client.get(url.clone()).header("Range", range).send().await?;

                            if response.status() == 429 || response.status() == 503 {
                                *throttled_ptr.lock().unwrap() = true;
                                return Err(DownloadError::Network("Server throttling".to_string()));
                            }

                            if !response.status().is_success() {
                                return Err(DownloadError::Network(format!("HTTP {}", response.status())));
                            }

                            // Capture content-type for detailed error reporting if decoding fails
                            let content_type = response.headers()
                                .get(reqwest::header::CONTENT_TYPE)
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("")
                                .to_string();

                            if content_type.contains("text/html") && url.contains("drive.google.com") {
                                return Err(DownloadError::Network("Google Drive blocked download (Virus Scan or Login required)".to_string()));
                            }

                            let mut stream = response.bytes_stream();
                            let mut local_downloaded = chunk.downloaded;
                            let mut last_db_update = std::time::Instant::now();

                            while let Ok(item_opt) = tokio::time::timeout(std::time::Duration::from_secs(60), stream.next()).await {
                                if let Some(sig) = &cancel_signal {
                                    if sig.load(Ordering::Relaxed) { break; }
                                }

                                let item = match item_opt {
                                    Some(i) => i,
                                    None => break, // Stream finished
                                };

                                let bytes = item.map_err(|e| {
                                    eprintln!("[{}] Stream error (ContentType: {}) on chunk {}-{}: {}", id_clone, content_type, chunk.start, chunk.end, e);
                                    DownloadError::Network(e.to_string())
                                })?;
                                chunk_file.write_all(&bytes).await?;
                                let len = bytes.len() as u64;

                                 if let Some(limiter) = &rate_limiter {
                                     limiter.acquire(len, &cancel_signal).await;
                                 }

                                local_downloaded += len;
                                chunk.downloaded += len;
                                let current_total_downloaded = downloaded_atomic.fetch_add(len, Ordering::Relaxed) + len;

                                // Throttled progress emission
                                let now_ms = start_emit_time.elapsed().as_millis() as u64;
                                let last = last_emit_clone.load(Ordering::Relaxed);
                                if now_ms - last > 200 {
                                    if last_emit_clone.compare_exchange(last, now_ms, Ordering::SeqCst, Ordering::Relaxed).is_ok() {
                                        let mut p = progress.lock().unwrap();
                                        p.downloaded = current_total_downloaded;
                                        p.connections = *active_ptr.lock().unwrap();
                                        
                                        {
                                            let mut ss = speed_state_clone.lock().unwrap();
                                            let interval_elapsed = ss.last_time.elapsed().as_secs_f64();
                                            if interval_elapsed >= 0.5 {
                                                let diff = current_total_downloaded.saturating_sub(ss.last_bytes);
                                                p.speed = (diff as f64 / interval_elapsed) as u64;
                                                ss.last_bytes = current_total_downloaded;
                                                ss.last_time = std::time::Instant::now();
                                                if p.speed > 0 {
                                                    p.eta = p.total.saturating_sub(p.downloaded) / p.speed;
                                                }
                                            }
                                        }
                                        (on_progress_cb)(p.clone());
                                    }
                                }

                                if last_db_update.elapsed().as_secs() >= 5 {
                                    if let Some(ref db) = db_path_clone {
                                        crate::db::update_chunk_progress(db, &id_clone, chunk.start as i64, local_downloaded as i64).ok();
                                    }
                                    last_db_update = std::time::Instant::now();
                                }
                            }
                            
                            chunk_file.flush().await?;
                            if let Some(ref db) = db_path_clone {
                                crate::db::update_chunk_progress(db, &id_clone, chunk.start as i64, local_downloaded as i64).ok();
                            }
                            Ok::<(), DownloadError>(())
                        }.await;

                        match res {
                            Ok(_) => { break; }
                            Err(e) => {
                                if let Some(sig) = &cancel_signal {
                                    if sig.load(Ordering::Relaxed) { break; }
                                }
                                final_error = Some(e);
                                attempts += 1;
                                
                                let retry_delay = 1000 * attempts as u64;
                                println!("[{}] Error cooldown: retrying after {}ms...", id_clone, retry_delay);
                                
                                // Responsive sleep for the outer retry loop
                                let sleep = tokio::time::sleep(std::time::Duration::from_millis(retry_delay));
                                tokio::pin!(sleep);
                                loop {
                                    tokio::select! {
                                        _ = &mut sleep => break,
                                        _ = tokio::time::sleep(std::time::Duration::from_millis(200)) => {
                                            if let Some(sig) = &cancel_signal {
                                                if sig.load(Ordering::Relaxed) { break 'worker_mission; }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    if let Some(e) = final_error {
                        *error_ptr.lock().unwrap() = Some(e);
                    }

                    *active_ptr.lock().unwrap() -= 1;
                    let _ = tx.send(()).await;
                });
            }

            if last_global_db_update.elapsed().as_secs() >= 1 {
                let (total_downloaded_p, current_speed) = {
                    let p = self.progress.lock().unwrap();
                    (p.downloaded as i64, p.speed as i64)
                };
                if let Some(ref db) = self.db_path {
                    crate::db::update_download_progress(db, &self.config.id, total_downloaded_p, current_speed).ok();
                }
                last_global_db_update = std::time::Instant::now();
            }

            if current_active == 0 && pending_chunks.lock().unwrap().is_empty() {
                break;
            }

            tokio::select! {
                _ = worker_rx.recv() => {},
                _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {},
            }
        }

        Ok(())
    }

    /// Fallback: Downloads a file using a single TCP connection.
    /// 
    /// Used when the server lacks `Range` support or for very small files where 
    /// multi-threading overhead is counter-productive.
    async fn download_single_connection<F>(&self, on_progress: F) -> Result<(), DownloadError>
    where
        F: Fn(DownloadProgress) + Send + Sync + 'static,
    {
        {
            let mut p = self.progress.lock().unwrap();
            p.status_text = Some("Downloading...".to_string());
        }
        (on_progress)(self.progress.lock().unwrap().clone());

        let response = self.client.get(&self.config.url).send().await?;
        
        // Safety check: If we're getting HTML but expecting a file, it's a login/warning page
        let content_type = response.headers().get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if content_type.contains("text/html") {
            return Err(DownloadError::Network("Server returned a webpage instead of a file. Login may be required.".to_string()));
        }

        let total_size = response.content_length().unwrap_or(0);

        let file_raw = tokio::fs::File::create(&self.config.filepath).await?;
        let mut file = BufWriter::with_capacity(256 * 1024, file_raw); // Larger buffer for single connection
        let mut stream = response.bytes_stream();
        let mut last_speed_time = std::time::Instant::now();
        let start_emit_time = std::time::Instant::now();
        let mut last_speed_bytes = self.downloaded_atomic.load(Ordering::Relaxed);

        let last_emit_clone = self.last_emit.clone();
        let downloaded_atomic = self.downloaded_atomic.clone();
        let progress = self.progress.clone();
        let global_speed_limit = self.config.speed_limit;

        while let Some(item) = stream.next().await {
            let chunk = item.map_err(|e| DownloadError::Network(e.to_string()))?;
            file.write_all(&chunk).await?;

            let len = chunk.len() as u64;

            // BANDWIDTH THROTTLING
            if let Some(limiter) = &self.rate_limiter {
                limiter.acquire(len, &self.cancel_signal).await;
            } else if global_speed_limit > 0 {
                // Fallback for when limiter isn't initialized but limit is set
                let cost_ms = (len * 1000) / global_speed_limit;
                if cost_ms > 0 {
                    tokio::time::sleep(std::time::Duration::from_millis(cost_ms)).await;
                }
            }

            let current_total = downloaded_atomic.fetch_add(len, Ordering::Relaxed) + len;
            
            // Throttled progress emission
            let now_ms = start_emit_time.elapsed().as_millis() as u64;
            let last = last_emit_clone.load(std::sync::atomic::Ordering::Relaxed);
            
            if now_ms - last > 200 {
                if last_emit_clone.compare_exchange(last, now_ms, Ordering::SeqCst, Ordering::Relaxed).is_ok() {
                    let mut p = progress.lock().unwrap();
                    p.downloaded = current_total;
                    p.total = total_size;
                    p.connections = 1;
                    
                    let interval_elapsed = last_speed_time.elapsed().as_secs_f64();
                    if interval_elapsed >= 0.3 {
                        let diff = current_total.saturating_sub(last_speed_bytes);
                        p.speed = (diff as f64 / interval_elapsed) as u64;
                        
                        last_speed_bytes = current_total;
                        last_speed_time = std::time::Instant::now();
                        
                        if p.speed > 0 {
                            p.eta = p.total.saturating_sub(p.downloaded) / p.speed;
                        }
                    }
                    (on_progress)(p.clone());
                }
            }
        }

        file.flush().await?;
        Ok(())
    }
}

/// Queries a URL using a `HEAD` request to verify if it supports segmented downloads. 
/// Also extracts the content length and suggested filename.
pub async fn check_range_support(client: &Client, url: &str) -> Result<(bool, u64, Option<String>), DownloadError> {
    let response = client.head(url)
        .timeout(std::time::Duration::from_secs(5))
        .send().await.map_err(|e| DownloadError::Network(e.to_string()))?;

    let filename = extract_filename(url, response.headers());
    let filename_opt = if filename != "download" && filename != "download_file" && filename != "uc" {
        Some(filename)
    } else {
        None
    };

    let supports_range = response
        .headers()
        .get("accept-ranges")
        .map(|v| v.to_str().unwrap_or("") == "bytes")
        .unwrap_or(false) || response.headers().contains_key("content-range");

    let total_size = response
        .headers()
        .get(reqwest::header::CONTENT_LENGTH)
        .and_then(|val| val.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);

    Ok((supports_range, total_size, filename_opt))
}

/// Heuristic: Extracts a probable filename from the URL or the `Content-Disposition` header.
pub fn extract_filename(url: &str, headers: &reqwest::header::HeaderMap) -> String {
    // 1. Try Content-Disposition header first
    if let Some(cd) = headers.get("content-disposition") {
        if let Ok(cd_str) = cd.to_str() {
            // Try filename*= (UTF-8 encoded according to RFC 6266)
            if let Some(pos) = cd_str.find("filename*=") {
                let parts = &cd_str[pos + 10..];
                let filename_part = parts.split(';').next().unwrap_or("").trim();
                // Format is usually charset'lang'filename (e.g. UTF-8''hello.txt)
                if let Some(last_quote) = filename_part.rfind('\'') {
                    let actual_name = &filename_part[last_quote + 1..];
                    if let Ok(decoded) = percent_encoding::percent_decode(actual_name.as_bytes()).decode_utf8() {
                        return sanitize_filename(&decoded);
                    }
                }
            }
            
            // Try standard filename= (often quoted, sometimes improperly percent-encoded by servers)
            if let Some(pos) = cd_str.find("filename=") {
                let parts = &cd_str[pos + 9..];
                let raw_name = parts.split(';').next().unwrap_or("").trim();
                let raw_name = raw_name.trim_matches('"').trim_matches('\'');
                if !raw_name.is_empty() {
                    // Even for standard filename=, some servers send percent-encoded strings.
                    // We attempt to decode it; if it's not encoded, it returns the original.
                    if let Ok(decoded) = percent_encoding::percent_decode(raw_name.as_bytes()).decode_utf8() {
                        return sanitize_filename(&decoded);
                    }
                    return sanitize_filename(raw_name);
                }
            }
        }
    }

    // 2. Fall back to URL path
    // We want the last non-empty segment before any query parameters or hash fragments
    let filename = url.split('?').next().unwrap_or(url)
        .split('#').next().unwrap_or(url)
        .rsplit('/')
        .find(|s| !s.is_empty())
        .map(|s| {
            percent_encoding::percent_decode(s.as_bytes())
                .decode_utf8()
                .map(|decoded| decoded.into_owned())
                .unwrap_or_else(|_| s.to_string())
        })
        .unwrap_or_else(|| "download".to_string());
        
    sanitize_filename(&filename)
}

fn sanitize_filename(name: &str) -> String {
    let sanitized = name.replace(|c: char| c.is_control() || "<>:\"/\\|?*".contains(c), "_");
    if sanitized.is_empty() {
        "download".to_string()
    } else {
        sanitized
    }
}
