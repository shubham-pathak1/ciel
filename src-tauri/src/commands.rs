use crate::db::{self, DbState, Download, DownloadStatus, DownloadProtocol};
use crate::downloader::{Downloader, DownloadConfig};
use crate::torrent::TorrentManager;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, Runtime, State};
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use rookie;


use std::fs;
use tauri_plugin_notification::NotificationExt;

/// Orchestrates the lifecycle of active HTTP downloads.
/// 
/// It acts as a registry for ongoing transfers, allowing the application
/// to send cancellation signals to specific download tasks via `mpsc` channels.
#[derive(Clone)]
pub struct DownloadManager {
    /// Internal map linking Download IDs to their respective cancellation senders and progress monitors.
    active_downloads: Arc<Mutex<HashMap<String, (mpsc::Sender<()>, Arc<std::sync::Mutex<crate::downloader::DownloadProgress>>)>>>,
}

impl DownloadManager {
    pub fn new() -> Self {
        Self {
            active_downloads: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Registers a new active download and its cancellation hook.
    pub async fn add_active(&self, id: String, cancel_tx: mpsc::Sender<()>, progress: Arc<std::sync::Mutex<crate::downloader::DownloadProgress>>) {
        let mut active = self.active_downloads.lock().await;
        active.insert(id, (cancel_tx, progress));
    }

    /// Unregisters a download, typically called after a successful completion or an error.
    pub async fn remove_active(&self, id: &str) {
        let mut active = self.active_downloads.lock().await;
        active.remove(id);
    }

    /// Signals an active download task to abort immediately.
    pub async fn cancel(&self, id: &str) {
        let mut active = self.active_downloads.lock().await;
        if let Some((tx, _)) = active.get(id) {
            // Signal the async task to stop.
            let _ = tx.send(()).await;
        }
        active.remove(id);
    }

    pub async fn is_active(&self, id: &str) -> bool {
        self.active_downloads.lock().await.contains_key(id)
    }

    /// Calculates aggregate download statistics for the system tray.
    pub async fn get_global_status(&self) -> (usize, u64) {
        let active = self.active_downloads.lock().await;
        let mut total_speed = 0;
        let count = active.len();
        
        for (_, (_, progress)) in active.iter() {
            if let Ok(p) = progress.lock() {
                total_speed += p.speed;
            }
        }
        
        (count, total_speed)
    }
}

/// Helper to transform Google Drive viewer links into direct download links.
fn transform_google_drive_url(url: &str) -> String {
    // 1. Convert /file/d/ID/view -> uc?export=download&id=ID
    if url.contains("drive.google.com/file/d/") && (url.contains("/view") || url.contains("/edit")) {
        if let Some(re) = regex::Regex::new(r"drive\.google\.com/file/d/([^/?#]+)").ok() {
            if let Some(caps) = re.captures(url) {
                if let Some(id) = caps.get(1) {
                    return format!("https://drive.google.com/uc?export=download&id={}&confirm=t", id.as_str());
                }
            }
        }
    }

    // 2. Convert open?id=ID -> uc?export=download&id=ID
    if url.contains("drive.google.com/open?id=") {
        if let Some(re) = regex::Regex::new(r"id=([^&?#]+)").ok() {
            if let Some(caps) = re.captures(url) {
                if let Some(id) = caps.get(1) {
                    return format!("https://drive.google.com/uc?export=download&id={}&confirm=t", id.as_str());
                }
            }
        }
    }
    url.to_string()
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
    /// The final resolved URL (useful for Drive tokens discovered during validation).
    resolved_url: Option<String>,
}

/// Performs a lightweight inspection of a URL to determine its type and metadata.
/// 
/// Instead of downloading the full file, it uses HTTP `GET` with a `Range` header
/// or sniffs the first few bytes to extract headers and verify the content type.
#[tauri::command]
pub async fn validate_url_type(db_state: State<'_, DbState>, url: String) -> Result<UrlTypeInfo, String> {
    let url = transform_google_drive_url(&url);
    if url.starts_with("magnet:") {
        return Ok(UrlTypeInfo {
            is_magnet: true,
            content_type: None,
            content_length: None,
            hinted_filename: None,
            resolved_url: Some(url),
        });
    }

    let mut builder = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36");

    // Automatically fetch cookies if a browser is selected in settings
    if let Ok(Some(browser)) = db::get_setting(&db_state.path, "cookie_browser") {
        if browser != "none" {
            if let Some(cookies) = get_cookies_from_browser(&browser, &url) {
                use reqwest::header::{HeaderMap, HeaderValue, COOKIE};
                let mut headers = HeaderMap::new();
                if let Ok(v) = HeaderValue::from_str(&cookies) {
                    headers.insert(COOKIE, v);
                    builder = builder.default_headers(headers);
                }
            }
        }
    }

    let client = builder.build().unwrap_or_default();

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

    let hinted_filename = Some(crate::downloader::extract_filename(&url, headers));

    // Special check for Google Drive: if it's returning HTML, it's likely the "virus scan" warning or login page.
    if url.contains("drive.google.com/uc") && content_type.as_ref().map(|s| s.contains("text/html")).unwrap_or(false) {
        // Optimization: Try to find a confirm token in the HTML body if confirm=t failed.
        // We need to fetch the FULL body, so we retry without the Range header.
        if let Ok(full_res) = client.get(&url).send().await {
            let body = full_res.text().await.unwrap_or_default();
            
            // 1. Scrape metadata from HTML as a fallback
            let mut scraped_filename = None;
            let mut scraped_size = None;
            if let Some(re) = regex::Regex::new(r#"class="uc-name-size"><a [^>]+>([^<]+)</a>\s*\(([^)]+)\)"#).ok() {
                if let Some(caps) = re.captures(&body) {
                    scraped_filename = caps.get(1).map(|m| m.as_str().to_string());
                    scraped_size = caps.get(2).map(|m| m.as_str().to_string());
                }
            }

            // 2. Try to find confirm token and uuid
            let mut found_token = None;
            let mut found_uuid = None;
            
            if let Some(re) = regex::Regex::new(r#"name="confirm" value="([^"]+)""#).ok() {
                found_token = re.captures(&body).and_then(|c| c.get(1)).map(|m| m.as_str().to_string());
            }
            if let Some(re) = regex::Regex::new(r#"name="uuid" value="([^"]+)""#).ok() {
                found_uuid = re.captures(&body).and_then(|c| c.get(1)).map(|m| m.as_str().to_string());
            }

            // 3. Loop Prevention: If we only found 't' and we already had it, or we found nothing new
            if let Some(ref token) = found_token {
                if token == "t" && url.contains("confirm=t") {
                    found_token = None; // Stop recursion
                }
            }

            if let Some(token) = found_token {
                let mut new_url = url.clone();
                if !new_url.contains("confirm=") {
                    new_url = format!("{}&confirm={}", new_url, token);
                } else {
                    new_url = new_url.replace("confirm=t", &format!("confirm={}", token));
                }
                
                if let Some(uuid) = found_uuid {
                    if !new_url.contains("uuid=") {
                        new_url = format!("{}&uuid={}", new_url, uuid);
                    }
                }
                
                return Box::pin(validate_url_type(db_state, new_url)).await;
            }

            // 4. Return Warning with scraped metadata
            let display_name = match (scraped_filename, scraped_size) {
                (Some(n), Some(s)) => format!("{} ({}) - Login Required", n, s),
                (Some(n), None) => format!("{} - Login Required", n),
                _ => "Google Drive Warning (Check Settings > Privacy)".to_string(),
            };

            return Ok(UrlTypeInfo {
                is_magnet: false,
                content_type: Some("text/html".to_string()),
                content_length: None,
                hinted_filename: Some(display_name),
                resolved_url: Some(url),
            });
        }
    }

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
        resolved_url: Some(url),
    })
}

/// Resolves a human-provided path into a valid, absolute filesystem path.
/// 
/// It handles:
/// - Absolute vs Relative paths.
/// - System-specific "Downloads" folder fallback.
/// - Custom user-defined download directories.
pub(crate) fn resolve_download_path<R: Runtime>(app: &tauri::AppHandle<R>, db_path: &str, provided_path: &str, override_folder: Option<String>) -> String {
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

    // --- START AUTO-ORGANIZE LOGIC ---
    let auto_organize = db::get_setting(db_path, "auto_organize")
        .unwrap_or(None)
        .map(|v| v == "true")
        .unwrap_or(false);

    let base_dir = if auto_organize {
        let category = get_category_from_filename(p.file_name().unwrap_or_default().to_str().unwrap_or_default());
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
pub (crate) async fn execute_post_download_actions<R: Runtime>(app: AppHandle<R>, db_path: String, download: Download) {
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
pub async fn add_download<R: Runtime>(
    app: AppHandle<R>,
    db_state: State<'_, DbState>,
    manager: State<'_, DownloadManager>,
    torrent_manager: State<'_, TorrentManager>,
    url: String,
    filename: String,
    _filepath: String,
    output_folder: Option<String>,
    user_agent: Option<String>,
    mut cookies: Option<String>,
    start_paused: Option<bool>,
) -> Result<Download, String> {
    let url = transform_google_drive_url(&url);

    // Automatically fetch cookies if a browser is selected in settings and none provided
    if cookies.is_none() || cookies.as_ref().map(|s| s.is_empty()).unwrap_or(false) {
        if let Ok(Some(browser)) = db::get_setting(&db_state.path, "cookie_browser") {
            if browser != "none" {
                cookies = get_cookies_from_browser(&browser, &url);
            }
        }
    }

    // Get max connections from settings
    let max_connections = db::get_setting(&db_state.path, "max_connections")
        .ok()
        .flatten()
        .and_then(|v| v.parse::<i32>().ok())
        .unwrap_or(16);
    
    // Streamline: No synchronous sniffing here. 
    // The Downloader will handle metadata discovery in the background to prevent UI lag.
    let mut filename = if let Ok(decoded) = percent_encoding::percent_decode(filename.as_bytes()).decode_utf8() {
        decoded.into_owned()
    } else {
        filename
    };

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

    // Queue enforcement: Check if we can start immediately or must queue
    let max_simultaneous = db::get_setting(&db_state.path, "max_concurrent")
        .ok()
        .flatten()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(3);
    
    let active_count = manager.active_downloads.lock().await.len();
    let (torrent_active, _) = torrent_manager.get_global_status().await;
    let should_queue = !start_paused.unwrap_or(false) && (active_count + torrent_active) >= max_simultaneous;

    let id = uuid::Uuid::new_v4().to_string();
    let download = Download {
        id: id.clone(),
        url: url.clone(),
        filename: final_filename,
        filepath: final_resolved_path,
        size: 0,
        downloaded: 0,
        status: if start_paused.unwrap_or(false) { 
            DownloadStatus::Paused 
        } else if should_queue { 
            DownloadStatus::Queued 
        } else { 
            DownloadStatus::Downloading 
        },
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
    db::log_event(&db_state.path, &download.id, "created", Some(if start_paused.unwrap_or(false) { 
        "HTTP download added (Scheduled/Paused)" 
    } else if should_queue {
        "HTTP download queued (concurrent limit reached)"
    } else { 
        "HTTP download initiated" 
    })).ok();

    // Only start if not paused and not queued
    if !start_paused.unwrap_or(false) && !should_queue {
        start_download_task(app, db_state.path.clone(), manager.inner().clone(), download.clone()).await?;
    }

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
    let http_active = manager.active_downloads.lock().await.len();
    let (torrent_active, _) = torrent_manager.get_global_status().await;
    let active_count = http_active + torrent_active;
    let should_queue = !start_paused.unwrap_or(false) && active_count >= max_simultaneous;

    let id = uuid::Uuid::new_v4().to_string();
    let download = Download {
        id: id.clone(),
        url: url.clone(),
        filename: final_filename,
        filepath: final_resolved_path.clone(),
        size: 0,
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
        metadata: indices.as_ref().and_then(|idxs| serde_json::to_string(idxs).ok()),
        user_agent: None,
        cookies: None,
        category: "Other".to_string(),
    };

    db::insert_download(&db_state.path, &download).map_err(|e| e.to_string())?;
    db::log_event(&db_state.path, &download.id, "created", Some(if start_paused.unwrap_or(false) { 
        "Torrent added (Scheduled/Paused)" 
    } else if should_queue {
        "Torrent queued (concurrent limit reached)"
    } else { 
        "Torrent download initiated" 
    })).ok();

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
        Path::new(&final_resolved_path).parent().unwrap_or(Path::new(".")).to_string_lossy().to_string()
    };
    
    // Only start if not paused and not queued
    if !should_queue {
        torrent_manager.add_magnet(app, id.clone(), url, base_folder, db_state.path.clone(), indices, 0, false, start_paused.unwrap_or(false)).await?;
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

/// Internal: Spawns the long-running async task for an HTTP download.
/// 
/// It sets up:
/// - Real-time progress emission via Tauri events.
/// - Graceful cancellation handling.
/// - Database persistence of progress and final status.
/// - OS-level notifications on completion/failure.
async fn start_download_task<R: Runtime>(
    app: AppHandle<R>,
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
    
    // Fetch global speed limit
    let speed_limit = db::get_setting(&db_path, "speed_limit")
        .ok()
        .flatten()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);

    // Spawn download in background
    tokio::spawn(async move {
        let mut cookies = download.cookies.clone();

        // Automatic Browser Cookie Extraction
        if cookies.is_none() {
            if let Ok(Some(browser)) = db::get_setting(&db_path, "cookie_browser") {
                if browser != "none" {
                    cookies = get_cookies_from_browser(&browser, &url);
                    if let Some(ref c) = cookies {
                        // Log success and update DB so we don't have to extract every time for this link
                        let _ = db::update_download_cookies(&db_path, &id, c);
                    }
                }
            }
        }

        let config = DownloadConfig {
            id: id.clone(),
            url,
            filepath: PathBuf::from(filepath),
            connections,
            chunk_size: 5 * 1024 * 1024,
            speed_limit,
            user_agent: download.user_agent.clone(),
            cookies,
        };

        let downloader = Downloader::new(config)
            .with_db(db_path.clone())
            .with_cancel_signal(is_cancelled.clone()); // Pass signal

        let progress_obj = downloader.get_progress();
        manager.add_active(id.clone(), tx, progress_obj.clone()).await;

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
                        
                        let _ = db::mark_download_completed(&db_path_inner, &id_inner);
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
                        let _ = app.emit(
                            "download-error",
                            serde_json::json!({
                                "id": id_inner.clone(),
                                "message": e.to_string()
                            }),
                        );

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
pub async fn resume_download<R: Runtime>(
    app: AppHandle<R>,
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
            // Try to resume existing in-memory session first.
            if torrent_manager.is_active(&id).await {
                torrent_manager.resume_torrent(&id).await?;
            } else {
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
                    true,
                    false
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
pub fn show_in_folder<R: Runtime>(app: AppHandle<R>, db_state: State<'_, DbState>, path: String) -> Result<(), String> {
    show_in_folder_internal(app, &db_state.path, path)
}

/// Internal wrapper for "Show in Folder" that handles cross-platform logic.
/// 
/// - Windows: Uses `explorer.exe /select` to highlight the file.
/// - MacOS: Uses `open -R`.
/// - Linux: Uses `xdg-open` on the parent folder.
pub fn show_in_folder_internal<R: Runtime>(app: AppHandle<R>, db_path: &str, path: String) -> Result<(), String> {
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

/// New Deep Search for Firefox cookies on Windows to bypass file locks and find correct profiles.
fn get_cookies_from_firefox_deep(url_str: &str) -> Option<String> {
    let domain = url::Url::parse(url_str).ok()?.host_str()?.to_string();
    let target_host = domain.to_lowercase();
    
    // 1. Resolve Firefox Profile directory on Windows
    let app_data = std::env::var("APPDATA").ok()?;
    let profiles_path = Path::new(&app_data).join("Mozilla").join("Firefox").join("Profiles");
    
    println!("[Firefox] Scaning for profiles in: {:?}", profiles_path);
    if !profiles_path.exists() { 
        println!("[Firefox] Profiles directory not found.");
        return None; 
    }

    let mut all_cookies = Vec::new();

    // 2. Iterate through all profile folders
    if let Ok(entries) = fs::read_dir(profiles_path) {
        for entry in entries.flatten() {
            let cookie_db = entry.path().join("cookies.sqlite");
            if cookie_db.exists() {
                println!("[Firefox] Found profile with cookies: {:?}", entry.path());
                // 3. SECRETS OF THE DEEP: Copy the database to bypass Firefox's file lock
                let temp_db = std::env::temp_dir().join(format!("ciel_tmp_cookies_{}.sqlite", uuid::Uuid::new_v4()));
                if fs::copy(&cookie_db, &temp_db).is_ok() {
                    // 4. Use rusqlite to read the copied database
                    if let Ok(conn) = rusqlite::Connection::open(&temp_db) {
                        let stmt = conn.prepare("SELECT name, value, host FROM moz_cookies").ok();
                        if let Some(mut stmt) = stmt {
                            let rows = stmt.query_map([], |row| {
                                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?))
                            }).ok();
                            
                            if let Some(rows) = rows {
                                for row in rows.flatten() {
                                    let (name, value, host) = row;
                                    let cookie_domain = host.trim_start_matches('.').to_lowercase();
                                    if target_host == cookie_domain || target_host.ends_with(&format!(".{}", cookie_domain)) {
                                        all_cookies.push(format!("{}={}", name, value));
                                    }
                                }
                            }
                            println!("[Firefox] Extracted {} relevant cookies from this profile.", all_cookies.len());
                        }
                    }
                    let _ = fs::remove_file(temp_db);
                }
            }
        }
    }

    if all_cookies.is_empty() { None } else { Some(all_cookies.join("; ")) }
}

/// Helper: Extracts cookies for a specific URL from a chosen browser using `rookie`.
fn get_cookies_from_browser(browser: &str, url: &str) -> Option<String> {
    // SPECIAL CASE: Deep Scan for Firefox on Windows
    #[cfg(target_os = "windows")]
    if browser.to_lowercase() == "firefox" {
        if let Some(cookies) = get_cookies_from_firefox_deep(url) {
            return Some(cookies);
        }
    }

    let domain = url::Url::parse(url).ok()?.host_str()?.to_string();
    
    let cookies_result = match browser.to_lowercase().as_str() {
        "chrome" => rookie::chrome(None),
        "firefox" => rookie::firefox(None),
        "edge" => rookie::edge(None),
        "brave" => rookie::brave(None),
        "opera" => rookie::opera(None),
        "vivaldi" => rookie::vivaldi(None),
        #[cfg(target_os = "macos")]
        "safari" => rookie::safari(None),
        _ => return None,
    };

    match cookies_result {
        Ok(cookies) => {
            let target_host = domain.to_lowercase();
            let cookie_str = cookies.iter()
                .filter(|c| {
                    let cookie_domain = c.domain.trim_start_matches('.').to_lowercase();
                    target_host == cookie_domain || target_host.ends_with(&format!(".{}", cookie_domain))
                })
                .map(|c| format!("{}={}", c.name, c.value))
                .collect::<Vec<_>>()
                .join("; ");
            
            if cookie_str.is_empty() { None } else { Some(cookie_str) }
        }
        Err(e) => {
            eprintln!("Failed to extract cookies from {}: {}", browser, e);
            None
        }
    }
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
        
        let http_active = manager.active_downloads.lock().await.len();
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
                if let Err(e) = start_download_task(app.clone(), db_state.path.clone(), manager.inner().clone(), next_download).await {
                     eprintln!("Failed to start queued HTTP download {}: {}", id, e);
                     let _ = db::update_download_status(&db_state.path, &id, DownloadStatus::Error);
                     let _ = app.emit(
                         "download-error",
                         serde_json::json!({
                             "id": id,
                             "message": e
                         }),
                     );
                }
            },
            DownloadProtocol::Torrent => {
                let path = Path::new(&next_download.filepath);
                let base_folder = path.parent().unwrap_or(Path::new(".")).to_string_lossy().to_string();
                
                let indices: Option<Vec<usize>> = next_download.metadata.as_ref()
                    .and_then(|m| serde_json::from_str(m).ok());

                if let Err(e) = torrent_manager.add_magnet(
                    app.clone(), 
                    id.clone(), 
                    next_download.url.clone(), 
                    base_folder, 
                    db_state.path.clone(), 
                    indices, 
                    0, 
                    true, // is_resume
                    false // start_paused
                ).await {
                     eprintln!("Failed to start queued torrent {}: {}", id, e);
                     let _ = db::update_download_status(&db_state.path, &id, DownloadStatus::Error);
                     let _ = app.emit(
                         "download-error",
                         serde_json::json!({
                             "id": id,
                             "message": e
                         }),
                     );
                }
            },
            DownloadProtocol::Video => {
                // TODO: Implement video download queuing when video support is fully added
                eprintln!("Video queuing not yet supported for {}", id);
                let _ = db::update_download_status(&db_state.path, &id, DownloadStatus::Error);
            }
        }
    }
}

