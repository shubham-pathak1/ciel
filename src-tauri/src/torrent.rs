use librqbit::{Session, AddTorrent, ManagedTorrent};
use std::sync::Arc;
use tokio::sync::Mutex;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
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
    /// Ephemeral analysis token used to avoid fetching metadata twice.
    pub id: Option<String>,
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
    /// The underlying BitTorrent session (initialized in background).
    session: Arc<Mutex<Option<Arc<Session>>>>,
    /// Tracks handles for active torrents, indexed by Ciel's internal UUID.
    active_torrents: Arc<Mutex<HashMap<String, Arc<ManagedTorrent>>>>,
    /// Short-lived cache of analyzed torrent bytes keyed by analysis token.
    analyzed_torrents: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    /// Memory cache for paused states to avoid constant DB polling.
    paused_downloads: Arc<Mutex<HashSet<String>>>,
}

impl TorrentManager {
    fn extract_initial_peers_from_magnet(magnet: &str) -> Vec<std::net::SocketAddr> {
        let parsed = match url::Url::parse(magnet) {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        };

        let mut peers = Vec::new();
        let mut seen = HashSet::new();
        for (key, value) in parsed.query_pairs() {
            if key != "x.pe" {
                continue;
            }
            if let Ok(addr) = value.parse::<std::net::SocketAddr>() {
                if seen.insert(addr) {
                    peers.push(addr);
                }
            }
        }
        peers
    }

    fn cleanup_unselected_placeholder_files(
        output_folder: &str,
        selected_indices: &[usize],
        file_entries: &[(usize, String)],
    ) {
        if selected_indices.is_empty() {
            return;
        }

        let selected: HashSet<usize> = selected_indices.iter().copied().collect();
        let base = Path::new(output_folder);
        let mut parent_dirs: Vec<PathBuf> = Vec::new();

        for (idx, relative_path) in file_entries {
            if selected.contains(idx) {
                continue;
            }

            let full_path = base.join(relative_path);
            if let Ok(meta) = std::fs::metadata(&full_path) {
                if meta.is_file() {
                    let _ = std::fs::remove_file(&full_path);
                    if let Some(parent) = Path::new(relative_path).parent() {
                        if !parent.as_os_str().is_empty() {
                            parent_dirs.push(parent.to_path_buf());
                        }
                    }
                }
            }
        }

        parent_dirs.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
        parent_dirs.dedup();

        for relative_dir in parent_dirs {
            let _ = std::fs::remove_dir(base.join(relative_dir));
        }
    }

    /// Creates a new `TorrentManager` and spawns a background task to initialize the `librqbit` session.
    pub fn new(session_dir: std::path::PathBuf, _force_encryption: bool) -> Self {
        let session = Arc::new(Mutex::new(None));
        let session_clone = session.clone();
        let session_dir_clone = session_dir.clone();

        // Spawn background initialization to prevent UI freeze during startup
        tauri::async_runtime::spawn(async move {
            // Ensure directory exists in background
            if !session_dir_clone.exists() {
                let _ = std::fs::create_dir_all(&session_dir_clone);
            }

            let options = librqbit::SessionOptions {
                disable_dht: false,
                disable_dht_persistence: false,
                // Persist session and bitfield state to enable fast resume across restarts.
                // Without fastresume, restored torrents can still trigger long local verification.
                fastresume: true,
                persistence: Some(librqbit::SessionPersistenceConfig::Json {
                    folder: Some(session_dir_clone.clone()),
                }),
                ..Default::default()
            };
            
            match Session::new_with_opts(session_dir_clone, options).await {
                Ok(s) => {
                    let mut sess = session_clone.lock().await;
                    *sess = Some(s);
                    println!("[Torrent] Engine initialized successfully in background.");
                },
                Err(e) => {
                    eprintln!("Failed to start torrent session in background: {}. Torrents will be disabled.", e);
                }
            }
        });

        Self {
            session,
            active_torrents: Arc::new(Mutex::new(HashMap::new())),
            analyzed_torrents: Arc::new(Mutex::new(HashMap::new())),
            paused_downloads: Arc::new(Mutex::new(HashSet::new())),
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

    /// Waits until the session is initialized, up to `timeout_ms`.
    pub async fn wait_until_ready(&self, timeout_ms: u64) -> bool {
        let timeout = std::time::Duration::from_millis(timeout_ms);
        let poll = std::time::Duration::from_millis(100);
        let start = std::time::Instant::now();

        loop {
            if self.session.lock().await.is_some() {
                return true;
            }

            if start.elapsed() >= timeout {
                return false;
            }

            tokio::time::sleep(poll).await;
        }
    }

    /// Consumes analyzed torrent bytes for a previous `analyze_magnet` call.
    pub async fn consume_analysis_bytes(&self, analysis_id: &str) -> Option<Vec<u8>> {
        self.analyzed_torrents.lock().await.remove(analysis_id)
    }

    /// Adds a new magnet link or torrent file to the active session.
    pub async fn add_magnet<R: Runtime>(&self, app: AppHandle<R>, id: String, magnet: String, output_folder: String, db_path: String, indices: Option<Vec<usize>>, total_size: u64, is_resume: bool, start_paused: bool, source_torrent_bytes: Option<Vec<u8>>) -> Result<(), String> {
        let session_guard = self.session.lock().await;
        let session = session_guard
            .as_ref()
            .ok_or("Torrent session is not yet initialized. Please wait a moment.")?
            .clone();
        drop(session_guard);

        let initial_peers = Self::extract_initial_peers_from_magnet(&magnet);
        let initial_peers_opt = if initial_peers.is_empty() {
            None
        } else {
            Some(initial_peers.clone())
        };
        if let Some(peers) = initial_peers_opt.as_ref() {
            println!(
                "[Torrent] {}: seeding {} initial peer(s) from magnet x.pe",
                id,
                peers.len()
            );
        }
        
        if start_paused {
            let mut paused = self.paused_downloads.lock().await;
            paused.insert(id.clone());
        }
        
        let response = match source_torrent_bytes {
            Some(torrent_bytes) => {
                let options = librqbit::AddTorrentOptions {
                    only_files: indices.clone(),
                    output_folder: Some(output_folder.clone()),
                    overwrite: is_resume,
                    initial_peers: initial_peers_opt.clone(),
                    ..Default::default()
                };
                session
                    .add_torrent(AddTorrent::from_bytes(torrent_bytes), Some(options))
                    .await
            }
            None => {
                let options = librqbit::AddTorrentOptions {
                    only_files: indices.clone(),
                    output_folder: Some(output_folder.clone()),
                    overwrite: is_resume,
                    initial_peers: initial_peers_opt.clone(),
                    ..Default::default()
                };
                session
                    .add_torrent(AddTorrent::from_url(&magnet), Some(options))
                    .await
            }
        }
        .map_err(|e| e.to_string())?;
        
        let handle = response.into_handle().ok_or("Failed to get torrent handle")?;
        
        if start_paused {
            let _ = session.pause(&handle).await;
        } else if is_resume {
            // Restart resume path: ensure handle is actively unpaused.
            // AlreadyManaged handles can be left paused depending on recovered state.
            if let Err(e) = session.unpause(&handle).await {
                let msg = e.to_string();
                if !msg.contains("not paused")
                    && !msg.contains("already running")
                    && !msg.contains("already live")
                {
                    eprintln!("[Torrent] Resume unpause failed for {}: {}", id, msg);
                }
            }
        }
        
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
        let session_for_monitor = session.clone();

        let db_path_clone = db_path.clone();
        let output_folder_clone = output_folder;
        let selected_indices_for_cleanup = indices;
        let active_torrents = self.active_torrents.clone();
        let paused_downloads = self.paused_downloads.clone();
        let initial_peers_count = initial_peers.len();
        tokio::spawn(async move {
                let mut name_updated = false;
                let mut last_downloaded = handle.stats().progress_bytes;
                let mut last_time = std::time::Instant::now();
                let mut speed_u64 = 0u64;
                let mut verified_speed_u64 = 0u64;
                let mut smoothed_speed = 0.0f64;
                let mut paused_counter = 0u8; // Tracks transitions between non-live/live states
                let mut last_resume_time = std::time::Instant::now();
                let mut was_live = false;
                let completion_handled = false;
                let mut stalled_since: Option<std::time::Instant> = None;
                let mut live_stalled_since: Option<std::time::Instant> = None;
                let mut last_recovery_poke: Option<std::time::Instant> = None;
                let mut last_progress_seen = handle.stats().progress_bytes;
                let mut phase_key = if is_resume {
                    "restoring_session".to_string()
                } else {
                    "initializing".to_string()
                };
                let mut phase_started_at = std::time::Instant::now();
                let startup_started_at = std::time::Instant::now();
                let startup_baseline_bytes = handle.stats().progress_bytes;
                let startup_baseline_fetched = handle
                    .stats()
                    .live
                    .as_ref()
                    .map(|l| l.snapshot.fetched_bytes)
                    .unwrap_or(0);
                let mut startup_metadata_at: Option<std::time::Duration> = None;
                let mut startup_live_at: Option<std::time::Duration> = None;
                let mut startup_peers_at: Option<std::time::Duration> = None;
                let mut startup_first_network_at: Option<std::time::Duration> = None;
                let mut startup_first_byte_at: Option<std::time::Duration> = None;
                let mut startup_timeout_logged = false;
                let mut startup_first_byte_logged = false;

                // First immediate emission to clear UI "Paused" state
                let stats = handle.stats();
                let connections = stats.live.as_ref().map(|l| l.snapshot.peer_stats.live).unwrap_or(0) as u64;
                let network_received = stats
                    .live
                    .as_ref()
                    .map(|l| l.snapshot.fetched_bytes)
                    .unwrap_or(stats.progress_bytes);
                let _ = app.emit("download-progress", serde_json::json!({
                    "id": id_clone,
                    "total": if stats.total_bytes > 0 { stats.total_bytes } else { total_size },
                    "downloaded": stats.progress_bytes,
                    "network_received": network_received,
                    "verified_speed": 0u64,
                    "speed": 0,
                    "eta": 0,
                    "connections": connections,
                    "status_text": Some(if is_resume { "Resuming..." } else { "Initializing..." }),
                    "status_phase": phase_key,
                    "phase_elapsed_secs": 0u64,
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
                let startup_elapsed = now.duration_since(startup_started_at);
                let fetched_now = stats
                    .live
                    .as_ref()
                    .map(|l| l.snapshot.fetched_bytes)
                    .unwrap_or(startup_baseline_fetched);

                if startup_metadata_at.is_none() && stats.total_bytes > 0 {
                    startup_metadata_at = Some(startup_elapsed);
                }
                if startup_live_at.is_none() && stats.live.is_some() {
                    startup_live_at = Some(startup_elapsed);
                }
                if startup_peers_at.is_none() && connections > 0 {
                    startup_peers_at = Some(startup_elapsed);
                }
                if startup_first_network_at.is_none() && fetched_now > startup_baseline_fetched {
                    startup_first_network_at = Some(startup_elapsed);
                }
                if startup_first_byte_at.is_none() && downloaded_now > startup_baseline_bytes {
                    startup_first_byte_at = Some(startup_elapsed);
                }
                let network_received = fetched_now.max(downloaded_now);
                
                if elapsed >= 0.5 {
                    let diff = downloaded_now.saturating_sub(last_downloaded);
                    let mut verified_speed = diff as f64 / elapsed;
                    if stats.live.is_none() || connections == 0 {
                        verified_speed = 0.0;
                    }
                    verified_speed_u64 = verified_speed as u64;

                    let mut current_speed = verified_speed;
                    let live_speed_bps = stats
                        .live
                        .as_ref()
                        .map(|l| (l.download_speed.mbps.max(0.0) * 1024.0 * 1024.0) as u64)
                        .unwrap_or(0);

                    // Verification/initialization can advance progress counters without network transfer.
                    // Clamp speed while there is no live peer activity.
                    if stats.live.is_none() || connections == 0 {
                        current_speed = 0.0;
                    } else if live_speed_bps > 0 {
                        // Prefer engine-estimated fetch speed to avoid "0 speed until first verified piece" UX.
                        current_speed = live_speed_bps as f64;
                    }

                    // Keep an additional startup spike guard around quick resume transitions.
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

                if stats.live.is_none() || connections == 0 {
                    speed_u64 = 0;
                    verified_speed_u64 = 0;
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
                     let is_cached_paused = {
                         let paused = paused_downloads.lock().await;
                         paused.contains(&id_clone)
                     };

                     if !is_cached_paused && !startup_timeout_logged
                        && startup_first_byte_at.is_none()
                        && startup_first_network_at.is_none()
                        && startup_elapsed >= std::time::Duration::from_secs(45)
                     {
                        let detail = format!(
                            "resume={}, reason=timeout_no_first_byte, elapsed_ms={}, metadata_ms={}, live_ms={}, peers_ms={}, first_network_ms={}, initial_peers={}",
                            is_resume,
                            startup_elapsed.as_millis(),
                            startup_metadata_at.map(|d| d.as_millis()).unwrap_or(0),
                            startup_live_at.map(|d| d.as_millis()).unwrap_or(0),
                            startup_peers_at.map(|d| d.as_millis()).unwrap_or(0),
                            startup_first_network_at.map(|d| d.as_millis()).unwrap_or(0),
                            initial_peers_count,
                        );
                        println!("[Torrent][Startup][{}] {}", id_clone, detail);
                        let db_p = db_path_clone.clone();
                        let id_p = id_clone.clone();
                        tokio::task::spawn_blocking(move || {
                            let _ = crate::db::log_event(&db_p, &id_p, "startup_slow", Some(detail.as_str()));
                        });
                        startup_timeout_logged = true;
                     }

                     if !startup_first_byte_logged {
                        if let Some(first_byte_at) = startup_first_byte_at {
                            let first_verified_lag_ms = startup_first_network_at
                                .map(|d| first_byte_at.saturating_sub(d).as_millis())
                                .unwrap_or(0);
                            let detail = format!(
                                "resume={}, reason=first_byte, first_byte_ms={}, first_network_ms={}, first_verified_lag_ms={}, metadata_ms={}, live_ms={}, peers_ms={}, initial_peers={}",
                                is_resume,
                                first_byte_at.as_millis(),
                                startup_first_network_at.map(|d| d.as_millis()).unwrap_or(0),
                                first_verified_lag_ms,
                                startup_metadata_at.map(|d| d.as_millis()).unwrap_or(0),
                                startup_live_at.map(|d| d.as_millis()).unwrap_or(0),
                                startup_peers_at.map(|d| d.as_millis()).unwrap_or(0),
                                initial_peers_count,
                            );
                            println!("[Torrent][Startup][{}] {}", id_clone, detail);
                            let db_p = db_path_clone.clone();
                            let id_p = id_clone.clone();
                            tokio::task::spawn_blocking(move || {
                                let _ = crate::db::log_event(&db_p, &id_p, "startup_profile", Some(detail.as_str()));
                            });
                            startup_first_byte_logged = true;
                        }
                     }

                     if stats.progress_bytes > last_progress_seen {
                         last_progress_seen = stats.progress_bytes;
                         stalled_since = None;
                         live_stalled_since = None;
                     } else if stats.live.is_none() && !is_cached_paused {
                         stalled_since.get_or_insert(now);
                         live_stalled_since = None;
                     } else if stats.live.is_some() && connections > 0 && !is_cached_paused {
                         stalled_since = None;
                         live_stalled_since.get_or_insert(now);
                     } else {
                         stalled_since = None;
                         live_stalled_since = None;
                     }

                     if stats.live.is_none() && !is_cached_paused {
                         if let Some(stalled_at) = stalled_since {
                             let stalled_for = now.duration_since(stalled_at);
                             let can_poke = last_recovery_poke
                                 .map(|t| now.duration_since(t) >= std::time::Duration::from_secs(12))
                                 .unwrap_or(true);

                             if stalled_for >= std::time::Duration::from_secs(20) && can_poke {
                                 if let Err(e) = session_for_monitor.unpause(&handle).await {
                                     let msg = e.to_string();
                                     if !msg.contains("not paused")
                                         && !msg.contains("already running")
                                         && !msg.contains("already live")
                                     {
                                         eprintln!("[Torrent] Recovery unpause failed for {}: {}", id_clone, msg);
                                     }
                                 }
                                 last_recovery_poke = Some(now);
                             }
                         }
                     } else {
                         last_recovery_poke = None;
                     }

                     // Emit progress only if NOT finished, to prevent race with completion event
                     let (status_text, phase_next): (Option<String>, &'static str) = if stats.total_bytes == 0 { 
                        (Some(format!("Fetching Metadata... ({} peers)", connections)), "fetching_metadata")
                    } else if is_cached_paused {
                         paused_counter = 50;
                         was_live = false;
                         // Reset speed baselines while paused so resumption starts fresh
                         last_downloaded = stats.progress_bytes;
                         last_time = now;
                         (Some("Paused".to_string()), "paused")
                    } else if stats.live.is_none() {
                         paused_counter = paused_counter.saturating_add(1);

                         // Reset speed baselines while non-live so resume starts fresh
                         last_downloaded = stats.progress_bytes;
                         last_time = now;
                         
                         if stats.total_bytes > 0
                             && stats.progress_bytes > 0
                             && stats.progress_bytes < stats.total_bytes
                         {
                             let stalled_for = stalled_since
                                 .map(|t| now.duration_since(t))
                                 .unwrap_or_default();

                             if stalled_for >= std::time::Duration::from_secs(20) {
                                 (Some("Finding peers...".to_string()), "finding_peers")
                             } else {
                                 let pct = (stats.progress_bytes as f64 / stats.total_bytes as f64) * 100.0;
                                 (Some(format!("Verifying local data... {:.1}%", pct.min(100.0))), "verifying_data")
                             }
                         } else if stats.total_bytes > 0 && stats.progress_bytes >= stats.total_bytes {
                             (Some("Finding peers...".to_string()), "finding_peers")
                         } else if is_resume {
                             (Some("Resuming...".to_string()), "restoring_session")
                         } else {
                             (Some("Initializing...".to_string()), "initializing")
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
                                 (Some("Connecting...".to_string()), "connecting")
                             } else {
                                 let negotiating_for = live_stalled_since
                                     .map(|t| now.duration_since(t))
                                     .unwrap_or_default();
                                 if negotiating_for >= std::time::Duration::from_secs(20) {
                                     (
                                         Some(format!(
                                             "Negotiating peers... ({} peers, slow swarm)",
                                             connections
                                         )),
                                         "negotiating_peers",
                                     )
                                 } else {
                                     (
                                         Some(format!("Negotiating peers... ({} peers)", connections)),
                                         "negotiating_peers",
                                     )
                                 }
                             }
                        } else if startup_first_byte_at.is_none() {
                            (
                                Some(format!(
                                    "Receiving chunks... waiting for verification ({} peers)",
                                    connections
                                )),
                                "preparing_first_piece",
                            )
                        } else {
                            (Some(format!("Downloading ({} peers)", connections)), "downloading")
                        }
                    };

                    if phase_key != phase_next {
                        if startup_elapsed <= std::time::Duration::from_secs(30) {
                            println!(
                                "[Torrent][Phase][{}] {} -> {} at {}ms peers={} speed={}Bps rx={} verified={}",
                                id_clone,
                                phase_key,
                                phase_next,
                                startup_elapsed.as_millis(),
                                connections,
                                speed_u64,
                                network_received,
                                stats.progress_bytes
                            );
                        }
                        phase_key = phase_next.to_string();
                        phase_started_at = now;
                    }
                    let phase_elapsed_secs = now.duration_since(phase_started_at).as_secs();

                    let _ = app.emit("download-progress", serde_json::json!({
                        "id": id_clone,
                        "total": stats.total_bytes,
                        "downloaded": stats.progress_bytes,
                        "network_received": network_received,
                        "verified_speed": verified_speed_u64,
                        "speed": speed_u64,
                        "eta": eta,
                        "connections": connections,
                        "status_text": status_text,
                        "status_phase": phase_key,
                        "phase_elapsed_secs": phase_elapsed_secs,
                    }));
                }

                if (stats.finished || (stats.total_bytes > 0 && stats.progress_bytes >= stats.total_bytes)) && !completion_handled {
                    let file_entries_for_cleanup = if selected_indices_for_cleanup.is_some() {
                        handle
                            .with_metadata(|m| {
                                m.file_infos
                                    .iter()
                                    .enumerate()
                                    .map(|(idx, f)| {
                                        (idx, f.relative_filename.to_string_lossy().to_string())
                                    })
                                    .collect::<Vec<(usize, String)>>()
                            })
                            .ok()
                    } else {
                        None
                    };

                    // 1. Update status to Completed in DB (Block until done to prevent race with frontend)
                    let db_p = db_path_clone.clone();
                    let id_p = id_clone.clone();
                    let total_bytes_final = stats.total_bytes; // Capture explicit current size
                    let _ = tokio::task::spawn_blocking(move || {
                        if let Err(e) = crate::db::mark_download_completed(&db_p, &id_p) {
                            eprintln!("CRITICAL DB ERROR: Failed to mark as completed: {}", e);
                        }

                        // Also ensure progress is capped at 100%
                        let _ = crate::db::update_download_progress(
                            &db_p,
                            &id_p,
                            total_bytes_final as i64,
                            0,
                        );
                    }).await;

                    // 2. Emit completion event only AFTER DB is updated
                    let _ = app.emit("download-completed", id_clone.clone());

                    // 3. Remove the torrent from in-memory/session state to release file handles.
                    {
                        let mut active = active_torrents.lock().await;
                        active.remove(&id_clone);
                    }
                    {
                        let mut paused = paused_downloads.lock().await;
                        paused.remove(&id_clone);
                    }
                    let info_hash = handle.info_hash();
                    if let Err(e) = session_for_monitor
                        .delete(librqbit::api::TorrentIdOrHash::Hash(info_hash), false)
                        .await
                    {
                        eprintln!("[Torrent] Failed to remove completed torrent {} from session: {}", id_clone, e);
                    }

                    // 4. Remove unselected placeholders after handle release.
                    if let (Some(selected_indices), Some(file_entries)) = (
                        selected_indices_for_cleanup.as_ref(),
                        file_entries_for_cleanup.as_ref(),
                    ) {
                        for attempt in 0..8 {
                            Self::cleanup_unselected_placeholder_files(
                                &output_folder_clone,
                                selected_indices,
                                file_entries,
                            );
                            let selected: HashSet<usize> = selected_indices.iter().copied().collect();
                            let has_remaining_unselected = file_entries.iter().any(|(idx, relative_path)| {
                                if selected.contains(idx) {
                                    return false;
                                }
                                Path::new(&output_folder_clone).join(relative_path).exists()
                            });
                            if !has_remaining_unselected {
                                break;
                            }
                            if attempt < 7 {
                                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                            }
                        }
                    }
                    
                    // completion_handled = true; // Unused as we break immediately
                    
                    // 5. Post-Download Actions
                    // We need the full Download record to know the filepath
                    if let Ok(downloads) = crate::db::get_all_downloads(&db_path_clone) {
                        if let Some(download) = downloads.into_iter().find(|d| d.id == id_clone) {
                            crate::commands::execute_post_download_actions(app.clone(), db_path_clone.clone(), download).await;
                        }
                    }
                    break;
                }
                
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            }
        });

        Ok(())
    }

    /// Metadata Sniffer: Briefly joins a swarm to extract the file tree and total size.
    pub async fn analyze_magnet(&self, magnet: String) -> Result<TorrentInfo, String> {
        let session_guard = self.session.lock().await;
        let session = session_guard.as_ref().ok_or("Torrent session is not ready.")?.clone();
        drop(session_guard); // Release early to allow other calls

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

                (
                    TorrentInfo {
                        id: None,
                        name: m.name.clone().unwrap_or_default(),
                        total_size: m.file_infos.iter().map(|f| f.len).sum(),
                        files,
                    },
                    m.torrent_bytes.to_vec(),
                )
            });

            match result {
                Ok((mut info, torrent_bytes)) => {
                    let analysis_id = uuid::Uuid::new_v4().to_string();
                    info.id = Some(analysis_id.clone());
                    self.analyzed_torrents
                        .lock()
                        .await
                        .insert(analysis_id, torrent_bytes);

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
        let session_guard = self.session.lock().await;
        let session = session_guard.as_ref().ok_or("Torrent session is not yet initialized")?;
        
        let active = self.active_torrents.lock().await;
        if let Some(handle) = active.get(id) {
             session.unpause(handle).await.map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// Pauses an active torrent in the `librqbit` session.
    pub async fn pause_torrent(&self, id: &str) -> Result<(), String> {
        let session_guard = self.session.lock().await;
        let session = session_guard.as_ref().ok_or("Torrent session is not active")?;
        
        let active = self.active_torrents.lock().await;
        if let Some(handle) = active.get(id) {
            {
                let mut paused = self.paused_downloads.lock().await;
                paused.insert(id.to_string());
            }
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
    pub async fn resume_torrent(&self, id: &str) -> Result<(), String> {
        let session_guard = self.session.lock().await;
        let session = session_guard.as_ref().ok_or("Torrent session is not active")?;
        
        let active = self.active_torrents.lock().await;
        if let Some(handle) = active.get(id) {
            {
                let mut paused = self.paused_downloads.lock().await;
                paused.remove(id);
            }
            match session.unpause(handle).await {
                Ok(_) => {},
                Err(e) => {
                    let msg = e.to_string();
                    // If it's already running, we consider that a success
                    if !msg.contains("not paused")
                        && !msg.contains("already running")
                        && !msg.contains("already live")
                    {
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
        let session_guard = self.session.lock().await;
        let session = session_guard.as_ref().ok_or("Torrent session is not active")?;
        
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
        let session_guard = self.session.lock().await;
        let session = session_guard.as_ref().ok_or("Torrent session is not active")?;
        
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
