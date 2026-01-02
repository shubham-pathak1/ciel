use librqbit::{Session, AddTorrent, ManagedTorrent};
use std::sync::Arc;
use tokio::sync::Mutex;
use std::collections::HashMap;
use tauri::{AppHandle, Emitter};
use std::path::PathBuf;
use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize, Clone)]
pub struct TorrentFile {
    pub name: String,
    pub size: u64,
    pub index: usize,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct TorrentInfo {
    pub name: String,
    pub total_size: u64,
    pub files: Vec<TorrentFile>,
}

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

    pub async fn add_magnet(&self, app: AppHandle, id: String, magnet: String, _output_folder: String, db_path: String, indices: Option<Vec<usize>>) -> Result<(), String> {
        let options = librqbit::AddTorrentOptions {
            only_files: indices,
            ..Default::default()
        };
        let response = self.session.add_torrent(AddTorrent::from_url(magnet), Some(options)).await
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
                    let name_result = handle.with_metadata(|m| m.name.clone());
                    if let Ok(real_name) = name_result {
                        let total_size = stats.total_bytes;
                        
                        // Update DB size
                        let _ = crate::db::update_download_size(&db_path_clone, &id_clone, total_size as i64);
                        
                        // Update DB filename manually
                        let db_p = db_path_clone.clone();
                        let id_p = id_clone.clone();
                        let name_p = real_name.clone();
                        tokio::task::spawn_blocking(move || {
                            if let Ok(conn) = rusqlite::Connection::open(db_p) {
                                let _ = conn.execute("UPDATE downloads SET filename = ?1 WHERE id = ?2", (name_p, id_p));
                            }
                        });

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

    pub async fn analyze_magnet(&self, magnet: String) -> Result<TorrentInfo, String> {
        let temp_dir = std::env::temp_dir().to_string_lossy().to_string();
        let options = librqbit::AddTorrentOptions {
            output_folder: Some(temp_dir),
            only_files: Some(vec![]),
            overwrite: true,
            ..Default::default()
        };
        let response = self.session.add_torrent(librqbit::AddTorrent::from_url(magnet), Some(options)).await
            .map_err(|e| e.to_string())?;
        
        let handle = response.into_handle().ok_or("Failed to get torrent handle")?;
        
        // Wait for metadata (timeout after 30s)
        let start = std::time::Instant::now();
        loop {
            // Try to get metadata
            let result = handle.with_metadata(|m| {
                let files = m.file_infos.iter().enumerate().map(|(i, f)| {
                    TorrentFile {
                        name: f.relative_filename.to_string_lossy().to_string(),
                        size: f.len,
                        index: i,
                    }
                }).collect();

                TorrentInfo {
                    name: m.name.clone().unwrap_or_default(),
                    total_size: m.file_infos.iter().map(|f| f.len).sum(),
                    files,
                }
            });

            match result {
                Ok(info) => {
                    // Success! Remove from session and return info
                    // Remove from session so it can be re-added with selective files later
                    // Use the infohash from the handle
                    let info_hash = handle.info_hash();
                    self.session.delete(librqbit::api::TorrentIdOrHash::Hash(info_hash), false).await
                        .map_err(|e| e.to_string())?;
                    return Ok(info);
                },
                Err(_) => {
                    // Metadata not ready yet or other error (likely just not ready if we just added it)
                    if start.elapsed().as_secs() > 30 {
                        return Err("Timeout waiting for metadata".to_string());
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                }
            }
        }
    }

    pub async fn start_selective(&self, id: &str, _indices: Vec<usize>) -> Result<(), String> {
        // Since we removed it during analysis, this might not be in active_torrents yet
        // Wait, start_selective is called AFTER add_torrent in the new flow?
        // Let's check commands.rs
        let active = self.active_torrents.lock().await;
        if let Some(handle) = active.get(id) {
             self.session.unpause(handle).await.map_err(|e| e.to_string())?;
        }
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
