use librqbit::{Session, AddTorrent, ManagedTorrent};
use std::sync::Arc;
use tokio::sync::Mutex;
use std::collections::HashMap;
use tauri::{AppHandle, Emitter};
use std::path::PathBuf;

pub struct TorrentManager {
    session: Arc<Session>,
    active_torrents: Arc<Mutex<HashMap<String, Arc<ManagedTorrent>>>>, // Maps Ciel ID to librqbit handle
}

impl TorrentManager {
    pub async fn new() -> Self {
        // Default download folder for the session (we can override per torrent if library allows)
        let download_dir = PathBuf::from("./downloads");
        if !download_dir.exists() {
            std::fs::create_dir_all(&download_dir).ok();
        }
        
        let session = Session::new(download_dir).await.expect("Failed to create librqbit session");
        Self {
            session: session,
            active_torrents: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn add_magnet(&self, app: AppHandle, id: String, magnet: String, _output_folder: String, _db_path: String) -> Result<(), String> {
        let response = self.session.add_torrent(AddTorrent::from_url(magnet), None).await
            .map_err(|e| e.to_string())?;
        
        let handle = response.into_handle().ok_or("Failed to get torrent handle")?;
        
        {
            let mut active = self.active_torrents.lock().await;
            active.insert(id.clone(), handle.clone());
        }

        let id_clone = id.clone();

        let db_path_clone = db_path.clone();
        tokio::spawn(async move {
            let mut name_updated = false;
            let mut last_downloaded = handle.stats().progress_bytes;
            let mut last_time = std::time::Instant::now();

            loop {
                let stats = handle.stats();
                
                // Calculate speed manually for 100% accuracy
                let now = std::time::Instant::now();
                let elapsed = now.duration_since(last_time).as_secs_f64();
                let downloaded_now = stats.progress_bytes;
                
                let mut speed = 0;
                if elapsed > 0.5 {
                    let diff = downloaded_now.saturating_sub(last_downloaded);
                    speed = (diff as f64 / elapsed) as u64;
                    last_downloaded = downloaded_now;
                    last_time = now;
                } else {
                    // During very short intervals, keep the last known speed if available?
                    // For simplicity, we'll just wait for the next iteration.
                }

                let connections = stats.live.as_ref().map(|l| l.snapshot.peer_stats.live).unwrap_or(0) as u64;
                
                // Calculate ETA
                let eta = if speed > 0 {
                    stats.total_bytes.saturating_sub(stats.progress_bytes) / speed
                } else {
                    0
                };

                // 1. Update Filename & Metadata discovery
                if !name_updated && stats.total_bytes > 0 {
                    if let Some(info) = handle.shared().info() {
                        let real_name = info.name.clone();
                        let total_size = stats.total_bytes;
                        
                        // Update DB size
                        let _ = crate::db::update_download_size(&db_path_clone, &id_clone, total_size as i64);
                        
                        // Update DB filename manually
                        let db_p = db_path_clone.clone();
                        let id_p = id_clone.clone();
                        let name_p = real_name.clone();
                        let _ = tokio::task::spawn_blocking(move || {
                            if let Ok(conn) = rusqlite::Connection::open(db_p) {
                                let _ = conn.execute("UPDATE downloads SET filename = ?1 WHERE id = ?2", (name_p, id_p));
                            }
                        }).await;

                        let _ = app.emit("download-name-updated", serde_json::json!({
                            "id": id_clone,
                            "filename": real_name
                        }));
                        
                        name_updated = true;
                    }
                }

                // Emit progress
                let _ = app.emit("download-progress", serde_json::json!({
                    "id": id_clone,
                    "total": stats.total_bytes,
                    "downloaded": stats.progress_bytes,
                    "speed": speed,
                    "eta": eta,
                    "connections": connections,
                }));

                if stats.finished {
                    let _ = app.emit("download-completed", id_clone);
                    break;
                }
                
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        });

        Ok(())
    }

    pub async fn pause_torrent(&self, id: &str) -> Result<(), String> {
        let active = self.active_torrents.lock().await;
        if let Some(handle) = active.get(id) {
            self.session.pause(handle).await.map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub async fn resume_torrent(&self, id: &str) -> Result<(), String> {
        let active = self.active_torrents.lock().await;
        if let Some(handle) = active.get(id) {
            self.session.unpause(handle).await.map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}
