use crate::db::{self, DbState, Download, DownloadStatus, DownloadProtocol};
use crate::downloader::{Downloader, DownloadConfig};
use crate::torrent::TorrentManager;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::Mutex;
use tokio::sync::mpsc;


use tauri_plugin_notification::NotificationExt;

/// Orchestrates the lifecycle of active HTTP downloads.
/// 
/// It acts as a registry for ongoing transfers, allowing the application
/// to send cancellation signals to specific download tasks via `mpsc` channels.
#[derive(Clone)]
pub struct DownloadManager {
    /// Internal map linking Download IDs to their respective cancellation senders.
    active_downloads: Arc<Mutex<HashMap<String, mpsc::Sender<()>>>>,
}

impl DownloadManager {
    pub fn new() -> Self {
        Self {
            active_downloads: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Registers a new active download and its cancellation hook.
    pub async fn add_active(&self, id: String, cancel_tx: mpsc::Sender<()>) {
        let mut active = self.active_downloads.lock().await;
        active.insert(id, cancel_tx);
    }

    /// Unregisters a download, typically called after a successful completion or an error.
    pub async fn remove_active(&self, id: &str) {
        let mut active = self.active_downloads.lock().await;
        active.remove(id);
    }

    /// Signals an active download task to abort immediately.
    pub async fn cancel(&self, id: &str) {
        let mut active = self.active_downloads.lock().await;
        if let Some(tx) = active.get(id) {
            // Signal the async task to stop.
            let _ = tx.send(()).await;
        }
        active.remove(id);
    }

    pub async fn is_active(&self, id: &str) -> bool {
        self.active_downloads.lock().await.contains_key(id)
    }
}

// ... types for validation ...
/// Detailed metadata discovered during URL validation.
#[derive(serde::Serialize)]
pub struct UrlTypeInfo {
    /// True if the URL follows the `magnet:` protocol.
    is_magnet: bool,
    /// The MIME type reported by the server (e.g., `application/zip`).
    content_type: Option<String>,
    /// Total file size reported by the server in bytes.
    content_length: Option<u64>,
    /// A suggested filename extracted from the `Content-Disposition` header.
    hinted_filename: Option<String>,
}

/// Performs a lightweight inspection of a URL to determine its type and metadata.
/// 
/// Instead of downloading the full file, it uses HTTP `GET` with a `Range` header
/// or sniffs the first few bytes to extract headers and verify the content type.
#[tauri::command]
pub async fn validate_url_type(url: String) -> Result<UrlTypeInfo, String> {
    if url.starts_with("magnet:") {
        return Ok(UrlTypeInfo {
            is_magnet: true,
            content_type: None,
            content_length: None,
            hinted_filename: None,
        });
    }

    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        .build()
        .unwrap_or_default();

    // Use GET with Range: bytes=0-0 to get headers (including Content-Disposition) without downloading
    let response = client.get(&url)
        .header("Range", "bytes=0-0")
        .send().await
        .map_err(|e| e.to_string())?;
    
    let headers = response.headers();
    let mut content_type = headers.get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
        
    let content_length = headers.get(reqwest::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok());

    let hinted_filename = headers.get(reqwest::header::CONTENT_DISPOSITION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| {
            // First try RFC 5987 format: filename*=CHARSET''... (e.g., UTF-8, ISO-8859-1, etc.)
            // The format is: filename*=<charset>'<language>'<encoded-value>
            if let Some(re) = regex::Regex::new(r"(?i)filename\*\s*=\s*([^']+)'[^']*'([^;\s]+)").ok() {
                if let Some(caps) = re.captures(s) {
                    if let Some(m) = caps.get(2) {
                        // URL decode the filename (works for any charset, decoded as UTF-8 lossy)
                        return Some(percent_encoding::percent_decode_str(m.as_str())
                            .decode_utf8_lossy()
                            .to_string());
                    }
                }
            }
            // Fallback: try simple filename="..." or filename=...
            let re = regex::Regex::new(r#"filename\s*=\s*(?:"([^"]+)"|([^;\s]+))"#).ok()?;
            let caps = re.captures(s)?;
            caps.get(1).or(caps.get(2)).map(|m| m.as_str().to_string())
        });

    // 3. If content-type is generic or missing, try to sniff magic bytes
    if content_type.as_deref().map_or(true, |ct| ct == "application/octet-stream" || ct == "application/x-zip-compressed") {
        if let Ok(range_res) = client.get(&url).header("Range", "bytes=0-3").send().await {
            if let Ok(bytes) = range_res.bytes().await {
                if bytes.len() >= 4 && bytes[0] == 0x50 && bytes[1] == 0x4b && bytes[2] == 0x03 && bytes[3] == 0x04 {
                    content_type = Some("application/zip".to_string());
                }
            }
        }
    }

    // 4. If we have a filename but no extension, try to append one based on Content-Type
    let mut final_filename = hinted_filename;
    if let Some(ref name) = final_filename {
        if !name.contains('.') {
            if let Some(ref ct) = content_type {
                let ext = match ct.as_str() {
                    "application/zip" | "application/x-zip-compressed" => Some(".zip"),
                    "application/x-rar-compressed" | "application/vnd.rar" => Some(".rar"),
                    "application/x-7z-compressed" => Some(".7z"),
                    "video/mp4" => Some(".mp4"),
                    "video/x-matroska" => Some(".mkv"),
                    "image/jpeg" => Some(".jpg"),
                    "image/png" => Some(".png"),
                    "application/pdf" => Some(".pdf"),
                    "audio/mpeg" => Some(".mp3"),
                    _ => None,
                };
                if let Some(e) = ext {
                    final_filename = Some(format!("{}{}", name, e));
                }
            }
        }
    }

    Ok(UrlTypeInfo {
        is_magnet: false,
        content_type,
        content_length,
        hinted_filename: final_filename,
    })
}

/// Resolves a human-provided path into a valid, absolute filesystem path.
/// 
/// It handles:
/// - Absolute vs Relative paths.
/// - System-specific "Downloads" folder fallback.
/// - Custom user-defined download directories.
pub(crate) fn resolve_download_path(app: &tauri::AppHandle, db_path: &str, provided_path: &str, override_folder: Option<String>) -> String {
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
                app.path().download_dir().unwrap_or_else(|_| PathBuf::from(".")).join(path)
            }
        } else {
            // Fallback to system's downloads folder via Tauri
            app.path().download_dir().unwrap_or_else(|_| PathBuf::from(".")).join("Ciel Downloads")
        }
    };

    // Ensure base directory exists
    let _ = std::fs::create_dir_all(&base_dir);

    // If provided path is simply a filename or relative like ./file
    let file_name = p.file_name().unwrap_or_default();
    let final_path = base_dir.join(file_name);
    
    // Ensure the path is absolute for external tool reliability (Explorer, etc)
    let absolute_path = if final_path.is_absolute() {
        final_path
    } else {
        app.path().download_dir().unwrap_or_else(|_| PathBuf::from(".")).join(final_path)
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
    let extension = path.extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();

    match extension.as_str() {
        "mp4" | "mkv" | "avi" | "mov" | "webm" | "flv" | "wmv" | "m4v" => "Video".to_string(),
        "mp3" | "wav" | "flac" | "aac" | "ogg" | "m4a" | "wma" => "Audio".to_string(),
        "zip" | "rar" | "7z" | "tar" | "gz" | "bz2" | "iso" => "Compressed".to_string(),
        "exe" | "msi" | "app" | "dmg" | "deb" | "rpm" => "Software".to_string(),
        "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "txt" | "rtf" | "epub" => "Documents".to_string(),
        _ => "Other".to_string(),
    }
}


/// Triggers post-transfer logic like opening the target folder or system power management.
/// 
/// This is called automatically when a download transitions to the 'Completed' status.
pub (crate) async fn execute_post_download_actions(app: AppHandle, db_path: String, download: Download) {
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
            let active_count = downloads.iter()
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
                    let _ = std::process::Command::new("shutdown")
                        .args(&["+1"])
                        .spawn();
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

/// Bridge: Initiates a new HTTP download.
/// 
/// This command:
/// 1. Resolves and validates the target filename (sniffing headers if needed).
/// 2. Ensures a unique path to prevent collisions.
/// 3. Persists the record to the database.
/// 4. Dispatches the async download task.
#[tauri::command]
pub async fn add_download(
    app: AppHandle,
    db_state: State<'_, DbState>,
    manager: State<'_, DownloadManager>,
    url: String,
    filename: String,
    _filepath: String,
    output_folder: Option<String>,
    user_agent: Option<String>,
    cookies: Option<String>,
) -> Result<Download, String> {

    // Get max connections from settings
    let max_connections = db::get_setting(&db_state.path, "max_connections")
        .ok()
        .flatten()
        .and_then(|v| v.parse::<i32>().ok())
        .unwrap_or(16);
    
    // Streamline: No synchronous sniffing here. 
    // The Downloader will handle metadata discovery in the background to prevent UI lag.
    let mut filename = filename;
    if filename.is_empty() {
        filename = "download_file".to_string();
    }

    // Finalize resolved path using the potentially updated filename and optional folder override
    let resolved_path = resolve_download_path(&app, &db_state.path, &filename, output_folder);
    let final_resolved_path = ensure_unique_path(&db_state.path, resolved_path);

    // Extract the final unique filename from the path
    let final_filename = Path::new(&final_resolved_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| filename.clone());

    let id = uuid::Uuid::new_v4().to_string();
    let download = Download {
        id: id.clone(),
        url: url.clone(),
        filename: final_filename,
        filepath: final_resolved_path,
        size: 0,
        downloaded: 0,
        status: DownloadStatus::Downloading,
        protocol: DownloadProtocol::Http,
        speed: 0,
        connections: max_connections,
        created_at: chrono::Utc::now().to_rfc3339(),
        completed_at: None,
        error_message: None,
        info_hash: None,
        metadata: None,
        user_agent,
        cookies,
        category: get_category_from_filename(&filename),
    };

    db::insert_download(&db_state.path, &download).map_err(|e| e.to_string())?;
    db::log_event(&db_state.path, &download.id, "created", Some("HTTP download initiated")).ok();

    start_download_task(app, db_state.path.clone(), manager.inner().clone(), download.clone()).await?;

    Ok(download)
}

/// Bridge: Initiates a new BitTorrent download (Magnet or .torrent file).
/// 
/// This command handles:
/// - Metadata extraction from magnet query parameters.
/// - Duplicate isolation: If a torrent with the same name exists, it creates 
///   a dedicated sub-folder to prevent file/hash collisions.
/// - Registration with the `TorrentManager`.
#[tauri::command]
pub async fn add_torrent(
    app: AppHandle,
    db_state: State<'_, DbState>,
    torrent_manager: State<'_, TorrentManager>,
    url: String, // Magnet link or local file path
    mut filename: String,
    _filepath: String,
    output_folder: Option<String>,
    indices: Option<Vec<usize>>,
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

    let id = uuid::Uuid::new_v4().to_string();
    let download = Download {
        id: id.clone(),
        url: url.clone(),
        filename: final_filename,
        filepath: final_resolved_path.clone(),
        size: 0,
        downloaded: 0,
        status: DownloadStatus::Downloading,
        protocol: DownloadProtocol::Torrent,
        speed: 0,
        connections: 0,
        created_at: chrono::Utc::now().to_rfc3339(),
        completed_at: None,
        error_message: None,
        info_hash: None,
        metadata: None,
        user_agent: None,
        cookies: None,
        category: "Other".to_string(), // Torrents can be anything, default to other or analyze further?
    };

    db::insert_download(&db_state.path, &download).map_err(|e| e.to_string())?;
    db::log_event(&db_state.path, &download.id, "created", Some("Torrent download initiated")).ok();

    let is_duplicate = resolved_path != final_resolved_path;

    // For torrents, base_folder must always be a DIRECTORY (not a file path)
    // librqbit will create the torrent's internal file structure inside this folder
    let base_folder = if is_duplicate {
        // For duplicates, use the unique path as a folder
        // BUT strip the extension so it looks like a folder, e.g. "Downloads/Movie (1)"
        // This isolates the duplicate download in its own folder, guaranteeing no hash collision
        let path = Path::new(&final_resolved_path);
        let stem = path.file_stem().unwrap_or(std::ffi::OsStr::new("unknown"));
        let parent = path.parent().unwrap_or(Path::new("."));
        parent.join(stem).to_string_lossy().to_string()
    } else if let Some(folder) = output_folder {
        folder
    } else {
        // Use the parent directory of the resolved path
        Path::new(&final_resolved_path).parent().unwrap_or(Path::new(".")).to_string_lossy().to_string()
    };
    
    torrent_manager.add_magnet(app, id.clone(), url, base_folder, db_state.path.clone(), indices, 0, false).await?;

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

/// Internal: Spawns the long-running async task for an HTTP download.
/// 
/// It sets up:
/// - Real-time progress emission via Tauri events.
/// - Graceful cancellation handling.
/// - Database persistence of progress and final status.
/// - OS-level notifications on completion/failure.
async fn start_download_task(
    app: AppHandle,
    db_path: String,
    manager: DownloadManager,
    download: Download,
) -> Result<(), String> {
    let id = download.id.clone();
    let url = download.url.clone();
    let filepath = download.filepath.clone();
    let filename = download.filename.clone(); // Clone filename for use in tokio::spawn
    let connections = download.connections as u8;

    // Create cancellation channel and signal
    let (tx, mut rx) = mpsc::channel(1);
    let is_cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    
    manager.add_active(id.clone(), tx).await;

    // Fetch global speed limit
    let speed_limit = db::get_setting(&db_path, "speed_limit")
        .ok()
        .flatten()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);

    // Spawn download in background
    tokio::spawn(async move {
        let config = DownloadConfig {
            id: id.clone(),
            url,
            filepath: PathBuf::from(filepath),
            connections,
            chunk_size: 5 * 1024 * 1024,
            speed_limit,
            user_agent: download.user_agent.clone(),
            cookies: download.cookies.clone(),
        };

        let downloader = Downloader::new(config)
            .with_db(db_path.clone())
            .with_cancel_signal(is_cancelled.clone()); // Pass signal

        let id_inner = id.clone();
        let db_path_inner = db_path.clone();
        let app_clone = app.clone();
        let filename_inner = filename.clone(); // Clone filename for notification

        // Wrap download in a select to handle cancellation
        let download_task = downloader.download(move |progress| {
            let _ = app_clone.emit("download-progress", progress);
        });

        tokio::select! {
            res = download_task => {
                match res {
                    Ok(_) => {
                        // Get final stats from downloader if possible to ensure DB is accurate
                        let progress = downloader.get_progress();
                        let (final_downloaded, final_total) = {
                            let p = progress.lock().unwrap();
                            (p.downloaded, p.total)
                        };
                        
                        let _ = db::update_download_progress(&db_path_inner, &id_inner, final_downloaded as i64, 0);
                        if final_total > 0 {
                            let _ = db::update_download_size(&db_path_inner, &id_inner, final_total as i64);
                        }
                        
                        let _ = db::update_download_status(&db_path_inner, &id_inner, DownloadStatus::Completed);
                        let _ = app.emit("download-completed", id_inner.clone());

                        // Native Notification
                        app.notification()
                            .builder()
                            .title("Download Completed")
                            .body(format!("{} has finished downloading successfully.", filename_inner))
                            .show().ok();

                        // Post-Download Actions
                        let download_clone = download.clone();
                        execute_post_download_actions(app.clone(), db_path_inner.clone(), download_clone).await;
                    }
                    Err(e) => {
                        let _ = db::update_download_status(&db_path_inner, &id_inner, DownloadStatus::Error);
                        let _ = app.emit("download-error", (id_inner.clone(), e.to_string()));

                        // Native Notification
                        app.notification()
                            .builder()
                            .title("Download Failed")
                            .body(format!("Failed to download {}: {}", filename_inner, e))
                            .show().ok();
                    }
                }
            }
            _ = rx.recv() => {
                // Signal cancellation to workers
                is_cancelled.store(true, std::sync::atomic::Ordering::Relaxed);
                let _ = db::update_download_status(&db_path_inner, &id_inner, DownloadStatus::Paused);
                let _ = app.emit("download-paused", id_inner.clone());
            }
        }
        
        manager.remove_active(&id_inner).await;
    });

    Ok(())
}

/// Bridge: Pauses an active transfer.
/// 
/// For HTTP, it signals the worker to stop. For Torrents, it communicates
/// directly with the `librqbit` session.
#[tauri::command]
pub async fn pause_download(
    app: AppHandle,
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
    db::update_download_status(&db_state.path, &id, DownloadStatus::Paused)
        .map_err(|e| e.to_string())?;
    
    // Immediate UI Feedback
    // We construct a partial object that the frontend will merge/handle
    // The frontend mainly looks at 'status_text' for logic overrides we added
    use tauri::Emitter;
    let _ = app.emit("download-progress", serde_json::json!({
        "id": id,
        "total": download.size,
        "downloaded": download.downloaded,
        "speed": 0,
        "eta": 0,
        "connections": 0,
        "status_text": "Paused",
    }));

    Ok(())
}

/// Bridge: Resumes a previously paused transfer.
#[tauri::command]
pub async fn resume_download(
    app: AppHandle,
    db_state: State<'_, DbState>,
    manager: State<'_, DownloadManager>,
    torrent_manager: State<'_, TorrentManager>,
    id: String,
) -> Result<(), String> {
    let downloads = db::get_all_downloads(&db_state.path).map_err(|e| e.to_string())?;
    let mut download = downloads.into_iter().find(|d| d.id == id).ok_or("Download not found")?.clone();

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

    // Check if truly active in its respective manager
    let is_active = match download.protocol {
        DownloadProtocol::Torrent => torrent_manager.is_active(&id).await,
        _ => manager.is_active(&id).await,
    };

    if is_active && download.status == DownloadStatus::Downloading {
         // If it's active and status is downloading, it might already be running.
         // But we allow resuming a paused torrent.
    }

    db::update_download_status(&db_state.path, &id, DownloadStatus::Downloading).map_err(|e| e.to_string())?;
    db::log_event(&db_state.path, &id, "resumed", None).ok();

    match download.protocol {
        DownloadProtocol::Torrent => {
            // Try to resume existing session
            if let Err(_) = torrent_manager.resume_torrent(&id).await {
                // If it wasn't in the active session map (e.g. app restart), re-add it.
                // It will automatically verify existing files and resume.
                let output_folder = std::path::Path::new(&download.filepath).parent()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default();
                
                // Extract indices from metadata if present
                let indices = if let Some(meta_str) = &download.metadata {
                    if let Ok(json) = serde_json::from_str::<serde_json::Value>(meta_str) {
                         json["indices"].as_array().map(|arr| {
                             arr.iter().filter_map(|v| v.as_u64().map(|n| n as usize)).collect::<Vec<usize>>()
                         })
                    } else { None }
                } else { None };

                torrent_manager.add_magnet(
                    app, 
                    id, 
                    download.url, 
                    output_folder, 
                    db_state.path.clone(), 
                    indices,
                    download.size as u64,
                    true
                ).await?;
            }
        }
        _ => {
            start_download_task(app, db_state.path.clone(), manager.inner().clone(), download.clone()).await?;
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
                let _ = tm.delete_torrent(&id, delete_files, Some(download.filepath.clone())).await;
                if let Some(hash) = download.info_hash {
                    let _ = tm.delete_torrent_by_hash(hash, delete_files).await;
                } else if let Some(hash) = TorrentManager::extract_info_hash_from_magnet(&download.url) {
                    let _ = tm.delete_torrent_by_hash(hash, delete_files).await;
                }
            } else {
                m.cancel(&id).await;
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
pub fn show_in_folder(app: AppHandle, db_state: State<'_, DbState>, path: String) -> Result<(), String> {
    show_in_folder_internal(app, &db_state.path, path)
}

/// Internal wrapper for "Show in Folder" that handles cross-platform logic.
/// 
/// - Windows: Uses `explorer.exe /select` to highlight the file.
/// - MacOS: Uses `open -R`.
/// - Linux: Uses `xdg-open` on the parent folder.
pub fn show_in_folder_internal(app: AppHandle, db_path: &str, path: String) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let path_norm = path.replace("/", "\\");
        let mut p_buf = PathBuf::from(&path_norm);
        
        // Ensure path is absolute
        if !p_buf.is_absolute() {
             // Try to get configured download path first
             let configured_path = db::get_setting(&db_path, "download_path")
                .ok()
                .flatten()
                .filter(|p| !p.is_empty())
                .map(PathBuf::from);

             let base = configured_path.unwrap_or_else(|| {
                 app.path().download_dir().unwrap_or_else(|_| PathBuf::from("."))
             });
             
             p_buf = base.join(p_buf);
        }

        let p = p_buf.as_path();
        
        if p.exists() {
            if p.is_dir() {
                // If it's a directory, just open it
                let _ = std::process::Command::new("explorer.exe")
                    .arg(p)
                    .spawn();
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
                    let _ = std::process::Command::new("explorer.exe")
                        .arg(parent)
                        .spawn();
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

