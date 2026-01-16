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

/// Manager for active downloads
pub struct DownloadManager {
    // Maps download ID to a cancellation sender
    active_downloads: Arc<Mutex<HashMap<String, mpsc::Sender<()>>>>,
}

impl DownloadManager {
    pub fn new() -> Self {
        Self {
            active_downloads: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn add_active(&self, id: String, cancel_tx: mpsc::Sender<()>) {
        let mut active = self.active_downloads.lock().await;
        active.insert(id, cancel_tx);
    }

    pub async fn remove_active(&self, id: &str) {
        let mut active = self.active_downloads.lock().await;
        active.remove(id);
    }

    pub async fn cancel(&self, id: &str) {
        let mut active = self.active_downloads.lock().await;
        if let Some(tx) = active.get(id) {
            let _ = tx.send(()).await;
        }
        active.remove(id);
    }

    pub async fn is_active(&self, id: &str) -> bool {
        self.active_downloads.lock().await.contains_key(id)
    }
}

// ... types for validation ...
#[derive(serde::Serialize)]
pub struct UrlTypeInfo {
    is_magnet: bool,
    content_type: Option<String>,
    content_length: Option<u64>,
    hinted_filename: Option<String>,
}

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

/// Helper to resolve authentic filepath
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

/// Helper to ensure unique filename if file already exists
pub(crate) fn ensure_unique_path(path_str: String) -> String {
    let path = Path::new(&path_str);
    if !path.exists() {
        return path_str;
    }

    let file_stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("download");
    let extension = path.extension().and_then(|s| s.to_str());
    let parent = path.parent().unwrap_or_else(|| Path::new("."));

    let mut counter = 1;
    loop {
        let new_filename = match extension {
            Some(ext) => format!("{} ({}).{}", file_stem, counter, ext),
            None => format!("{} ({})", file_stem, counter),
        };
        let new_path = parent.join(&new_filename);
        if !new_path.exists() {
            return new_path.to_string_lossy().to_string();
        }
        counter += 1;
        // Safety break to prevent infinite loops in degenerate cases
        if counter > 10000 {
             return path_str;
        }
    }
}


/// Execute post-download actions (Open folder, Shutdown, etc.)
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

/// Get all downloads
#[tauri::command]
pub fn get_downloads(db_state: State<DbState>) -> Result<Vec<Download>, String> {
    db::get_all_downloads(&db_state.path).map_err(|e| e.to_string())
}

/// Add a new download
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
    
    let mut filename = filename;
    
    // Check if current filename is "suspicious" (no extension, generic, or looks like a hash)
    let has_ext = filename.contains('.');
    let is_generic = filename == "download" || filename == "download_file" || filename.is_empty();
    let looks_like_hash = filename.len() > 15 && !filename.contains(' ') && !has_ext;

    if (is_generic || looks_like_hash || !has_ext) && url.starts_with("http") {
        let mut builder = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5));
            
        if let Some(ref ua) = user_agent {
            builder = builder.user_agent(ua);
        } else {
            builder = builder.user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36");
        }

        if let Some(ref c) = cookies {
            use reqwest::header::{HeaderMap, HeaderValue, COOKIE};
            let mut headers = HeaderMap::new();
            if let Ok(v) = HeaderValue::from_str(c) {
                headers.insert(COOKIE, v);
                builder = builder.default_headers(headers);
            }
        }

        let client = builder.build().unwrap_or_default();
            
        let mut found_name = None;
        
        // Try HEAD first
        if let Ok(res) = client.head(&url).send().await {
            let name = crate::downloader::extract_filename(&url, res.headers());
            if name != "download" && name != "download_file" {
                found_name = Some(name);
            }
        }
        
        // If HEAD failed or didn't give a good name, try GET with a tiny range
        if found_name.is_none() {
            if let Ok(res) = client.get(&url).header("Range", "bytes=0-0").send().await {
                let name = crate::downloader::extract_filename(&url, res.headers());
                if name != "download" && name != "download_file" {
                    found_name = Some(name);
                }
            }
        }
        
        if let Some(name) = found_name {
            filename = name;
        }
    }

    // Finalize resolved path using the potentially updated filename and optional folder override
    let resolved_path = resolve_download_path(&app, &db_state.path, &filename, output_folder);
    let final_resolved_path = ensure_unique_path(resolved_path);

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
    };

    db::insert_download(&db_state.path, &download).map_err(|e| e.to_string())?;
    db::log_event(&db_state.path, &download.id, "created", Some("HTTP download initiated")).ok();

    start_download_task(app, db_state.path.clone(), manager.inner().clone(), download.clone()).await?;

    Ok(download)
}

/// Add a new torrent download
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
    let final_resolved_path = ensure_unique_path(resolved_path);

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
        status: DownloadStatus::Queued,
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
    };

    db::insert_download(&db_state.path, &download).map_err(|e| e.to_string())?;
    db::log_event(&db_state.path, &download.id, "created", Some("Torrent download initiated")).ok();

    let base_folder = if let Some(folder) = output_folder {
        folder
    } else {
        Path::new(&final_resolved_path).parent().unwrap_or(Path::new(".")).to_string_lossy().to_string()
    };
    
    torrent_manager.add_magnet(app, id.clone(), url, base_folder, db_state.path.clone(), indices).await?;

    Ok(download)
}

/// Analyze a torrent/magnet to get metadata
#[tauri::command]
pub async fn analyze_torrent(
    _app: AppHandle,
    torrent_manager: State<'_, TorrentManager>,
    url: String,
) -> Result<crate::torrent::TorrentInfo, String> {
    torrent_manager.analyze_magnet(url).await
}

/// Start a torrent that was previously analyzed and added (starts as paused)
#[tauri::command]
pub async fn start_selective_torrent(
    _app: AppHandle,
    torrent_manager: State<'_, TorrentManager>,
    id: String,
    indices: Vec<usize>,
) -> Result<(), String> {
    torrent_manager.start_selective(&id, indices).await
}

/// Helper function to start the background download task
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

/// Pause a download
#[tauri::command]
pub async fn pause_download(
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
        .map_err(|e| e.to_string())
}

/// Resume a download
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

    if manager.is_active(&id).await {
        return Err("Download is already active".to_string());
    }

    db::update_download_status(&db_state.path, &id, DownloadStatus::Downloading).map_err(|e| e.to_string())?;
    db::log_event(&db_state.path, &id, "resumed", None).ok();

    match download.protocol {
        DownloadProtocol::Torrent => {
            torrent_manager.resume_torrent(&id).await?;
        }
        DownloadProtocol::Video => {
            crate::video::start_video_download_task(app, db_state.path.clone(), manager.inner().clone(), download.clone()).await?;
        }
        _ => {
            start_download_task(app, db_state.path.clone(), manager.inner().clone(), download.clone()).await?;
        }
    }

    Ok(())
}

/// Get download history
#[tauri::command]
pub async fn get_history(db_state: State<'_, DbState>) -> Result<Vec<Download>, String> {
    db::get_history(&db_state.path).map_err(|e| e.to_string())
}

/// Get events for a specific download
#[tauri::command]
pub async fn get_download_events(
    db_state: State<'_, DbState>,
    id: String,
) -> Result<Vec<(String, String, Option<String>)>, String> {
    db::get_download_events(&db_state.path, &id).map_err(|e| e.to_string())
}

/// Delete a download
#[tauri::command]
pub async fn delete_download(
    db_state: State<'_, DbState>,
    manager: State<'_, DownloadManager>,
    id: String,
) -> Result<(), String> {
    manager.cancel(&id).await;
    db::delete_download_by_id(&db_state.path, &id).map_err(|e| e.to_string())
}

/// Get all settings
#[tauri::command]
pub fn get_settings(db_state: State<DbState>) -> Result<HashMap<String, String>, String> {
    db::get_all_settings(&db_state.path).map_err(|e| e.to_string())
}

/// Update a setting
#[tauri::command]
pub fn update_setting(db_state: State<DbState>, key: String, value: String) -> Result<(), String> {
    db::set_setting(&db_state.path, &key, &value).map_err(|e| e.to_string())
}

/// Show file in folder
#[tauri::command]
pub fn show_in_folder(app: AppHandle, db_state: State<'_, DbState>, path: String) -> Result<(), String> {
    show_in_folder_internal(app, &db_state.path, path)
}

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

impl Clone for DownloadManager {
    fn clone(&self) -> Self {
        Self {
            active_downloads: self.active_downloads.clone(),
        }
    }
}

/// Clear finished downloads
#[tauri::command]
pub fn clear_finished(db_state: State<DbState>) -> Result<(), String> {
    db::delete_finished_downloads(&db_state.path).map_err(|e| e.to_string())
}
