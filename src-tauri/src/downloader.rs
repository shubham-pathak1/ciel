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
use std::sync::atomic::{AtomicU64, Ordering};

/// Download engine errors
#[derive(Error, Debug, Clone, Serialize)]
pub enum DownloadError {
    #[error("Network error: {0}")]
    Network(String),

    #[error("IO error: {0}")]
    Io(String),

    #[error("Server does not support range requests")]
    NoRangeSupport,

    #[error("Download cancelled")]
    Cancelled,

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

/// Download configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadConfig {
    pub id: String, // Added id
    /// URL to download from
    pub url: String,
    /// Target file path
    pub filepath: PathBuf,
    /// Number of connections to use
    pub connections: u8,
    /// Chunk size in bytes (default: 5MB)
    pub chunk_size: u64,
    /// Speed limit in bytes per second (0 = unlimited)
    pub speed_limit: u64,
}

impl Default for DownloadConfig {
    fn default() -> Self {
        Self {
            id: String::new(),
            url: String::new(),
            filepath: PathBuf::new(),
            connections: 4,
            chunk_size: 5 * 1024 * 1024, // 5 MB
            speed_limit: 0,
        }
    }
}

/// Download progress information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadProgress {
    pub id: String, // Added id
    /// Total size in bytes
    pub total: u64,
    /// Downloaded bytes
    pub downloaded: u64,
    /// Current speed in bytes per second
    pub speed: u64,
    /// Estimated time remaining in seconds
    pub eta: u64,
    /// Active connections
    pub connections: u8,
}

/// Chunk record for database persistence
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkRecord {
    pub download_id: String,
    pub start: i64,
    pub end: i64,
    pub downloaded: i64,
}

/// Chunk information for active download
#[derive(Debug, Clone, Copy)]
struct WorkChunk {
    start: u64,
    end: u64,
    downloaded: u64,
    _index: usize, // Index in the original chunk list
}

/// Multi-connection downloader
pub struct Downloader {
    client: Client,
    config: DownloadConfig,
    progress: Arc<std::sync::Mutex<DownloadProgress>>,
    downloaded_atomic: Arc<AtomicU64>,
    db_path: Option<String>,
    cancel_signal: Option<Arc<std::sync::atomic::AtomicBool>>,
    last_emit: Arc<AtomicU64>,
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
        }));

        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .tcp_nodelay(true)
            .build()
            .unwrap_or_default();

        Self {
            client,
            config,
            progress,
            downloaded_atomic: Arc::new(AtomicU64::new(0)),
            db_path: None,
            cancel_signal: None,
            last_emit: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn with_db(mut self, db_path: String) -> Self {
        self.db_path = Some(db_path);
        self
    }

    pub fn get_progress(&self) -> Arc<std::sync::Mutex<DownloadProgress>> {
        self.progress.clone()
    }

    pub fn with_cancel_signal(mut self, signal: Arc<std::sync::atomic::AtomicBool>) -> Self {
        self.cancel_signal = Some(signal);
        self
    }

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

    pub async fn download<F>(&self, on_progress: F) -> Result<(), DownloadError>
    where
        F: Fn(DownloadProgress) + Send + Sync + 'static,
    {
        let url = self.config.url.clone();
        let (supports_range, total_size) = check_range_support(&self.client, &url).await?;

        {
            let mut p = self.progress.lock().unwrap();
            p.total = total_size;
        }

        if !supports_range || total_size == 0 || self.config.connections <= 1 {
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
                let start = i * chunk_size;
                let end = if i == num_chunks - 1 {
                    total_size - 1
                } else {
                    (i + 1) * chunk_size - 1
                };
                chunks.push(WorkChunk {
                    start,
                    end,
                    downloaded: 0,
                    _index: i as usize,
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

        // State for direct access
        let pending_chunks = Arc::new(std::sync::Mutex::new(chunks.into_iter().filter(|c| c.downloaded < (c.end - c.start + 1)).collect::<Vec<_>>()));
        let active_workers = Arc::new(std::sync::Mutex::new(0u8));
        let max_workers = self.config.connections;
        let start_time = std::time::Instant::now();
        let initial_downloaded_at_start = total_downloaded;
        let on_progress_arc = Arc::new(on_progress);
        let error_occurred = Arc::new(std::sync::Mutex::new(None));
        let throttled = Arc::new(std::sync::Mutex::new(false));

        // Start with full power immediately
        let current_target_workers = max_workers;
        
        // Channel to signal worker completion or scaling
        let (worker_tx, mut worker_rx) = mpsc::channel::<()>(32);

            let mut last_global_db_update = std::time::Instant::now();

            loop {
                // Check for errors
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
                    let start_time_clone = start_time;
                    let initial_downloaded_clone = initial_downloaded_at_start;
                    let error_ptr = error_occurred.clone();
                    let throttled_ptr = throttled.clone();

                    *active_workers.lock().unwrap() += 1;
                    current_active += 1;

                    let url = url.clone();
                    let filepath = filepath.clone();
                    let mut chunk = chunk.clone();
                    let client = client.clone();
                    let progress = progress.clone();
                    let active_ptr = active_ptr.clone();
                    let error_ptr = error_ptr.clone();
                    let throttled_ptr = throttled_ptr.clone();
                    let tx = tx.clone();
                    let db_path_clone = db_path_clone.clone();
                    let id_clone = id_clone.clone();
                    let on_progress_cb = on_progress_cb.clone();
                    let downloaded_atomic = downloaded_atomic.clone();
                    let cancel_signal = self.cancel_signal.clone();
                    let last_emit_clone = self.last_emit.clone();
                    
                    tokio::spawn(async move {
                        let mut attempts = 0;
                        let max_retries = 3;
                        let mut final_error = None;
                        
                        loop {
                            // Check cancellation
                            if let Some(sig) = &cancel_signal {
                                if sig.load(std::sync::atomic::Ordering::Relaxed) {
                                    break;
                                }
                            }

                            if attempts >= max_retries {
                                break;
                            }

                            let res = async {
                            let chunk_file_raw = tokio::fs::OpenOptions::new().write(true).open(&filepath).await?;
                            let mut chunk_file = BufWriter::with_capacity(128 * 1024, chunk_file_raw); // 128KB buffer
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

                                let mut stream = response.bytes_stream();
                                let mut local_downloaded = chunk.downloaded;
                                let mut last_db_update = std::time::Instant::now();

                                while let Some(item) = stream.next().await {
                                    // Check cancellation in stream
                                    if let Some(sig) = &cancel_signal {
                                        if sig.load(std::sync::atomic::Ordering::Relaxed) {
                                            return Ok(()); // Exit gracefully
                                        }
                                    }

                                    let bytes = item.map_err(|e| DownloadError::Network(e.to_string()))?;
                                    chunk_file.write_all(&bytes).await?;
                                    let len = bytes.len() as u64;
                                    local_downloaded += len;
                                    chunk.downloaded = local_downloaded;

                                    // Lock-free progress update
                                    let current_total_downloaded = downloaded_atomic.fetch_add(len, Ordering::Relaxed) + len;

                                    // Throttled progress emission
                                    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
                                    let last = last_emit_clone.load(std::sync::atomic::Ordering::Relaxed);
                                    
                                    if now - last > 150 { // Increased to 150ms for performance
                                        if last_emit_clone.compare_exchange(last, now, Ordering::SeqCst, Ordering::Relaxed).is_ok() {
                                            let mut p = progress.lock().unwrap();
                                            p.downloaded = current_total_downloaded;
                                            p.connections = *active_ptr.lock().unwrap();
                                            
                                            let elapsed = start_time_clone.elapsed().as_secs_f64();
                                            if elapsed > 0.1 {
                                                let bytes_since_start = p.downloaded - initial_downloaded_clone;
                                                p.speed = (bytes_since_start as f64 / elapsed) as u64;
                                                if p.speed > 0 {
                                                    p.eta = p.total.saturating_sub(p.downloaded) / p.speed;
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
                                
                                chunk_file.flush().await?; // Ensure everything is written before finishing
                                if let Some(ref db) = db_path_clone {
                                    crate::db::update_chunk_progress(db, &id_clone, chunk.start as i64, local_downloaded as i64).ok();
                                }
                                Ok::<(), DownloadError>(())
                            }.await;

                            match res {
                                Ok(_) => {
                                    final_error = None;
                                    break;
                                }
                                Err(e) => {
                                    // Check cancellation before retry
                                    if let Some(sig) = &cancel_signal {
                                        if sig.load(std::sync::atomic::Ordering::Relaxed) {
                                            break;
                                        }
                                    }
                                    final_error = Some(e);
                                    attempts += 1;
                                    tokio::time::sleep(std::time::Duration::from_millis(1000 * attempts as u64)).await;
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

                // Global DB progress update
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

                // Wait for a worker to finish or a timeout
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

    async fn download_single_connection<F>(&self, on_progress: F) -> Result<(), DownloadError>
    where
        F: Fn(DownloadProgress) + Send + Sync + 'static,
    {
        let response = self.client.get(&self.config.url).send().await?;
        let total_size = response.content_length().unwrap_or(0);

        let file_raw = tokio::fs::File::create(&self.config.filepath).await?;
        let mut file = BufWriter::with_capacity(256 * 1024, file_raw); // Larger buffer for single connection
        let mut stream = response.bytes_stream();
        let start_time = std::time::Instant::now();
        let initial_downloaded = self.downloaded_atomic.load(Ordering::Relaxed);

        let last_emit_clone = self.last_emit.clone();
        let downloaded_atomic = self.downloaded_atomic.clone();
        let progress = self.progress.clone();

        while let Some(item) = stream.next().await {
            let chunk = item.map_err(|e| DownloadError::Network(e.to_string()))?;
            file.write_all(&chunk).await?;

            let len = chunk.len() as u64;
            let current_total = downloaded_atomic.fetch_add(len, Ordering::Relaxed) + len;
            
            // Throttled progress emission
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
            let last = last_emit_clone.load(std::sync::atomic::Ordering::Relaxed);
            
            if now - last > 150 {
                if last_emit_clone.compare_exchange(last, now, Ordering::SeqCst, Ordering::Relaxed).is_ok() {
                    let mut p = progress.lock().unwrap();
                    p.downloaded = current_total;
                    p.total = total_size;
                    p.connections = 1;
                    
                    let elapsed = start_time.elapsed().as_secs_f64();
                    if elapsed > 0.1 {
                        let bytes_since_start = p.downloaded - initial_downloaded;
                        p.speed = (bytes_since_start as f64 / elapsed) as u64;
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

/// Check if server supports range requests
pub async fn check_range_support(client: &Client, url: &str) -> Result<(bool, u64), DownloadError> {
    let response = client.head(url).send().await.map_err(|e| DownloadError::Network(e.to_string()))?;

    let supports_range = response
        .headers()
        .get("accept-ranges")
        .map(|v| v.to_str().unwrap_or("") == "bytes")
        .unwrap_or(false) || response.headers().contains_key("content-range");

    let content_length = response
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    Ok((supports_range, content_length))
}

/// Extract filename from URL or Content-Disposition header
pub fn extract_filename(url: &str, headers: &reqwest::header::HeaderMap) -> String {
    // 1. Try Content-Disposition header first
    if let Some(cd) = headers.get("content-disposition") {
        if let Ok(cd_str) = cd.to_str() {
            // Try filename*= (UTF-8 encoded)
            if let Some(pos) = cd_str.find("filename*=") {
                let parts = &cd_str[pos + 10..];
                let filename = parts.split(';').next().unwrap_or("").trim();
                // Format is usually UTF-8''filename.ext
                if let Some(last_quote) = filename.rfind('\'') {
                    let actual_name = &filename[last_quote + 1..];
                    if let Ok(decoded) = percent_encoding::percent_decode(actual_name.as_bytes()).decode_utf8() {
                        return sanitize_filename(&decoded);
                    }
                }
            }
            
            // Try standard filename=
            if let Some(pos) = cd_str.find("filename=") {
                let parts = &cd_str[pos + 9..];
                let filename = parts.split(';').next().unwrap_or("").trim();
                let filename = filename.trim_matches('"').trim_matches('\'');
                if !filename.is_empty() {
                    return sanitize_filename(filename);
                }
            }
        }
    }

    // 2. Fall back to URL path
    let filename = url.rsplit('/')
        .next()
        .and_then(|s| s.split('?').next())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .map(|s| {
            percent_encoding::percent_decode(s.as_bytes())
                .decode_utf8()
                .map(|decoded| decoded.into_owned())
                .unwrap_or(s)
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
