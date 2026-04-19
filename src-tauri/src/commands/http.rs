use crate::db::{self, DbState, Download, DownloadProtocol, DownloadStatus};
use crate::downloader::{DownloadConfig, Downloader};
use crate::torrent::TorrentManager;
use super::{
    ensure_unique_path, execute_post_download_actions, get_category_from_filename,
    resolve_download_path, set_and_emit_download_error,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Runtime, State};
use tauri_plugin_notification::NotificationExt;
use tokio::sync::{mpsc, Mutex};
use rookie;

use std::fs;

/// Orchestrates the lifecycle of active HTTP downloads.
///
/// It acts as a registry for ongoing transfers, allowing the application
/// to send cancellation signals to specific download tasks via `mpsc` channels.
#[derive(Clone)]
pub struct DownloadManager {
    /// Internal map linking Download IDs to their respective cancellation senders and progress monitors.
    active_downloads: Arc<Mutex<HashMap<String, (mpsc::Sender<()>, Arc<std::sync::Mutex<crate::downloader::DownloadProgress>>)>>> ,
}

impl DownloadManager {
    pub fn new() -> Self {
        Self {
            active_downloads: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Registers a new active download and its cancellation hook.
    pub async fn add_active(
        &self,
        id: String,
        cancel_tx: mpsc::Sender<()>,
        progress: Arc<std::sync::Mutex<crate::downloader::DownloadProgress>>,
    ) {
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
    let response = client
        .get(&url)
        .header("Range", "bytes=0-0")
        .header(reqwest::header::ACCEPT_ENCODING, "identity")
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let headers = response.headers();
    let mut content_type = headers
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let content_length_from_header = headers
        .get(reqwest::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok());

    let content_length = headers
        .get(reqwest::header::CONTENT_RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split('/').last())
        .and_then(|v| v.parse::<u64>().ok())
        .or(content_length_from_header);

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
    if content_type
        .as_deref()
        .map_or(true, |ct| ct == "application/octet-stream" || ct == "application/x-zip-compressed")
    {
        if let Ok(range_res) = client.get(&url).header("Range", "bytes=0-3").header(reqwest::header::ACCEPT_ENCODING, "identity").send().await {
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
                            let rows = stmt
                                .query_map([], |row| {
                                    Ok((
                                        row.get::<_, String>(0)?,
                                        row.get::<_, String>(1)?,
                                        row.get::<_, String>(2)?,
                                    ))
                                })
                                .ok();

                            if let Some(rows) = rows {
                                for row in rows.flatten() {
                                    let (name, value, host) = row;
                                    let cookie_domain = host.trim_start_matches('.').to_lowercase();
                                    if target_host == cookie_domain
                                        || target_host.ends_with(&format!(".{}", cookie_domain))
                                    {
                                        all_cookies.push(format!("{}={}", name, value));
                                    }
                                }
                            }
                            println!(
                                "[Firefox] Extracted {} relevant cookies from this profile.",
                                all_cookies.len()
                            );
                        }
                    }
                    let _ = fs::remove_file(temp_db);
                }
            }
        }
    }

    if all_cookies.is_empty() {
        None
    } else {
        Some(all_cookies.join("; "))
    }
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
            let cookie_str = cookies
                .iter()
                .filter(|c| {
                    let cookie_domain = c.domain.trim_start_matches('.').to_lowercase();
                    target_host == cookie_domain || target_host.ends_with(&format!(".{}", cookie_domain))
                })
                .map(|c| format!("{}={}", c.name, c.value))
                .collect::<Vec<_>>()
                .join("; ");

            if cookie_str.is_empty() {
                None
            } else {
                Some(cookie_str)
            }
        }
        Err(e) => {
            eprintln!("Failed to extract cookies from {}: {}", browser, e);
            None
        }
    }
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
    size: Option<u64>,
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
        size: size.unwrap_or(0) as i64,
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
    db::log_event(
        &db_state.path,
        &download.id,
        "created",
        Some(if start_paused.unwrap_or(false) {
            "HTTP download added (Scheduled/Paused)"
        } else if should_queue {
            "HTTP download queued (concurrent limit reached)"
        } else {
            "HTTP download initiated"
        }),
    )
    .ok();

    // Only start if not paused and not queued
    if !start_paused.unwrap_or(false) && !should_queue {
        start_download_task(app, db_state.path.clone(), manager.inner().clone(), download.clone()).await?;
    }

    Ok(download)
}

/// Internal: Spawns the long-running async task for an HTTP download.
///
/// It sets up:
/// - Real-time progress emission via Tauri events.
/// - Graceful cancellation handling.
/// - Database persistence of progress and final status.
/// - OS-level notifications on completion/failure.
pub(super) async fn start_download_task<R: Runtime>(
    app: AppHandle<R>,
    db_path: String,
    manager: DownloadManager,
    download: Download,
) -> Result<(), String> {
    let id = download.id.clone();
    let url = download.url.clone();
    let filepath = download.filepath.clone();
    let filename = download.filename.clone(); // Clone filename for use in tokio::spawn
    let known_single_connection = download.metadata.as_deref() == Some("http_no_range");
    let connections = if known_single_connection {
        1
    } else {
        download.connections as u8
    };

    // Create cancellation channel and signal
    let (tx, mut rx) = mpsc::channel(1);
    let is_cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));

    // Fetch global speed limit
    let speed_limit = db::get_setting(&db_path, "speed_limit")
        .ok()
        .flatten()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    let force_multi_http = db::get_setting(&db_path, "force_multi_http")
        .ok()
        .flatten()
        .map(|v| v == "true")
        .unwrap_or(false);

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
            force_multi: force_multi_http && !known_single_connection,
            size_hint: if download.size > 0 { Some(download.size as u64) } else { None },
        };

        if known_single_connection {
            println!(
                "[{}] Known single-connection HTTP source. Skipping parallel mode.",
                id
            );
        }

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
                        let err_msg = e.to_string();
                        set_and_emit_download_error(&app, &db_path_inner, &id_inner, &err_msg);

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




