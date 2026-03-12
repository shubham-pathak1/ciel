use crate::torrent::types::{TorrentFile, TorrentInfo};
use librqbit::{ManagedTorrent, Session};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::Mutex;

/// The core engine for BitTorrent downloads.
///
/// It wraps a `librqbit` session and maintains a mapping of active
/// download handles to facilitate real-time monitoring and control.
#[derive(Clone)]
pub struct TorrentManager {
    /// The underlying BitTorrent session (initialized in background).
    pub(super) session: Arc<Mutex<Option<Arc<Session>>>>,
    /// Tracks handles for active torrents, indexed by Ciel's internal UUID.
    pub(super) active_torrents: Arc<Mutex<HashMap<String, Arc<ManagedTorrent>>>>,
    /// Short-lived cache of analyzed torrent bytes keyed by analysis token.
    pub(super) analyzed_torrents: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    /// Memory cache for paused states to avoid constant DB polling.
    pub(super) paused_downloads: Arc<Mutex<HashSet<String>>>,
}

impl TorrentManager {
    pub(super) fn extract_initial_peers_from_magnet(
        magnet: &str,
    ) -> Vec<std::net::SocketAddr> {
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
                }
                Err(e) => {
                    eprintln!(
                        "Failed to start torrent session in background: {}. Torrents will be disabled.",
                        e
                    );
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

    /// Metadata Sniffer: Briefly joins a swarm to extract the file tree and total size.
    pub async fn analyze_magnet(&self, magnet: String) -> Result<TorrentInfo, String> {
        let session_guard = self.session.lock().await;
        let session = session_guard
            .as_ref()
            .ok_or("Torrent session is not ready.")?
            .clone();
        drop(session_guard); // Release early to allow other calls

        let temp_dir = std::env::temp_dir().to_string_lossy().to_string();
        let options = librqbit::AddTorrentOptions {
            output_folder: Some(temp_dir),
            only_files: Some(vec![]),
            overwrite: true,
            ..Default::default()
        };
        let response = session
            .add_torrent(librqbit::AddTorrent::from_url(magnet), Some(options))
            .await
            .map_err(|e| e.to_string())?;

        let handle = response.into_handle().ok_or("Failed to get torrent handle")?;

        // Wait for metadata (timeout after 30s)
        let start = std::time::Instant::now();
        loop {
            // Try to get metadata
            let result = handle.with_metadata(|m| {
                let files = m
                    .file_infos
                    .iter()
                    .enumerate()
                    .map(|(i, f)| TorrentFile {
                        name: f.relative_filename.to_string_lossy().to_string(),
                        size: f.len,
                        index: i,
                    })
                    .collect();

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
                    session
                        .delete(librqbit::api::TorrentIdOrHash::Hash(info_hash), false)
                        .await
                        .map_err(|e| e.to_string())?;
                    return Ok(info);
                }
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
        let session = session_guard
            .as_ref()
            .ok_or("Torrent session is not yet initialized")?;

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
                Ok(_) => {}
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
                Ok(_) => {}
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
    pub async fn delete_torrent(
        &self,
        id: &str,
        delete_files: bool,
        target_path: Option<String>,
    ) -> Result<(), String> {
        let session_guard = self.session.lock().await;
        let session = session_guard.as_ref().ok_or("Torrent session is not active")?;

        let handle_opt = {
            let mut active = self.active_torrents.lock().await;
            active.remove(id)
        };

        if let Some(handle) = handle_opt {
            let info_hash = handle.info_hash();
            // Delete standard
            let _ = session
                .delete(librqbit::api::TorrentIdOrHash::Hash(info_hash), delete_files)
                .await;

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
                let _ = session
                    .delete(librqbit::api::TorrentIdOrHash::Hash(h), delete_files)
                    .await;
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
    pub async fn delete_torrent_by_hash(
        &self,
        hash_str: String,
        delete_files: bool,
    ) -> Result<(), String> {
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
            let _ = session
                .delete(librqbit::api::TorrentIdOrHash::Hash(info_hash), delete_files)
                .await;

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
                if !still_there {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
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
