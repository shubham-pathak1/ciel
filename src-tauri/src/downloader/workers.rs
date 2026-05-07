use futures::StreamExt;
use reqwest::Client;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncSeekExt, AsyncWriteExt, BufWriter};
use tokio::sync::mpsc;

use super::types::{SharedRateLimiter, WorkChunk};
use super::{DownloadError, DownloadProgress};

pub(super) struct SpeedState {
    pub(super) last_time: std::time::Instant,
    pub(super) last_bytes: u64,
}

pub(super) enum WorkerOutcome {
    Completed,
    NeedsFallback { reason: &'static str },
}

pub(super) struct WorkerOrchestrationConfig {
    pub(super) id: String,
    pub(super) url: String,
    pub(super) filepath: PathBuf,
    pub(super) client: Client,
    pub(super) db_path: Option<String>,
    pub(super) cancel_signal: Option<Arc<AtomicBool>>,
    pub(super) rate_limiter: Option<Arc<SharedRateLimiter>>,
    pub(super) progress: Arc<Mutex<DownloadProgress>>,
    pub(super) downloaded_atomic: Arc<AtomicU64>,
    pub(super) last_emit: Arc<AtomicU64>,
    pub(super) speed_state: Arc<Mutex<SpeedState>>,
    pub(super) on_progress: Arc<dyn Fn(DownloadProgress) + Send + Sync + 'static>,
    pub(super) pending_chunks: Vec<WorkChunk>,
    pub(super) max_workers: u8,
    pub(super) current_target_workers: u8,
    pub(super) force_multi: bool,
}

pub(super) async fn run_workers(
    cfg: WorkerOrchestrationConfig,
) -> Result<WorkerOutcome, DownloadError> {
    let WorkerOrchestrationConfig {
        id,
        url,
        filepath,
        client,
        db_path,
        cancel_signal,
        rate_limiter,
        progress,
        downloaded_atomic,
        last_emit,
        speed_state,
        on_progress,
        pending_chunks,
        max_workers,
        current_target_workers,
        force_multi,
    } = cfg;

    let error_occurred = Arc::new(Mutex::new(None));
    let abort_workers = Arc::new(AtomicBool::new(false));
    let throttled = Arc::new(Mutex::new(false));
    let failure_count = Arc::new(AtomicUsize::new(0));
    let range_diag_logged = Arc::new(AtomicBool::new(false));
    let multi_start = std::time::Instant::now();
    let pending_chunks = Arc::new(Mutex::new(
        pending_chunks
            .into_iter()
            .filter(|c| c.downloaded < (c.end - c.start + 1))
            .collect::<Vec<_>>(),
    ));
    let active_workers = Arc::new(Mutex::new(0u8));
    let (worker_tx, mut worker_rx) = mpsc::channel::<()>(32);
    let mut last_global_db_update = std::time::Instant::now();
    let start_emit_time = std::time::Instant::now();

    loop {
        let worker_error = { error_occurred.lock().unwrap().clone() };
        if let Some(err) = worker_error {
            if matches!(err, DownloadError::NoRangeSupport) {
                abort_workers.store(true, Ordering::Relaxed);
                tracing::info!(
                    "[{}] Falling back to single connection after range rejection.",
                    id
                );
                return Ok(WorkerOutcome::NeedsFallback {
                    reason: "Server does not support resume. Switching to single connection...",
                });
            }
            return Err(err);
        }

        if force_multi {
            let failures = failure_count.load(Ordering::Relaxed);
            let downloaded = downloaded_atomic.load(Ordering::Relaxed);
            if downloaded == 0
                && failures >= max_workers as usize
                && multi_start.elapsed().as_secs() >= 3
            {
                abort_workers.store(true, Ordering::Relaxed);
                tracing::info!(
                    "[{}] Falling back to single connection after repeated worker failures.",
                    id
                );
                return Ok(WorkerOutcome::NeedsFallback {
                    reason: "Parallel download was unstable. Switching to single connection...",
                });
            }
        }

        let mut current_active = *active_workers.lock().unwrap();
        while current_active < current_target_workers {
            let pending = pending_chunks.clone();
            let chunk = {
                let mut p = pending.lock().unwrap();
                if p.is_empty() {
                    break;
                }
                p.remove(0)
            };

            let active_ptr = active_workers.clone();
            let progress_clone = progress.clone();
            let downloaded_atomic_clone = downloaded_atomic.clone();
            let on_progress_cb = on_progress.clone();
            let db_path_clone = db_path.clone();
            let id_clone = id.clone();
            let client_clone = client.clone();
            let url_clone = url.clone();
            let filepath_clone = filepath.clone();
            let tx = worker_tx.clone();
            let error_ptr = error_occurred.clone();
            let throttled_ptr = throttled.clone();
            let cancel_signal_clone = cancel_signal.clone();
            let abort_signal = abort_workers.clone();
            let failure_counter = failure_count.clone();
            let range_diag_logged_clone = range_diag_logged.clone();
            let last_emit_clone = last_emit.clone();
            let speed_state_clone = speed_state.clone();
            let rate_limiter_clone = rate_limiter.clone();

            *active_workers.lock().unwrap() += 1;
            current_active += 1;

            tokio::spawn(async move {
                let mut chunk = chunk;
                let mut attempts = 0;
                let max_retries = 10;
                let mut final_error = None;

                'worker_mission: loop {
                    if abort_signal.load(Ordering::Relaxed) {
                        break 'worker_mission;
                    }
                    if let Some(sig) = &cancel_signal_clone {
                        if sig.load(Ordering::Relaxed) {
                            break;
                        }
                    }
                    if attempts >= max_retries {
                        tracing::error!(
                            "[{}] Worker reached max retries ({}) for chunk {}-{}",
                            id_clone,
                            max_retries,
                            chunk.start,
                            chunk.end
                        );
                        break;
                    }

                    if attempts > 0 {
                        let backoff = 2u64.pow(attempts as u32 - 1) * 1000;
                        let backoff = backoff.min(30000);
                        tracing::info!(
                            "[{}] Retry #{} for chunk {}-{}. Sleeping {}ms",
                            id_clone,
                            attempts,
                            chunk.start,
                            chunk.end,
                            backoff
                        );

                        let sleep = tokio::time::sleep(std::time::Duration::from_millis(backoff));
                        tokio::pin!(sleep);

                        loop {
                            tokio::select! {
                                _ = &mut sleep => break,
                                _ = tokio::time::sleep(std::time::Duration::from_millis(200)) => {
                                    if abort_signal.load(Ordering::Relaxed) { break 'worker_mission; }
                                    if let Some(sig) = &cancel_signal_clone {
                                        if sig.load(Ordering::Relaxed) { break 'worker_mission; }
                                    }
                                }
                            }
                        }
                    }

                    let res = async {
                        let chunk_file_raw = tokio::fs::OpenOptions::new().write(true).open(&filepath_clone).await?;
                        let mut chunk_file = BufWriter::with_capacity(128 * 1024, chunk_file_raw);
                        let current_start = chunk.start + chunk.downloaded;
                        chunk_file.seek(tokio::io::SeekFrom::Start(current_start)).await?;

                        let range = format!("bytes={}-{}", current_start, chunk.end);
                        let response = client_clone
                            .get(url_clone.clone())
                            .header(reqwest::header::RANGE, range.clone())
                            .header(reqwest::header::ACCEPT_ENCODING, "identity")
                            .send()
                            .await?;

                        if response.status() == 429 || response.status() == 503 {
                            *throttled_ptr.lock().unwrap() = true;
                            return Err(DownloadError::Network("Server throttling".to_string()));
                        }

                        let status = response.status();
                        let headers = response.headers();
                        let has_content_range = headers.contains_key(reqwest::header::CONTENT_RANGE);
                        if !status.is_success() {
                            if matches!(
                                status,
                                reqwest::StatusCode::FORBIDDEN
                                    | reqwest::StatusCode::RANGE_NOT_SATISFIABLE
                                    | reqwest::StatusCode::METHOD_NOT_ALLOWED
                            ) {
                                return Err(DownloadError::NoRangeSupport);
                            }
                            return Err(DownloadError::Network(format!("HTTP {}", status)));
                        }
                        if status != reqwest::StatusCode::PARTIAL_CONTENT && !has_content_range {
                            if !range_diag_logged_clone.swap(true, Ordering::Relaxed) {
                                let content_range = headers
                                    .get(reqwest::header::CONTENT_RANGE)
                                    .and_then(|v| v.to_str().ok())
                                    .unwrap_or("-");
                                let accept_ranges = headers
                                    .get("accept-ranges")
                                    .and_then(|v| v.to_str().ok())
                                    .unwrap_or("-");
                                let content_length = headers
                                    .get(reqwest::header::CONTENT_LENGTH)
                                    .and_then(|v| v.to_str().ok())
                                    .unwrap_or("-");
                                let content_type = headers
                                    .get(reqwest::header::CONTENT_TYPE)
                                    .and_then(|v| v.to_str().ok())
                                    .unwrap_or("-");
                                tracing::error!(
                                    "[{}] Range rejected: status={} range='{}' content-range='{}' accept-ranges='{}' content-length='{}' content-type='{}'",
                                    id_clone,
                                    status,
                                    range,
                                    content_range,
                                    accept_ranges,
                                    content_length,
                                    content_type
                                );
                            }
                            return Err(DownloadError::NoRangeSupport);
                        }

                        {
                            let mut p = progress_clone.lock().unwrap();
                            if p.status_phase.as_deref() != Some("downloading") {
                                p.status_text = Some("Downloading...".to_string());
                                p.status_phase = Some("downloading".to_string());
                                p.phase_elapsed_secs = Some(0);
                            }
                        }

                        let content_type = headers
                            .get(reqwest::header::CONTENT_TYPE)
                            .and_then(|v| v.to_str().ok())
                            .unwrap_or("")
                            .to_string();

                        if content_type.contains("text/html") && url_clone.contains("drive.google.com") {
                            return Err(DownloadError::Network(
                                "Google Drive blocked download (Virus Scan or Login required)"
                                    .to_string(),
                            ));
                        }

                        let mut stream = response.bytes_stream();
                        let mut local_downloaded = chunk.downloaded;
                        let mut last_db_update = std::time::Instant::now();

                        while let Ok(item_opt) =
                            tokio::time::timeout(std::time::Duration::from_secs(60), stream.next())
                                .await
                        {
                            if abort_signal.load(Ordering::Relaxed) {
                                break;
                            }
                            if let Some(sig) = &cancel_signal_clone {
                                if sig.load(Ordering::Relaxed) {
                                    break;
                                }
                            }

                            let item = match item_opt {
                                Some(i) => i,
                                None => break,
                            };

                            let bytes = item.map_err(|e| {
                                tracing::error!(
                                    "[{}] Stream error (ContentType: {}) on chunk {}-{}: {}",
                                    id_clone,
                                    content_type,
                                    chunk.start,
                                    chunk.end,
                                    e
                                );
                                DownloadError::Network(e.to_string())
                            })?;
                            chunk_file.write_all(&bytes).await?;
                            let len = bytes.len() as u64;

                            if let Some(limiter) = &rate_limiter_clone {
                                limiter.acquire(len, &cancel_signal_clone).await;
                            }

                            local_downloaded += len;
                            chunk.downloaded += len;
                            let current_total_downloaded =
                                downloaded_atomic_clone.fetch_add(len, Ordering::Relaxed) + len;

                            let now_ms = start_emit_time.elapsed().as_millis() as u64;
                            let last = last_emit_clone.load(Ordering::Relaxed);
                            if now_ms - last > 200 {
                                if last_emit_clone
                                    .compare_exchange(last, now_ms, Ordering::SeqCst, Ordering::Relaxed)
                                    .is_ok()
                                {
                                    let mut p = progress_clone.lock().unwrap();
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
                                    crate::db::update_chunk_progress(
                                        db,
                                        &id_clone,
                                        chunk.start as i64,
                                        local_downloaded as i64,
                                    )
                                    .ok();
                                }
                                last_db_update = std::time::Instant::now();
                            }
                        }

                        chunk_file.flush().await?;
                        if let Some(ref db) = db_path_clone {
                            crate::db::update_chunk_progress(
                                db,
                                &id_clone,
                                chunk.start as i64,
                                local_downloaded as i64,
                            )
                            .ok();
                        }
                        Ok::<(), DownloadError>(())
                    }
                    .await;

                    match res {
                        Ok(_) => {
                            break;
                        }
                        Err(e) => {
                            if matches!(e, DownloadError::NoRangeSupport) {
                                abort_signal.store(true, Ordering::Relaxed);
                                let mut shared_error = error_ptr.lock().unwrap();
                                if shared_error.is_none() {
                                    tracing::error!(
                                        "[{}] Worker error on chunk {}-{}: {}",
                                        id_clone,
                                        chunk.start,
                                        chunk.end,
                                        e
                                    );
                                    *shared_error = Some(e.clone());
                                }
                                final_error = Some(e);
                                break;
                            }
                            tracing::error!(
                                "[{}] Worker error on chunk {}-{}: {}",
                                id_clone,
                                chunk.start,
                                chunk.end,
                                e
                            );
                            failure_counter.fetch_add(1, Ordering::Relaxed);
                            if let Some(sig) = &cancel_signal_clone {
                                if sig.load(Ordering::Relaxed) {
                                    break;
                                }
                            }
                            final_error = Some(e);
                            attempts += 1;

                            let retry_delay = 1000 * attempts as u64;
                            tracing::info!(
                                "[{}] Error cooldown: retrying after {}ms...",
                                id_clone,
                                retry_delay
                            );

                            let sleep =
                                tokio::time::sleep(std::time::Duration::from_millis(retry_delay));
                            tokio::pin!(sleep);
                            loop {
                                tokio::select! {
                                    _ = &mut sleep => break,
                                    _ = tokio::time::sleep(std::time::Duration::from_millis(200)) => {
                                        if let Some(sig) = &cancel_signal_clone {
                                            if sig.load(Ordering::Relaxed) { break 'worker_mission; }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                if let Some(e) = final_error {
                    let mut shared_error = error_ptr.lock().unwrap();
                    if shared_error.is_none() {
                        *shared_error = Some(e);
                    }
                }

                *active_ptr.lock().unwrap() -= 1;
                let _ = tx.send(()).await;
            });
        }

        if last_global_db_update.elapsed().as_secs() >= 1 {
            let (total_downloaded_p, current_speed) = {
                let p = progress.lock().unwrap();
                (p.downloaded as i64, p.speed as i64)
            };
            if let Some(ref db) = db_path {
                crate::db::update_download_progress(db, &id, total_downloaded_p, current_speed)
                    .ok();
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

    Ok(WorkerOutcome::Completed)
}
