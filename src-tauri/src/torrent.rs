use librqbit::{Session, AddTorrent, ManagedTorrent};
use std::sync::Arc;
use tokio::sync::Mutex;
use std::collections::HashMap;
use tauri::{AppHandle, Emitter};
use serde::{Serialize, Deserialize};
use hex;

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
    session: Option<Arc<Session>>,
    active_torrents: Arc<Mutex<HashMap<String, Arc<ManagedTorrent>>>>, // Maps Ciel ID to librqbit handle
}

impl TorrentManager {
    pub async fn new(_force_encryption: bool) -> Result<Self, String> {
        // Use a temporary session directory that gets cleared on startup.
        // This prevents stale session state from causing "ghost" torrents.
        // Actual downloads go to user-specified output_folder per torrent.
        let session_dir = std::env::temp_dir().join("ciel_torrent_session");
        
        if !session_dir.exists() {
            std::fs::create_dir_all(&session_dir).map_err(|e| e.to_string())?;
        }
        
        let options = librqbit::SessionOptions {
            disable_dht: false,
            disable_dht_persistence: true,  // Don't persist DHT state
            persistence: None,              // Don't persist torrent state
            ..Default::default()
        };
        
        match Session::new_with_opts(session_dir, options).await {
            Ok(session) => Ok(Self {
                session: Some(session),
                active_torrents: Arc::new(Mutex::new(HashMap::new())),
            }),
            Err(e) => {
                eprintln!("Failed to start torrent session: {}. Torrents will be disabled.", e);
                Ok(Self {
                    session: None,
                    active_torrents: Arc::new(Mutex::new(HashMap::new())),
                })
            }
        }
    }

    pub async fn add_magnet(&self, app: AppHandle, id: String, magnet: String, output_folder: String, db_path: String, indices: Option<Vec<usize>>) -> Result<(), String> {
        let session = self.session.as_ref().ok_or("Torrent session is not active (port conflict or initialization error)")?;
        
        // PROACTIVE GHOST CLEANUP: Delete any existing torrent with the same info hash before adding.
        // This ensures we start fresh even if librqbit has persisted session state.
        if let Some(magnet_hash) = Self::extract_info_hash_from_magnet(&magnet) {
            let hash_to_delete = session.with_torrents(|iter| {
                for (_id, handle) in iter {
                    let h_hex = hex::encode(handle.info_hash().0).to_lowercase();
                    // Magnet might use hex (40 chars) or base32 (32 chars). Check hex match.
                    if h_hex == magnet_hash {
                        return Some(handle.info_hash());
                    }
                }
                None
            });

            if let Some(info_hash) = hash_to_delete {
                // Delete the ghost, keeping files (delete_files = false)
                let _ = session.delete(librqbit::api::TorrentIdOrHash::Hash(info_hash), false).await;
            }
        }
        
        let options = librqbit::AddTorrentOptions {
            only_files: indices.clone(),
            output_folder: Some(output_folder.clone()),
            overwrite: true, // Allow overwriting to prevent "file exists" errors
            ..Default::default()
        };
        let response = session.add_torrent(AddTorrent::from_url(&magnet), Some(options)).await
            .map_err(|e| e.to_string())?;
        
        let handle = response.into_handle().ok_or("Failed to get torrent handle")?;
        
        {
            let mut active = self.active_torrents.lock().await;
            active.insert(id.clone(), handle.clone());
        }

        let id_clone = id.clone();

        let db_path_clone = db_path.clone();
        let _output_folder_clone = output_folder; // Prefixed with _ since unused after refactor
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

                // 1. Update Size & Info Hash on Metadata discovery
                // NOTE: We do NOT update filename/filepath here - they are already set correctly
                // by commands.rs with unique paths like "Movie (1).mkv"
                if !name_updated && stats.total_bytes > 0 {
                    let name_result = handle.with_metadata(|m| m.name.clone());
                    if let Ok(_real_name) = name_result {
                        let total_size = stats.total_bytes;
                        
                        // Update DB size
                        let _ = crate::db::update_download_size(&db_path_clone, &id_clone, total_size as i64);
                        
                        // Update info_hash in DB
                        let db_p = db_path_clone.clone();
                        let id_p = id_clone.clone();
                        let info_hash_hex = hex::encode(handle.info_hash().0);

                        tokio::task::spawn_blocking(move || {
                            if let Ok(conn) = rusqlite::Connection::open(db_p) {
                                let _ = conn.execute(
                                    "UPDATE downloads SET info_hash = ?1 WHERE id = ?2", 
                                    (info_hash_hex, id_p)
                                );
                            }
                        });
                        
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

                // Persist progress to DB
                let _ = crate::db::update_download_progress(
                     &db_path_clone, 
                     &id_clone, 
                     stats.progress_bytes as i64, 
                     speed as i64
                );

                if stats.finished {
                    let _ = app.emit("download-completed", id_clone.clone());
                    
                    // Final DB update: set progress to 100% and status to Completed
                    let _ = crate::db::update_download_progress(
                        &db_path_clone, 
                        &id_clone, 
                        stats.total_bytes as i64, 
                        0
                    );
                    
                    // Update status to Completed
                    let db_p = db_path_clone.clone();
                    let id_p = id_clone.clone();
                    tokio::task::spawn_blocking(move || {
                        if let Ok(conn) = rusqlite::Connection::open(&db_p) {
                            let _ = conn.execute(
                                "UPDATE downloads SET status = 'Completed', completed_at = datetime('now') WHERE id = ?1", 
                                [&id_p]
                            );
                        }
                    });
                    
                    // Post-Download Actions
                    // We need the full Download record to know the filepath
                    if let Ok(downloads) = crate::db::get_all_downloads(&db_path_clone) {
                        if let Some(download) = downloads.into_iter().find(|d| d.id == id_clone) {
                            crate::commands::execute_post_download_actions(app.clone(), db_path_clone.clone(), download).await;
                        }
                    }
                    break;
                }
                
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        });

        Ok(())
    }

    pub async fn analyze_magnet(&self, magnet: String) -> Result<TorrentInfo, String> {
        let session = self.session.as_ref().ok_or("Torrent session is not active (port conflict or initialization error)")?;

        let temp_dir = std::env::temp_dir().to_string_lossy().to_string();
        let options = librqbit::AddTorrentOptions {
            output_folder: Some(temp_dir),
            only_files: Some(vec![]),
            overwrite: true,
            ..Default::default()
        };
        let response = session.add_torrent(librqbit::AddTorrent::from_url(magnet), Some(options)).await
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
                    session.delete(librqbit::api::TorrentIdOrHash::Hash(info_hash), false).await
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
        let session = self.session.as_ref().ok_or("Torrent session is not active")?;
        
        let active = self.active_torrents.lock().await;
        if let Some(handle) = active.get(id) {
             session.unpause(handle).await.map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub async fn pause_torrent(&self, id: &str) -> Result<(), String> {
        let session = self.session.as_ref().ok_or("Torrent session is not active")?;
        let active = self.active_torrents.lock().await;
        if let Some(handle) = active.get(id) {
            session.pause(handle).await.map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub async fn resume_torrent(&self, id: &str) -> Result<(), String> {
        let session = self.session.as_ref().ok_or("Torrent session is not active")?;
        let active = self.active_torrents.lock().await;
        if let Some(handle) = active.get(id) {
            session.unpause(handle).await.map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub async fn delete_torrent(&self, id: &str) -> Result<(), String> {
        let session = self.session.as_ref().ok_or("Torrent session is not active")?;
        
        let handle_opt = {
            let mut active = self.active_torrents.lock().await;
            active.remove(id)
        };

        if let Some(handle) = handle_opt {
             let info_hash = handle.info_hash();
             session.delete(librqbit::api::TorrentIdOrHash::Hash(info_hash), false).await
                 .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub async fn delete_torrent_by_hash(&self, hash_str: String) -> Result<(), String> {
        let session = self.session.as_ref().ok_or("Torrent session is not active")?;
        
        let hash_to_delete = session.with_torrents(|iter| {
            for (_id, handle) in iter {
                let h_hex = hex::encode(handle.info_hash().0);
                if h_hex.eq_ignore_ascii_case(&hash_str) {
                    return Some(handle.info_hash());
                }
            }
            None
        });

        if let Some(info_hash) = hash_to_delete {
             session.delete(librqbit::api::TorrentIdOrHash::Hash(info_hash), false).await
                 .map_err(|e| e.to_string())?;
        }
        
        Ok(())
    }

    /// Extracts the info hash from a magnet URL.
    /// Magnet format: magnet:?xt=urn:btih:INFOHASH&...
    fn extract_info_hash_from_magnet(magnet: &str) -> Option<String> {
        // Find the btih: prefix
        let magnet_lower = magnet.to_lowercase();
        if let Some(start) = magnet_lower.find("btih:") {
            let hash_start = start + 5; // length of "btih:"
            let hash_part = &magnet_lower[hash_start..];
            // Hash ends at & or end of string
            let hash_end = hash_part.find('&').unwrap_or(hash_part.len());
            let hash = &hash_part[..hash_end];
            // Return lowercase hex hash (40 chars for hex, 32 for base32)
            Some(hash.to_string())
        } else {
            None
        }
    }
}
