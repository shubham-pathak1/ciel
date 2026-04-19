pub mod http;
pub mod torrent;

pub use http::{add_download, validate_url_type, DownloadManager, UrlTypeInfo};
pub use torrent::{add_torrent, analyze_torrent, run_torrent_diagnostics, start_selective_torrent};

use crate::db::{self, DbState, Download, DownloadProtocol, DownloadStatus};
use crate::torrent::TorrentManager;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter, Manager, Runtime, State};

use self::torrent::parse_optional_torrent_indices_metadata;

/// Resolves a human-provided path into a valid, absolute filesystem path.
///
/// It handles:
/// - Absolute vs Relative paths.
/// - System-specific "Downloads" folder fallback.
/// - Custom user-defined download directories.
pub(crate) fn resolve_download_path<R: Runtime>(
    app: &tauri::AppHandle<R>,
    db_path: &str,
    provided_path: &str,
    override_folder: Option<String>,
) -> String {
    let p = Path::new(provided_path);
    if p.is_absolute() {
        return provided_path.to_string();
    }

    let base_dir = if let Some(folder) = override_folder {
        PathBuf::from(folder)
    } else {
        // Get configured download path
        let configured_path = db::get_setting(db_path, "download_path")
            .unwrap_or(None)
            .unwrap_or_default();

        if !configured_path.is_empty() {
            let path = PathBuf::from(&configured_path);
            if path.is_absolute() {
                path
            } else {
                app.path()
                    .download_dir()
                    .unwrap_or_else(|_| PathBuf::from("."))
                    .join(path)
            }
        } else {
            // Fallback to system's downloads folder via Tauri
            app.path()
                .download_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join("Ciel Downloads")
        }
    };

    // --- START AUTO-ORGANIZE LOGIC ---
    let auto_organize = db::get_setting(db_path, "auto_organize")
        .unwrap_or(None)
        .map(|v| v == "true")
        .unwrap_or(false);

    let base_dir = if auto_organize {
        let category =
            get_category_from_filename(p.file_name().unwrap_or_default().to_str().unwrap_or_default());
        if category != "Other" {
            base_dir.join(category)
        } else {
            base_dir
        }
    } else {
        base_dir
    };
    // --- END AUTO-ORGANIZE LOGIC ---

    // Ensure base directory exists
    let _ = std::fs::create_dir_all(&base_dir);

    // If provided path is simply a filename or relative like ./file
    let file_name = p.file_name().unwrap_or_default();
    let final_path = base_dir.join(file_name);

    // Ensure the path is absolute for external tool reliability (Explorer, etc)
    let absolute_path = if final_path.is_absolute() {
        final_path
    } else {
        app.path()
            .download_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(final_path)
    };

    absolute_path.to_string_lossy().to_string()
}

/// Prevents file overwriting by appending a numeric suffix (e.g., "file (1).txt")
/// if a collision is detected on the disk OR in the database.
pub(crate) fn ensure_unique_path(db_path: &str, path_str: String) -> String {
    let path = Path::new(&path_str);

    // Check if it exists on disk OR in the DB
    let exists_in_db = crate::db::check_filepath_exists(db_path, &path_str).unwrap_or(false);

    if !path.exists() && !exists_in_db {
        return path_str;
    }

    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let extension = path.extension().unwrap_or_default().to_string_lossy();
    let parent = path.parent().unwrap_or_else(|| Path::new(""));

    let mut counter = 1;
    loop {
        let new_filename = if extension.is_empty() {
            format!("{} ({})", stem, counter)
        } else {
            format!("{} ({}).{}", stem, counter, extension)
        };
        let new_path = parent.join(new_filename);
        let new_path_str = new_path.to_string_lossy().to_string();

        let exists_in_db = crate::db::check_filepath_exists(db_path, &new_path_str).unwrap_or(false);

        if !new_path.exists() && !exists_in_db {
            return new_path_str;
        }
        counter += 1;
    }
}

/// Map file extensions to broad categories for UI filtering.
pub fn get_category_from_filename(filename: &str) -> String {
    let path = Path::new(filename);
    let extension = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();

    match extension.as_str() {
        "mp4" | "mkv" | "avi" | "mov" | "webm" | "flv" | "wmv" | "m4v" => "Video".to_string(),
        "mp3" | "wav" | "flac" | "aac" | "ogg" | "m4a" | "wma" => "Audio".to_string(),
        "zip" | "rar" | "7z" | "tar" | "gz" | "bz2" | "iso" => "Compressed".to_string(),
        "exe" | "msi" | "app" | "dmg" | "deb" | "rpm" => "Software".to_string(),
        "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "txt" | "rtf" | "epub" => {
            "Documents".to_string()
        }
        _ => "Other".to_string(),
    }
}

fn emit_download_error_event<R: Runtime>(app: &AppHandle<R>, id: &str, message: &str) {
    let _ = app.emit(
        "download-error",
        serde_json::json!({
            "id": id,
            "message": message
        }),
    );
}

pub(crate) fn set_and_emit_download_error<R: Runtime>(
    app: &AppHandle<R>,
    db_path: &str,
    id: &str,
    message: &str,
) {
    let _ = db::update_download_error(db_path, id, message);
    emit_download_error_event(app, id, message);
}

/// Triggers post-transfer logic like opening the target folder or system power management.
///
/// This is called automatically when a download transitions to the 'Completed' status.
pub(crate) async fn execute_post_download_actions<R: Runtime>(
    app: AppHandle<R>,
    db_path: String,
    download: Download,
) {
    // 1. Open Folder on Finish
    let open_folder = db::get_setting(&db_path, "open_folder_on_finish")
        .ok()
        .flatten()
        .map(|v| v == "true")
        .unwrap_or(false);

    if open_folder {
        // Use the internal helper that doesn't require State
        let _ = show_in_folder_internal(app, &db_path, download.filepath.clone());
    }

    // 2. Sound notification (handled by frontend or system toast by default, but we can add more if needed)

    // 3. Shutdown on Finish
    let shutdown_enabled = db::get_setting(&db_path, "shutdown_on_finish")
        .ok()
        .flatten()
        .map(|v| v == "true")
        .unwrap_or(false);

    if shutdown_enabled {
        // Check if there are ANY other active downloads
        if let Ok(downloads) = db::get_all_downloads(&db_path) {
            let active_count = downloads
                .iter()
                .filter(|d| d.id != download.id) // Exclude current one as it might still be in 'Downloading' status during this call
                .filter(|d| d.status == db::DownloadStatus::Downloading)
                .count();

            if active_count == 0 {
                // All downloads finished! Trigger shutdown.
                #[cfg(target_os = "windows")]
                {
                    let _ = std::process::Command::new("shutdown")
                        .args(&["/s", "/t", "60", "/c", "Ciel: All downloads finished. Shutting down in 60s."])
                        .spawn();
                }
                #[cfg(target_os = "linux")]
                {
                    let _ = std::process::Command::new("shutdown").args(&["+1"]).spawn();
                }
                #[cfg(target_os = "macos")]
                {
                    let _ = std::process::Command::new("shutdown")
                        .args(&["-h", "+1"])
                        .spawn();
                }
            }
        }
    }
}

/// Bridge: Fetches the full list of downloads for the Frontend.
#[tauri::command]
pub fn get_downloads(db_state: State<DbState>) -> Result<Vec<Download>, String> {
    db::get_all_downloads(&db_state.path).map_err(|e| e.to_string())
}

/// Bridge: Pauses an active transfer.
///
/// For HTTP, it signals the worker to stop. For Torrents, it communicates
/// directly with the `librqbit` session.
#[tauri::command]
pub async fn pause_download<R: Runtime>(
    app: AppHandle<R>,
    db_state: State<'_, DbState>,
    manager: State<'_, DownloadManager>,
    torrent_manager: State<'_, TorrentManager>,
    id: String,
) -> Result<(), String> {
    let downloads = db::get_all_downloads(&db_state.path).map_err(|e| e.to_string())?;
    let download = downloads.iter().find(|d| d.id == id).ok_or("Download not found")?;

    if download.protocol == DownloadProtocol::Torrent {
        torrent_manager.pause_torrent(&id).await?;
    } else {
        manager.cancel(&id).await;
    }

    db::log_event(&db_state.path, &id, "paused", None).ok();
    db::update_download_status(&db_state.path, &id, DownloadStatus::Paused).map_err(|e| e.to_string())?;

    // Immediate UI Feedback
    // We construct a partial object that the frontend will merge/handle
    // The frontend mainly looks at 'status_text' for logic overrides we added
    let _ = app.emit("download-progress", serde_json::json!({
        "id": id,
        "total": download.size,
        "downloaded": download.downloaded,
        "network_received": download.downloaded,
        "verified_speed": 0u64,
        "speed": 0,
        "eta": 0,
        "connections": 0,
        "status_text": "Paused",
        "status_phase": "paused",
        "phase_elapsed_secs": 0,
    }));

    Ok(())
}

/// Bridge: Resumes a previously paused transfer.
#[tauri::command]
pub async fn resume_download<R: Runtime>(
    app: AppHandle<R>,
    db_state: State<'_, DbState>,
    manager: State<'_, DownloadManager>,
    torrent_manager: State<'_, TorrentManager>,
    id: String,
) -> Result<(), String> {
    let downloads = db::get_all_downloads(&db_state.path).map_err(|e| e.to_string())?;
    let mut download = downloads
        .into_iter()
        .find(|d| d.id == id)
        .ok_or("Download not found")?
        .clone();

    // Update connections from settings
    let max_connections = db::get_setting(&db_state.path, "max_connections")
        .ok()
        .flatten()
        .and_then(|v| v.parse::<i32>().ok())
        .unwrap_or(16);

    download.connections = max_connections;

    if download.status == DownloadStatus::Completed {
        return Err("Download already completed".to_string());
    }

    // Idempotency guard for HTTP downloads:
    // if already active in memory, do not start another worker task.
    if download.protocol == DownloadProtocol::Http && manager.is_active(&id).await {
        // Keep DB state consistent in case it's stale.
        if download.status != DownloadStatus::Downloading {
            db::update_download_status(&db_state.path, &id, DownloadStatus::Downloading)
                .map_err(|e| e.to_string())?;
        }
        return Ok(());
    }

    db::update_download_status(&db_state.path, &id, DownloadStatus::Downloading).map_err(|e| e.to_string())?;
    db::log_event(&db_state.path, &id, "resumed", None).ok();

    match download.protocol {
        DownloadProtocol::Torrent => {
            let resume_watch_id = id.clone();
            let resume_watch_app = app.clone();
            let resume_watch_manager = torrent_manager.inner().clone();
            let resume_watch_baseline = download.downloaded.max(0) as u64;
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(20)).await;
                if let Some(snapshot) = resume_watch_manager
                    .get_stats_snapshot(&resume_watch_id)
                    .await
                {
                    if !snapshot.is_live
                        && snapshot.live_peers == 0
                        && snapshot.progress_bytes <= resume_watch_baseline
                    {
                        println!(
                            "[Torrent][Resume][{}] watchdog_stuck progress={} peers={} live={}",
                            resume_watch_id,
                            snapshot.progress_bytes,
                            snapshot.live_peers,
                            snapshot.is_live
                        );
                        let _ = resume_watch_app.emit(
                            "download-progress",
                            serde_json::json!({
                                "id": resume_watch_id,
                                "status_text": "Restoring session... (retrying)",
                                "status_phase": "restoring_session",
                                "phase_elapsed_secs": 20u64,
                            }),
                        );
                        let _ = resume_watch_manager.resume_torrent(&resume_watch_id).await;
                    }
                }
            });

            println!(
                "[Torrent][Resume][{}] requested status_before={} downloaded={} total={} info_hash={} meta_len={} url_len={}",
                id,
                download.status.as_str(),
                download.downloaded.max(0),
                download.size.max(0),
                download.info_hash.clone().unwrap_or_default(),
                download.metadata.as_ref().map(|m| m.len()).unwrap_or(0),
                download.url.len()
            );
            let _ = app.emit("download-progress", serde_json::json!({
                "id": id,
                "total": download.size.max(0) as u64,
                "downloaded": download.downloaded.max(0) as u64,
                "network_received": download.downloaded.max(0) as u64,
                "verified_speed": 0u64,
                "speed": 0u64,
                "eta": 0u64,
                "connections": 0u64,
                "status_text": "Restoring session...",
                "status_phase": "restoring_session",
                "phase_elapsed_secs": 0u64,
            }));

            if !torrent_manager.wait_until_ready(30000).await {
                println!("[Torrent][Resume][{}] engine_not_ready timeout_ms=30000", id);
                let msg = "Torrent engine is still initializing. Please wait a moment and try again.".to_string();
                set_and_emit_download_error(&app, &db_state.path, &id, &msg);
                return Err(msg);
            }

            // Try to resume existing in-memory session first.
            let in_memory_active = torrent_manager.is_active(&id).await;
            if in_memory_active {
                println!("[Torrent][Resume][{}] path=in_memory_handle", id);
                if let Err(e) = torrent_manager.resume_torrent(&id).await {
                    let msg = format!("Failed to resume torrent: {}", e);
                    set_and_emit_download_error(&app, &db_state.path, &id, &msg);
                    return Err(msg);
                }
            } else {
                // If it wasn't in the active session map (e.g. app restart), re-add it.
                // It will automatically verify existing files and resume.
                let output_folder = std::path::Path::new(&download.filepath)
                    .parent()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default();

                let indices = match parse_optional_torrent_indices_metadata(&download.metadata) {
                    Ok(v) => v,
                    Err(msg) => {
                        set_and_emit_download_error(&app, &db_state.path, &id, &msg);
                        return Err(msg);
                    }
                };
                println!(
                    "[Torrent][Resume][{}] path=readd_to_session selected_files={} output_folder={}",
                    id,
                    indices.as_ref().map(|v| v.len()).unwrap_or(0),
                    output_folder
                );

                if let Err(e) = torrent_manager
                    .add_magnet(
                        app.clone(),
                        id.clone(),
                        download.url,
                        output_folder,
                        db_state.path.clone(),
                        indices,
                        download.size as u64,
                        download.downloaded.max(0) as u64,
                        true,
                        false,
                        None,
                    )
                    .await
                {
                    let msg = format!("Failed to resume torrent: {}", e);
                    set_and_emit_download_error(&app, &db_state.path, &id, &msg);
                    return Err(msg);
                }
            }
        }
        _ => {
            let known_single_connection = download.metadata.as_deref() == Some("http_no_range");
            let _ = app.emit("download-progress", serde_json::json!({
                "id": id,
                "total": download.size.max(0) as u64,
                "downloaded": if known_single_connection { 0u64 } else { download.downloaded.max(0) as u64 },
                "network_received": if known_single_connection { 0u64 } else { download.downloaded.max(0) as u64 },
                "verified_speed": 0u64,
                "speed": 0u64,
                "eta": 0u64,
                "connections": if known_single_connection { 1u64 } else { 0u64 },
                "status_text": if known_single_connection { "Restarting..." } else { "Resuming..." },
                "status_phase": if known_single_connection { "restarting" } else { "resuming" },
                "phase_elapsed_secs": 0u64,
            }));
            http::start_download_task(
                app,
                db_state.path.clone(),
                manager.inner().clone(),
                download.clone(),
            )
            .await?;
        }
    }

    Ok(())
}

/// Bridge: Fetches only the completed downloads for the History view.
#[tauri::command]
pub async fn get_history(db_state: State<'_, DbState>) -> Result<Vec<Download>, String> {
    db::get_history(&db_state.path).map_err(|e| e.to_string())
}

/// Bridge: Retrieves the event log (history) for a specific download.
#[tauri::command]
pub async fn get_download_events(
    db_state: State<'_, DbState>,
    id: String,
) -> Result<Vec<(String, String, Option<String>)>, String> {
    db::get_download_events(&db_state.path, &id).map_err(|e| e.to_string())
}

/// Bridge: Permanently removes a download from the registry and aborts it if active.
#[tauri::command]
pub async fn delete_download(
    db_state: State<'_, DbState>,
    manager: State<'_, DownloadManager>,
    torrent_manager: State<'_, TorrentManager>,
    id: String,
    delete_files: bool,
) -> Result<(), String> {
    // 1. Get record first to know protocol and hash
    let downloads = db::get_all_downloads(&db_state.path).map_err(|e| e.to_string())?;
    let download_opt = downloads.into_iter().find(|d| d.id == id);

    if let Some(download) = download_opt {
        // 2. Clear from DB FIRST to ensure it doesn't "ghost" back into the UI.
        // This makes the deletion feel instant to the user.
        db::delete_download_by_id(&db_state.path, &id).map_err(|e| {
            eprintln!("Failed to delete DB record for {}: {}", id, e);
            e.to_string()
        })?;

        // 3. Cleanup Engine (Fire-and-forget in a background task)
        // This prevents hangs in the engine (e.g. searching for missing files) from blocking the UI.
        let tm = torrent_manager.inner().clone();
        let m = manager.inner().clone();

        tokio::spawn(async move {
            if download.protocol == DownloadProtocol::Torrent {
                let _ = tm
                    .delete_torrent(&id, delete_files, Some(download.filepath.clone()))
                    .await;
                if let Some(hash) = download.info_hash {
                    let _ = tm.delete_torrent_by_hash(hash, delete_files).await;
                } else if let Some(hash) = TorrentManager::extract_info_hash_from_magnet(&download.url) {
                    let _ = tm.delete_torrent_by_hash(hash, delete_files).await;
                }
            } else {
                m.cancel(&id).await;
                if delete_files {
                    // Slight delay to ensure Downloader has flushed and closed the file handle
                    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                    let _ = std::fs::remove_file(&download.filepath);
                }
            }
        });
    } else {
        // No DB record found, attempt blind engine purge
        let tm = torrent_manager.inner().clone();
        tokio::spawn(async move {
            let _ = tm.delete_torrent(&id, delete_files, None).await;
        });
    }

    Ok(())
}

/// Bridge: Fetches the entire configuration map.
#[tauri::command]
pub fn get_settings(db_state: State<DbState>) -> Result<HashMap<String, String>, String> {
    db::get_all_settings(&db_state.path).map_err(|e| e.to_string())
}

/// Bridge: Updates a specific configuration key.
#[tauri::command]
pub fn update_setting(db_state: State<DbState>, key: String, value: String) -> Result<(), String> {
    db::set_setting(&db_state.path, &key, &value).map_err(|e| e.to_string())
}

/// Bridge: Opens the OS file explorer and focuses the downloaded file/folder.
#[tauri::command]
pub fn show_in_folder<R: Runtime>(
    app: AppHandle<R>,
    db_state: State<'_, DbState>,
    path: String,
) -> Result<(), String> {
    show_in_folder_internal(app, &db_state.path, path)
}

/// Internal wrapper for "Show in Folder" that handles cross-platform logic.
///
/// - Windows: Uses `explorer.exe /select` to highlight the file.
/// - MacOS: Uses `open -R`.
/// - Linux: Uses `xdg-open` on the parent folder.
pub fn show_in_folder_internal<R: Runtime>(
    app: AppHandle<R>,
    db_path: &str,
    path: String,
) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let path_norm = path.replace("/", "\\");
        let mut p_buf = PathBuf::from(&path_norm);

        // Ensure path is absolute
        if !p_buf.is_absolute() {
            // Try to get configured download path first
            let configured_path = db::get_setting(db_path, "download_path")
                .ok()
                .flatten()
                .filter(|p| !p.is_empty())
                .map(PathBuf::from);

            let base = configured_path.unwrap_or_else(|| {
                app.path()
                    .download_dir()
                    .unwrap_or_else(|_| PathBuf::from("."))
            });

            p_buf = base.join(p_buf);
        }

        let p = p_buf.as_path();

        if p.exists() {
            if p.is_dir() {
                // If it's a directory, just open it
                let _ = std::process::Command::new("explorer.exe").arg(p).spawn();
            } else {
                // If it's a file, select it in its parent folder
                // We use lossy string conversion to handle potential unicode issues safely
                let path_str = p.to_string_lossy().to_string();
                let _ = std::process::Command::new("explorer.exe")
                    .arg("/select,")
                    .arg(path_str)
                    .spawn();
            }
        } else {
            // If the specific file doesn't exist (e.g. download in progress),
            // try opening its parent directory
            if let Some(parent) = p.parent() {
                if parent.exists() {
                    let _ = std::process::Command::new("explorer.exe").arg(parent).spawn();
                }
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        let p = Path::new(&path);
        if p.exists() {
            std::process::Command::new("open")
                .args(["-R", &path])
                .spawn()
                .map_err(|e| e.to_string())?;
        } else if let Some(parent) = p.parent() {
            std::process::Command::new("open")
                .arg(parent)
                .spawn()
                .map_err(|e| e.to_string())?;
        }
    }

    #[cfg(target_os = "linux")]
    {
        let p = Path::new(&path);
        let folder = if p.is_dir() {
            p
        } else {
            p.parent().unwrap_or_else(|| Path::new("/"))
        };

        std::process::Command::new("xdg-open")
            .arg(folder)
            .spawn()
            .map_err(|e| e.to_string())?;
    }

    Ok(())
}

/// Clear finished downloads
#[tauri::command]
pub fn clear_finished(db_state: State<DbState>) -> Result<(), String> {
    db::delete_finished_downloads(&db_state.path).map_err(|e| e.to_string())
}

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
                eprintln!("Failed to fetch queued download: {}", e);
                break;
            }
        };

        // 3. Start Download
        let id = next_download.id.clone();
        println!("Queue Processor: Starting {}", next_download.filename);

        // Update status first to prevent race conditions (double starting)
        if let Err(e) = db::update_download_status(&db_state.path, &id, DownloadStatus::Downloading) {
            eprintln!("Failed to update status for {}: {}", id, e);
            continue;
        }

        db::log_event(&db_state.path, &id, "started", Some("Auto-started from queue")).ok();
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
                    eprintln!("Failed to start queued HTTP download {}: {}", id, e);
                    set_and_emit_download_error(&app, &db_state.path, &id, &e);
                }
            }
            DownloadProtocol::Torrent => {
                let path = Path::new(&next_download.filepath);
                let base_folder = path.parent().unwrap_or(Path::new(".")).to_string_lossy().to_string();

                let indices = match parse_optional_torrent_indices_metadata(&next_download.metadata) {
                    Ok(v) => v,
                    Err(msg) => {
                        set_and_emit_download_error(&app, &db_state.path, &id, &msg);
                        continue;
                    }
                };

                if !torrent_manager.wait_until_ready(30000).await {
                    eprintln!("Queue Processor: torrent engine still initializing; will retry {}", id);
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
                    eprintln!("Failed to start queued torrent {}: {}", id, e);
                    set_and_emit_download_error(&app, &db_state.path, &id, &e);
                }
            }
            DownloadProtocol::Video => {
                // TODO: Implement video download queuing when video support is fully added
                eprintln!("Video queuing not yet supported for {}", id);
                let _ = db::update_download_status(&db_state.path, &id, DownloadStatus::Error);
            }
        }
    }
}
