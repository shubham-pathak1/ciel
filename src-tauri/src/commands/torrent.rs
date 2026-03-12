use crate::db::{self, DbState, Download, DownloadProtocol, DownloadStatus};
use crate::torrent::TorrentManager;
use super::{DownloadManager, ensure_unique_path, resolve_download_path, set_and_emit_download_error};
use std::path::Path;
use tauri::{AppHandle, Emitter, Runtime, State};

fn serialize_torrent_indices_metadata(indices: &Option<Vec<usize>>) -> Option<String> {
    indices
        .as_ref()
        .and_then(|idxs| serde_json::to_string(&serde_json::json!({ "indices": idxs })).ok())
}

fn parse_torrent_indices_metadata(metadata: &str) -> Option<Vec<usize>> {
    // Legacy format support: raw JSON array like [0,1,2]
    if let Ok(v) = serde_json::from_str::<Vec<usize>>(metadata) {
        return Some(v);
    }

    // Current canonical format: {"indices":[...]}
    let json = serde_json::from_str::<serde_json::Value>(metadata).ok()?;
    let arr = json.get("indices").and_then(|v| v.as_array())?;
    Some(
        arr.iter()
            .filter_map(|v| v.as_u64().map(|n| n as usize))
            .collect(),
    )
}

pub(crate) fn parse_optional_torrent_indices_metadata(
    metadata: &Option<String>,
) -> Result<Option<Vec<usize>>, String> {
    match metadata {
        None => Ok(None),
        Some(raw) => {
            let parsed = parse_torrent_indices_metadata(raw)
                .ok_or_else(|| "Torrent selection metadata is invalid. Please re-add this torrent.".to_string())?;
            if parsed.is_empty() {
                return Err("No files are selected for this torrent task.".to_string());
            }
            Ok(Some(parsed))
        }
    }
}

/// Bridge: Initiates a new BitTorrent download (Magnet or .torrent file).
///
/// This command handles:
/// - Metadata extraction from magnet query parameters.
/// - Duplicate isolation: If a torrent with the same name exists, it creates
///   a dedicated sub-folder to prevent file/hash collisions.
/// - Registration with the `TorrentManager`.
#[tauri::command]
pub async fn add_torrent<R: Runtime>(
    app: AppHandle<R>,
    db_state: State<'_, DbState>,
    manager: State<'_, DownloadManager>,
    torrent_manager: State<'_, TorrentManager>,
    url: String, // Magnet link or local file path
    mut filename: String,
    _filepath: String,
    output_folder: Option<String>,
    indices: Option<Vec<usize>>,
    analysis_id: Option<String>,
    total_size: Option<u64>,
    start_paused: Option<bool>,
) -> Result<Download, String> {
    let is_magnet = url.starts_with("magnet:");

    // Attempt to extract name from magnet link "dn" parameter
    if is_magnet {
        if let Ok(parsed_url) = url::Url::parse(&url) {
            if let Some((_, name)) = parsed_url.query_pairs().find(|(k, _)| k == "dn") {
                filename = name.to_string();
            }
        }
    }

    // Finalize resolved path (Smart Duplicate Handling)
    let resolved_path = resolve_download_path(&app, &db_state.path, &filename, output_folder.clone());
    let final_resolved_path = ensure_unique_path(&db_state.path, resolved_path.clone());

    // Extract the final unique filename from the path
    let final_filename = Path::new(&final_resolved_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| filename.clone());

    // Queue enforcement: Check if we can start immediately or must queue
    let max_simultaneous = db::get_setting(&db_state.path, "max_concurrent")
        .ok()
        .flatten()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(3);

    // Count both HTTP and Torrent active downloads
    let (http_active, _) = manager.get_global_status().await;
    let (torrent_active, _) = torrent_manager.get_global_status().await;
    let active_count = http_active + torrent_active;
    let should_queue = !start_paused.unwrap_or(false) && active_count >= max_simultaneous;

    let id = uuid::Uuid::new_v4().to_string();
    let download = Download {
        id: id.clone(),
        url: url.clone(),
        filename: final_filename,
        filepath: final_resolved_path.clone(),
        size: total_size.unwrap_or(0) as i64,
        downloaded: 0,
        status: if start_paused.unwrap_or(false) {
            DownloadStatus::Paused
        } else if should_queue {
            DownloadStatus::Queued
        } else {
            DownloadStatus::Downloading
        },
        protocol: DownloadProtocol::Torrent,
        speed: 0,
        connections: 0,
        created_at: chrono::Utc::now().to_rfc3339(),
        completed_at: None,
        error_message: None,
        info_hash: None,
        metadata: serialize_torrent_indices_metadata(&indices),
        user_agent: None,
        cookies: None,
        category: "Other".to_string(),
    };

    db::insert_download(&db_state.path, &download).map_err(|e| e.to_string())?;
    db::log_event(
        &db_state.path,
        &download.id,
        "created",
        Some(if start_paused.unwrap_or(false) {
            "Torrent added (Scheduled/Paused)"
        } else if should_queue {
            "Torrent queued (concurrent limit reached)"
        } else {
            "Torrent download initiated"
        }),
    )
    .ok();

    let is_duplicate = resolved_path != final_resolved_path;

    // For torrents, base_folder must always be a DIRECTORY (not a file path)
    // librqbit will create the torrent's internal file structure inside this folder
    let base_folder = if is_duplicate {
        let path = Path::new(&final_resolved_path);
        let stem = path.file_stem().unwrap_or(std::ffi::OsStr::new("unknown"));
        let parent = path.parent().unwrap_or(Path::new("."));
        parent.join(stem).to_string_lossy().to_string()
    } else if let Some(folder) = output_folder {
        folder
    } else {
        Path::new(&final_resolved_path)
            .parent()
            .unwrap_or(Path::new("."))
            .to_string_lossy()
            .to_string()
    };

    let source_torrent_bytes = if !should_queue {
        if let Some(analysis_id) = analysis_id.as_ref() {
            torrent_manager.consume_analysis_bytes(analysis_id).await
        } else {
            None
        }
    } else {
        None
    };

    // Only start if not paused and not queued
    if !should_queue {
        let _ = app.emit("download-progress", serde_json::json!({
            "id": id,
            "total": download.size.max(0) as u64,
            "downloaded": 0u64,
            "network_received": 0u64,
            "verified_speed": 0u64,
            "speed": 0u64,
            "eta": 0u64,
            "connections": 0u64,
            "status_text": "Initializing...",
            "status_phase": "initializing",
            "phase_elapsed_secs": 0u64,
        }));

        if !torrent_manager.wait_until_ready(30000).await {
            let msg = "Torrent engine is still initializing. Please retry in a few seconds.".to_string();
            set_and_emit_download_error(&app, &db_state.path, &id, &msg);
            return Err(msg);
        }

        torrent_manager
            .add_magnet(
                app,
                id.clone(),
                url,
                base_folder,
                db_state.path.clone(),
                indices,
                download.size as u64,
                false,
                start_paused.unwrap_or(false),
                source_torrent_bytes,
            )
            .await?;
    }

    Ok(download)
}

/// Bridge: Inspects a torrent source to retrieve its file list and metadata.
///
/// This is used for "Selective Downloads" where the user chooses specific
/// files before starting the transfer.
#[tauri::command]
pub async fn analyze_torrent(
    _app: AppHandle,
    torrent_manager: State<'_, TorrentManager>,
    url: String,
) -> Result<crate::torrent::TorrentInfo, String> {
    torrent_manager.analyze_magnet(url).await
}

/// Bridge: Starts a previously analyzed torrent with a specific file selection.
#[tauri::command]
pub async fn start_selective_torrent(
    _app: AppHandle,
    torrent_manager: State<'_, TorrentManager>,
    id: String,
    indices: Vec<usize>,
) -> Result<(), String> {
    torrent_manager.start_selective(&id, indices).await
}
