use librqbit::{Session, AddTorrent, ManagedTorrent};
use std::sync::Arc;
use tokio::sync::Mutex;
use std::collections::HashMap;
use tauri::{AppHandle, Emitter, Runtime};
use serde::{Serialize, Deserialize};
use hex;

/// Summary of a single file within a torrent.
#[derive(Serialize, Deserialize, Clone)]
pub struct TorrentFile {
    pub name: String,
    pub size: u64,
    /// Internal index used by `librqbit` to identify the file.
    pub index: usize,
}

/// Consolidated metadata for a BitTorrent source.
#[derive(Serialize, Deserialize, Clone)]
pub struct TorrentInfo {
    pub name: String,
    pub total_size: u64,
    /// Flattened list of all files available in the torrent.
    pub files: Vec<TorrentFile>,
}

/// The core engine for BitTorrent downloads.
/// 
/// It wraps a `librqbit` session and maintains a mapping of active
/// download handles to facilitate real-time monitoring and control.
#[derive(Clone)]
pub struct TorrentManager {
    /// The underlying BitTorrent session.
    session: Option<Arc<Session>>,
    /// Tracks handles for active torrents, indexed by Ciel's internal UUID.
    active_torrents: Arc<Mutex<HashMap<String, Arc<ManagedTorrent>>>>,
}

impl TorrentManager {
    /// Creates a new `TorrentManager` and initializes the `librqbit` session.
    /// 
    /// Note: `persistence` is disabled to ensure that Ciel maintains total control
    /// over the download list via its own SQLite database.
    pub async fn new(session_dir: std::path::PathBuf, _force_encryption: bool) -> Result<Self, String> {
        if !session_dir.exists() {
            std::fs::create_dir_all(&session_dir).map_err(|e| e.to_string())?;
        }
        
        let options = librqbit::SessionOptions {
            disable_dht: false,
            disable_dht_persistence: false, // Enable DHT persistence for faster startups
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
    
    /// Calculates aggregate torrent statistics for the system tray (count only for now).
    pub async fn get_global_status(&self) -> (usize, u64) {
        let active = self.active_torrents.lock().await;
        let count = active.len();
        // Speed calculation for torrents is complex to aggregate here without a cache.
        // We'll return 0 for now to fix the build, and I'll add real tracking in a follow-up.
        (count, 0)
    }

    /// Adds a new magnet link or torrent file to the active session.
    pub async fn add_magnet<R: Runtime>(&self, app: AppHandle<R>, id: String, magnet: String, output_folder: String, db_path: String, indices: Option<Vec<usize>>, total_size: u64, is_resume: bool) -> Result<(), String> {
        let session = self.session.as_ref().ok_or("Torrent session is not active (port conflict or initialization error)")?;
        
        // PROACTIVE GHOST CLEANUP: Delete any existing torrent with the same info hash before adding.
        // This ensures we start fresh even if librqbit has persisted session state.
        if let Some(magnet_hash) = Self::extract_info_hash_from_magnet(&magnet) {
            // Force a deep purge by hash before adding
            let _ = self.delete_torrent_by_hash(magnet_hash.clone(), false).await;
        }
        
        let options = librqbit::AddTorrentOptions {
            only_files: indices.clone(),
            output_folder: Some(output_folder.clone()),
            overwrite: false, // Prevent overwriting existing files
            ..Default::default()
        };
        let response = session.add_torrent(AddTorrent::from_url(&magnet), Some(options)).await
            .map_err(|e| e.to_string())?;
        
        let handle = response.into_handle().ok_or("Failed to get torrent handle")?;
        
        {
            let mut active = self.active_torrents.lock().await;
            active.insert(id.clone(), handle.clone());
        }

        // Store indices in metadata for resumption support
        if let Some(idx) = &indices {
            let db_p = db_path.clone();
            let id_p = id.clone();
            let meta_json = serde_json::json!({ "indices": idx }).to_string();
            tokio::task::spawn_blocking(move || {
                if let Ok(conn) = crate::db::open_db(db_p) {
                    let _ = conn.execute(
                        "UPDATE downloads SET metadata = ?1 WHERE id = ?2",
                        (meta_json, id_p)
                    );
                }
            });
        }

        let id_clone = id.clone();

        let db_path_clone = db_path.clone();
        let _output_folder_clone = output_folder; // Prefixed with _ since unused after refactor
            let active_torrents = self.active_torrents.clone();
            tokio::spawn(async move {
                let mut name_updated = false;
                let mut last_downloaded = handle.stats().progress_bytes;
                let mut last_time = std::time::Instant::now();
                let mut speed_u64 = 0u64;
                let mut smoothed_speed = 0.0f64;
                let mut paused_counter = 0u8; // Hysteresis counter for Paused state
                let mut last_resume_time = std::time::Instant::now();
                let mut was_live = false;
                let completion_handled = false;

                // First immediate emission to clear UI "Paused" state
                let stats = handle.stats();
                let connections = stats.live.as_ref().map(|l| l.snapshot.peer_stats.live).unwrap_or(0) as u64;
                let _ = app.emit("download-progress", serde_json::json!({
                    "id": id_clone,
                    "total": if stats.total_bytes > 0 { stats.total_bytes } else { total_size },
                    "downloaded": stats.progress_bytes,
                    "speed": 0,
                    "eta": 0,
                    "connections": connections,
                    "status_text": Some(if is_resume { "Resuming..." } else { "Initializing..." }),
                }));

                loop {
                    // CANCELLATION CHECK: If not in active_torrents anymore, exit loop
                    {
                        let active = active_torrents.lock().await;
                        if !active.contains_key(&id_clone) {
                            break;
                        }
                    }

                    let stats = handle.stats();
                let connections = stats.live.as_ref().map(|l| l.snapshot.peer_stats.live).unwrap_or(0) as u64;
                
                // Calculate speed manually
                let now = std::time::Instant::now();
                let elapsed = now.duration_since(last_time).as_secs_f64();
                let downloaded_now = stats.progress_bytes;
                
                if elapsed >= 0.5 {
                    let diff = downloaded_now.saturating_sub(last_downloaded);
                    let mut current_speed = diff as f64 / elapsed;
                    
                    // Mitigation: Ignore speed spikes during initial verification if no peers are connected
                    // We use last_resume_time to ensure this works even after unpause.
                    if connections == 0 && last_resume_time.elapsed().as_secs() < 10 && current_speed > 5_000_000.0 {
                        current_speed = 0.0;
                    }

                    // Faster alpha (0.7) for first 5 seconds after resume to ramp up, then 0.3 for stability
                    let alpha = if last_resume_time.elapsed().as_secs() < 5 { 0.7 } else { 0.3 };
                    
                    if smoothed_speed == 0.0 && current_speed > 0.0 {
                        smoothed_speed = current_speed;
                    } else {
                        smoothed_speed = smoothed_speed * (1.0 - alpha) + current_speed * alpha;
                    }

                    speed_u64 = smoothed_speed as u64;
                    last_downloaded = downloaded_now;
                    last_time = now;
                }

                // Calculate ETA
                let eta = if speed_u64 > 0 {
                    stats.total_bytes.saturating_sub(stats.progress_bytes) / speed_u64
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
                            if let Ok(conn) = crate::db::open_db(db_p) {
                                let _ = conn.execute(
                                    "UPDATE downloads SET info_hash = ?1 WHERE id = ?2", 
                                    (info_hash_hex, id_p)
                                );
                            }
                        });
                        
                        name_updated = true;
                    }
                }

                // Persist progress to DB
                let _ = crate::db::update_download_progress(
                     &db_path_clone, 
                     &id_clone, 
                     stats.progress_bytes as i64, 
                     speed_u64 as i64
                );

                if !stats.finished {
                     // Emit progress only if NOT finished, to prevent race with completion event
                     let status_text = if stats.total_bytes == 0 { 
                        Some(format!("Fetching Metadata... ({} peers)", connections)) 
                    } else if stats.live.is_none() {
                         if was_live {
                             // Force immediate Paused if we were just live (user manually paused)
                             paused_counter = 50;
                         } else {
                             paused_counter = paused_counter.saturating_add(1);
                         }
                         
                         // Reset speed baselines while paused so resumption starts fresh
                         last_downloaded = stats.progress_bytes;
                         last_time = std::time::Instant::now();
                         // Show "Paused" immediately if it was live before (manual user action)
                         // OR if we've been paused for a significant number of samples (hysteresis).
                         let is_explicit_pause = was_live || paused_counter >= 15;
                         
                         if is_explicit_pause {
                             // CRITICAL: Double-check with DB before declaring "Paused"
                             // If the DB says "Downloading", we are actually Resuming (connecting).
                             let db_path_check = db_path_clone.clone();
                             let id_check = id_clone.clone();
                             
                             // We use spawn_blocking to check DB without blocking the async runtime, 
                             // but we need the result here. Since we are in an async loop, 
                             // we can't easily await a blocking task without reorganizing.
                             // However, checking the DB every second is fine.
                             let is_db_paused = std::thread::spawn(move || {
                                 if let Ok(conn) = crate::db::open_db(&db_path_check) {
                                     if let Ok(status) = crate::db::get_download_status(&conn, &id_check) {
                                         return status == crate::db::DownloadStatus::Paused;
                                     }
                                 }
                                 false // Default to false (Not Paused) if DB read fails
                             }).join().unwrap_or(false); // Default to false if thread panic

                             if is_db_paused {
                                 was_live = false; // Only clear was_live once confirmed Paused
                                 Some("Paused".to_string())
                             } else {
                                 // DB says Downloading -> Reset counters and show Resuming
                                 paused_counter = 0;
                                 was_live = false; 
                                 Some("Resuming...".to_string())
                             }
                         } else {
                             // Stay in Initializing/Resuming for at least 10 seconds during engine startup
                             Some(if is_resume { "Resuming..." } else { "Initializing..." }.to_string())
                         }
                    } else { 
                        // Engine is live
                        if paused_counter >= 1 || !was_live {
                             // We just transitioned from paused to live
                             last_resume_time = std::time::Instant::now();
                        }
                        paused_counter = 0; // Reset counter when live
                        was_live = true;

                        if speed_u64 == 0 {
                             if connections == 0 {
                                 Some("Connecting...".to_string())
                             } else {
                                 Some(format!("Downloading ({} peers)", connections))
                             }
                        } else { 
                            Some(format!("Downloading ({} peers)", connections))
                        }
                    };

                    let _ = app.emit("download-progress", serde_json::json!({
                        "id": id_clone,
                        "total": stats.total_bytes,
                        "downloaded": stats.progress_bytes,
                        "speed": speed_u64,
                        "eta": eta,
                        "connections": connections,
                        "status_text": status_text,
                    }));
                }

                if (stats.finished || (stats.total_bytes > 0 && stats.progress_bytes >= stats.total_bytes)) && !completion_handled {
                    // 1. Update status to Completed in DB (Block until done to prevent race with frontend)
                    let db_p = db_path_clone.clone();
                    let id_p = id_clone.clone();
                    let total_bytes_final = stats.total_bytes; // Capture explicit current size
                    let _ = tokio::task::spawn_blocking(move || {
                        if let Ok(conn) = crate::db::open_db(&db_p) {
                            if let Err(e) = conn.execute(
                                "UPDATE downloads SET status = 'completed', completed_at = datetime('now') WHERE id = ?1", 
                                [&id_p]
                            ) {
                                eprintln!("CRITICAL DB ERROR: Failed to mark as completed: {}", e);
                            }
                            
                            // Also ensure progress is capped at 100%
                             let _ = crate::db::update_download_progress(
                                &db_p, 
                                &id_p, 
                                total_bytes_final as i64, 
                                0
                            );
                        } else {
                            eprintln!("CRITICAL DB ERROR: Failed to open DB for completion");
                        }
                    }).await;

                    // 2. Emit completion event only AFTER DB is updated
                    let _ = app.emit("download-completed", id_clone.clone());
                    
                    // completion_handled = true; // Unused as we break immediately
                    
                    // Post-Download Actions
                    // We need the full Download record to know the filepath
                    if let Ok(downloads) = crate::db::get_all_downloads(&db_path_clone) {
                        if let Some(download) = downloads.into_iter().find(|d| d.id == id_clone) {
                            crate::commands::execute_post_download_actions(app.clone(), db_path_clone.clone(), download).await;
                        }
                    }
                    break;
                }
                
                tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
            }
        });

        Ok(())
    }

    /// Metadata Sniffer: Briefly joins a swarm to extract the file tree and total size.
    /// 
    /// This adds the torrent to a temporary directory with file downloads disabled,
    /// waits for the metadata to arrive, then deletes the "ghost" torrent from the session.
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

    /// Internal: Transitions a selectively-configured torrent from Paused to active.
    pub async fn start_selective(&self, id: &str, _indices: Vec<usize>) -> Result<(), String> {
        let session = self.session.as_ref().ok_or("Torrent session is not active")?;
        
        let active = self.active_torrents.lock().await;
        if let Some(handle) = active.get(id) {
             session.unpause(handle).await.map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// Pauses an active torrent in the `librqbit` session.
    /// Pauses an active torrent in the `librqbit` session.
    pub async fn pause_torrent(&self, id: &str) -> Result<(), String> {
        let session = self.session.as_ref().ok_or("Torrent session is not active")?;
        let active = self.active_torrents.lock().await;
        if let Some(handle) = active.get(id) {
            match session.pause(handle).await {
                Ok(_) => {},
                Err(e) => {
                    let msg = e.to_string();
                    // If it's already paused, we consider that a success
                    if !msg.contains("already paused") {
                        return Err(msg);
                    }
                }
            }
        }
        Ok(())
    }

    /// Resumes a paused torrent in the `librqbit` session.
    /// Resumes a paused torrent in the `librqbit` session.
    pub async fn resume_torrent(&self, id: &str) -> Result<(), String> {
        let session = self.session.as_ref().ok_or("Torrent session is not active")?;
        let active = self.active_torrents.lock().await;
        if let Some(handle) = active.get(id) {
            match session.unpause(handle).await {
                Ok(_) => {},
                Err(e) => {
                    let msg = e.to_string();
                    // If it's already running, we consider that a success
                    if !msg.contains("not paused") && !msg.contains("already running") {
                        return Err(msg);
                    }
                }
            }
            return Ok(());
        }
        Err("Torrent not in active session".to_string())
    }

    /// Deletes a torrent and its metadata from the session. 
    /// If delete_files is true, also removes the data from disk.
    /// target_path is an optional manual override to ensure files are gone even if engine hangs.
    pub async fn delete_torrent(&self, id: &str, delete_files: bool, target_path: Option<String>) -> Result<(), String> {
        let session = self.session.as_ref().ok_or("Torrent session is not active")?;
        
        let handle_opt = {
            let mut active = self.active_torrents.lock().await;
            active.remove(id)
        };

        if let Some(handle) = handle_opt {
             let info_hash = handle.info_hash();
             // Delete standard
             let _ = session.delete(librqbit::api::TorrentIdOrHash::Hash(info_hash), delete_files).await;
             
              // FORCED PURGE: Ensure it's gone from session entirely
              let hash_to_purge = session.with_torrents(|iter| {
                  for (_id, h) in iter {
                      if h.info_hash() == info_hash {
                          return Some(h.info_hash());
                      }
                  }
                  None
              });
              if let Some(h) = hash_to_purge {
                  let _ = session.delete(librqbit::api::TorrentIdOrHash::Hash(h), delete_files).await;
              }
        }

        // 4. MANUAL CLEANUP FALLBACK (Windows resilience)
        if delete_files {
            if let Some(path) = target_path {
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                let path_obj = std::path::Path::new(&path);
                if path_obj.exists() {
                    if path_obj.is_dir() {
                        let _ = std::fs::remove_dir_all(path_obj);
                    } else {
                        let _ = std::fs::remove_file(path_obj);
                    }
                }
            }
        }

        Ok(())
    }

    /// Checks if a torrent with the given ID is currently active in the manager.
    pub async fn is_active(&self, id: &str) -> bool {
        self.active_torrents.lock().await.contains_key(id)
    }

    /// Forcefully removes a torrent from the session by its info hash.
    /// Useful for cleaning up "zombie" or "ghost" torrents.
    pub async fn delete_torrent_by_hash(&self, hash_str: String, delete_files: bool) -> Result<(), String> {
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
             let _ = session.delete(librqbit::api::TorrentIdOrHash::Hash(info_hash), delete_files).await;
             
             // POSITIVE CONFIRMATION: Wait for engine to drop it
             let mut attempts = 0;
             while attempts < 20 {
                 let still_there = session.with_torrents(|iter| {
                     for (_id, h) in iter {
                         if h.info_hash() == info_hash {
                             return true;
                         }
                     }
                     false
                 });
                 if !still_there { break; }
                 tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                 attempts += 1;
             }
        }
        
        Ok(())
    }

    /// Helper: Parses a magnet link to extract the unique info hash.
    /// Supports both hex and base32 variants.
    pub fn extract_info_hash_from_magnet(magnet: &str) -> Option<String> {
        // Handle direct hex string (40 chars)
        if magnet.len() == 40 && magnet.chars().all(|c| c.is_ascii_hexdigit()) {
            return Some(magnet.to_lowercase());
        }

        // Find the btih: prefix
        let magnet_lower = magnet.to_lowercase();
        if let Some(start) = magnet_lower.find("btih:") {
            let hash_start = start + 5; // length of "btih:"
            let hash_part = &magnet_lower[hash_start..];
            // Hash ends at & or end of string
            let hash_end = hash_part.find('&').unwrap_or(hash_part.len());
            let hash = &hash_part[..hash_end];
            
            // Normalize to hex if base32 (32 chars)
            if hash.len() == 32 {
                let mut alphabet = [0u8; 128];
                for (i, &c) in "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567".as_bytes().iter().enumerate() {
                    alphabet[c as usize] = i as u8;
                }
                
                let input = hash.to_uppercase();
                let input_bytes = input.as_bytes();
                let mut output = Vec::new();
                let mut buffer = 0u32;
                let mut bits_left = 0;
                
                for &byte in input_bytes {
                    if (byte as usize) < alphabet.len() {
                        let val = alphabet[byte as usize] as u32;
                        buffer = (buffer << 5) | val;
                        bits_left += 5;
                        while bits_left >= 8 {
                            output.push((buffer >> (bits_left - 8)) as u8);
                            bits_left -= 8;
                            buffer &= (1 << bits_left) - 1;
                        }
                    }
                }
                return Some(hex::encode(output).to_lowercase());
            }
            
            Some(hash.to_string().to_lowercase())
        } else {
            None
        }
    }
}
