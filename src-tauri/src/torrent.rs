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

    pub async fn add_magnet(&self, app: AppHandle, id: String, magnet: String, _output_folder: String) -> Result<(), String> {
        let response = self.session.add_torrent(AddTorrent::from_url(magnet), None).await
            .map_err(|e| e.to_string())?;
        
        let handle = response.into_handle().ok_or("Failed to get torrent handle")?;
        
        {
            let mut active = self.active_torrents.lock().await;
            active.insert(id.clone(), handle.clone());
        }

        let id_clone = id.clone();

        tokio::spawn(async move {
            loop {
                let stats = handle.stats();
                
                let speed = stats.live.as_ref().map(|l| (l.download_speed.mbps * 1024.0 * 1024.0 / 8.0) as u64).unwrap_or(0);
                let connections = 0; // TODO: find exact peer field
                let eta = 0; // TODO: find exact Duration field

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
