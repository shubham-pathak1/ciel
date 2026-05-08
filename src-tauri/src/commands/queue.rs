use crate::commands::http::{self, DownloadManager};
use crate::commands::set_and_emit_download_error;
use crate::db::{self, DbState, DownloadProtocol, DownloadStatus};
use crate::torrent::TorrentManager;
use std::path::Path;
use tauri::{AppHandle, Emitter, Manager, Runtime, State};

use super::torrent::parse_optional_torrent_indices_metadata;

/// QUEUE PROCESSOR
///
/// Checks if the number of active downloads is below the limit, and if so,
/// starts the next queued download from the database.
pub async fn process_queue<R: Runtime>(app: AppHandle<R>) {
    let db_state: State<DbState> = app.state();
    let manager: State<DownloadManager> = app.state();
    let torrent_manager: State<TorrentManager> = app.state();

    // Loop until we max out slots or run out of queued items
    loop {
        // 1. Check Limits
        let max_simultaneous = db::get_setting(&db_state.path, "max_concurrent")
            .ok()
            .flatten()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(3);

        let (http_active, _) = manager.get_global_status().await;
        let (torrent_active, _) = torrent_manager.get_global_status().await;

        if (http_active + torrent_active) >= max_simultaneous {
            break;
        }

        // 2. Get Next Queued
        let next_download = match db::get_next_queued_download(&db_state.path) {
            Ok(Some(d)) => d,
            Ok(None) => break, // No more queued items
            Err(e) => {
                tracing::error!("Failed to fetch queued download: {}", e);
                break;
            }
        };

        // 3. Start Download
        let id = next_download.id.clone();
        tracing::info!("Queue Processor: Starting {}", next_download.filename);

        // Update status first to prevent race conditions (double starting)
        if let Err(e) = db::update_download_status(&db_state.path, &id, DownloadStatus::Downloading)
        {
            tracing::error!("Failed to update status for {}: {}", id, e);
            continue;
        }

        db::log_event(
            &db_state.path,
            &id,
            "started",
            Some("Auto-started from queue"),
        )
        .ok();
        let _ = app.emit("download-started", id.clone());

        match next_download.protocol {
            DownloadProtocol::Http => {
                if let Err(e) = http::start_download_task(
                    app.clone(),
                    db_state.path.clone(),
                    manager.inner().clone(),
                    next_download,
                )
                .await
                {
                    tracing::error!("Failed to start queued HTTP download {}: {}", id, e);
                    set_and_emit_download_error(&app, &db_state.path, &id, &e);
                }
            }
            DownloadProtocol::Torrent => {
                let path = Path::new(&next_download.filepath);
                let base_folder = path
                    .parent()
                    .unwrap_or(Path::new("."))
                    .to_string_lossy()
                    .to_string();

                let indices = match parse_optional_torrent_indices_metadata(&next_download.metadata)
                {
                    Ok(v) => v,
                    Err(msg) => {
                        set_and_emit_download_error(&app, &db_state.path, &id, &msg);
                        continue;
                    }
                };

                if !torrent_manager.wait_until_ready(30000).await {
                    tracing::error!(
                        "Queue Processor: torrent engine still initializing; will retry {}",
                        id
                    );
                    let _ = db::update_download_status(&db_state.path, &id, DownloadStatus::Queued);
                    break;
                }

                if let Err(e) = torrent_manager
                    .add_magnet(
                        app.clone(),
                        id.clone(),
                        next_download.url.clone(),
                        base_folder,
                        db_state.path.clone(),
                        indices,
                        next_download.size as u64,
                        next_download.downloaded.max(0) as u64,
                        true,  // is_resume
                        false, // start_paused
                        None,
                    )
                    .await
                {
                    tracing::error!("Failed to start queued torrent {}: {}", id, e);
                    set_and_emit_download_error(&app, &db_state.path, &id, &e);
                }
            }
            DownloadProtocol::Video => {
                // TODO: Implement video download queuing when video support is fully added
                tracing::error!("Video queuing not yet supported for {}", id);
                let _ = db::update_download_status(&db_state.path, &id, DownloadStatus::Error);
            }
        }
    }
}
