use crate::db::{self, DbState, Download, DownloadStatus, DownloadProtocol};
use crate::downloader::{Downloader, DownloadConfig};
use crate::torrent::TorrentManager;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use crate::torrent::TorrentFile;
use reqwest::header::RANGE;
use std::convert::TryInto;
use serde::Serialize;
use flate2::read::DeflateDecoder;
use std::io::Read;
use std::fs::File;
use std::io::Write;

#[derive(Serialize)]
pub struct ZipPreviewInfo {
    pub id: String,
    pub name: String,
    pub total_size: u64,
    pub files: Vec<TorrentFile>,
}


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
}

// ... types for validation ...
#[derive(serde::Serialize)]
pub struct UrlTypeInfo {
    is_magnet: bool,
    content_type: Option<String>,
    content_length: Option<u64>,
    resolved_url: Option<String>,
    hinted_filename: Option<String>,
}

#[tauri::command]
pub async fn validate_url_type(url: String) -> Result<UrlTypeInfo, String> {
    if url.starts_with("magnet:") {
        return Ok(UrlTypeInfo {
            is_magnet: true,
            content_type: None,
            content_length: None,
            resolved_url: None,
            hinted_filename: None,
        });
    }

    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        .build()
        .unwrap_or_default();

    let mut final_url = url.clone();
    let mut resolved_link = None;

    // Smart Google Drive Resolution
    if let Some(resolved) = resolve_google_drive_link(&client, &url).await {
        final_url = resolved.clone();
        resolved_link = Some(resolved);
    }

    // If it's Drive, we MUST use GET because HEAD often returns 405/403 on the confirmation URL
    let response = if resolved_link.is_some() {
        client.get(&final_url).send().await.map_err(|e| e.to_string())?
    } else {
        client.head(&final_url).send().await.map_err(|e| e.to_string())?
    };
    
    let headers = response.headers();
    let content_type = headers.get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
        
    let content_length = headers.get(reqwest::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok());

    let mut hinted_filename = headers.get(reqwest::header::CONTENT_DISPOSITION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| {
            // Extract filename from "attachment; filename=\"...\""
            let re = regex::Regex::new(r#"filename\s*=\s*(?:"([^"]+)"|([^;]+))"#).ok()?;
            let caps = re.captures(s)?;
            caps.get(1).or(caps.get(2)).map(|m| m.as_str().to_string())
        });

    // Fallback: If it's a Drive link and filename is missing or generic (like 'view.html')
    // we can scrape the <title> tag from the confirmation page.
    if resolved_link.is_some() && (hinted_filename.is_none() || hinted_filename.as_deref() == Some("view.html")) {
        if let Ok(text) = response.text().await {
            let title_re = regex::Regex::new(r"(?i)<title>(.*?)</title>").ok();
            if let Some(caps) = title_re.and_then(|re| re.captures(&text)) {
                let mut title = caps.get(1).map(|m| m.as_str().to_string()).unwrap_or_default();
                // Drive Titles are often "Filename - Google Drive"
                title = title.replace(" - Google Drive", "").trim().to_string();
                if !title.is_empty() && !title.contains("Google Drive") && !title.contains("Virus scan warning") {
                    hinted_filename = Some(title);
                }
            }
        }
    }

    Ok(UrlTypeInfo {
        is_magnet: false,
        content_type,
        content_length,
        resolved_url: resolved_link,
        hinted_filename,
    })
}

/// Helper to resolve Google Drive links into direct download links
async fn resolve_google_drive_link(client: &reqwest::Client, url: &str) -> Option<String> {
    if !url.contains("drive.google.com") {
        return None;
    }

    // Extract file ID using regex
    // Formats: /file/d/ID/..., /id=ID, /open?id=ID
    let id_regex = regex::Regex::new(r"/(?:file/d/|id=|open\?id=)([a-zA-Z0-9_-]{25,})").ok()?;
    let caps = id_regex.captures(url)?;
    let file_id = caps.get(1)?.as_str();

    let direct_url = format!("https://drive.google.com/uc?export=download&id={}", file_id);

    // Some files (large ones) require a confirmation token
    if let Ok(res) = client.get(&direct_url).send().await {
        let text = res.text().await.unwrap_or_default();
        
        // Look for the "confirm" token in the HTML (usually in a form or link)
        let confirm_regex = regex::Regex::new(r#"confirm=([a-zA-Z0-9_-]+)"#).ok()?;
        if let Some(confirm_caps) = confirm_regex.captures(&text) {
            let token = confirm_caps.get(1)?.as_str();
            return Some(format!("{}&confirm={}", direct_url, token));
        }
    }

    Some(direct_url)
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
) -> Result<Download, String> {

    // Get max connections from settings
    let max_connections = db::get_setting(&db_state.path, "max_connections")
        .ok()
        .flatten()
        .and_then(|v| v.parse::<i32>().ok())
        .unwrap_or(8);
    
    let mut filename = filename;
    
    // Check if current filename is "suspicious" (no extension, generic, or looks like a hash)
    let has_ext = filename.contains('.');
    let is_generic = filename == "download" || filename == "download_file" || filename.is_empty();
    let looks_like_hash = filename.len() > 15 && !filename.contains(' ') && !has_ext;

    if (is_generic || looks_like_hash || !has_ext) && url.starts_with("http") {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
            .build()
            .unwrap_or_default();
            
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
    let final_resolved_path = resolve_download_path(&app, &db_state.path, &filename, output_folder);

    let id = uuid::Uuid::new_v4().to_string();
    let download = Download {
        id: id.clone(),
        url: url.clone(),
        filename,
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

    let resolved_path = resolve_download_path(&app, &db_state.path, &filename, output_folder.clone());

    let id = uuid::Uuid::new_v4().to_string();
    let download = Download {
        id: id.clone(),
        url: url.clone(),
        filename,
        filepath: resolved_path.clone(),
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
    };

    db::insert_download(&db_state.path, &download).map_err(|e| e.to_string())?;
    db::log_event(&db_state.path, &download.id, "created", Some("Torrent download initiated")).ok();

    let base_folder = if let Some(folder) = output_folder {
        folder
    } else {
        Path::new(&resolved_path).parent().unwrap_or(Path::new(".")).to_string_lossy().to_string()
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

/// Preview ZIP contents without full download
#[tauri::command]
pub async fn preview_zip(url: String) -> Result<ZipPreviewInfo, String> {
    let client = reqwest::Client::new();

    // 1. Get Content-Length
    let head_resp = client.head(&url).send().await
        .map_err(|e| format!("Failed to HEAD url: {}", e))?;
    
    let content_len = head_resp.content_length()
        .ok_or("Server did not report Content-Length")?;

    // 2. Fetch Tail (End of Central Directory)
    // EOCD is min 22 bytes. Comment can be up to 65535 bytes.
    // Safe bet: last 65536 + 22 bytes, or full file if smaller.
    let tail_size = 65557.min(content_len);
    let start_range = content_len - tail_size;
    
    let tail_resp = client.get(&url)
        .header(RANGE, format!("bytes={}-{}", start_range, content_len - 1))
        .send().await
        .map_err(|e| format!("Failed to fetch tail: {}", e))?;

    let tail_bytes = tail_resp.bytes().await
        .map_err(|e| format!("Failed to read tail bytes: {}", e))?;

    // 3. Find EOCD Signature: 0x06054b50
    // Scan backwards
    let mut eocd_offset_in_tail = None;
    for i in (0..tail_bytes.len().saturating_sub(3)).rev() {
        if tail_bytes[i] == 0x50 && tail_bytes[i+1] == 0x4b && tail_bytes[i+2] == 0x05 && tail_bytes[i+3] == 0x06 {
            eocd_offset_in_tail = Some(i);
            break;
        }
    }

    let eocd_idx = eocd_offset_in_tail.ok_or("Not a valid ZIP (EOCD not found)")?;

    // Check valid EOCD
    if tail_bytes.len() < eocd_idx + 22 {
        return Err("Truncated EOCD".to_string());
    }

    // Parse EOCD (Little Endian)
    // Offset 10 (2 bytes): Total entries
    // Offset 12 (4 bytes): Size of CD
    // Offset 16 (4 bytes): Offset of CD
    
    let cd_size = u32::from_le_bytes(tail_bytes[eocd_idx+12..eocd_idx+16].try_into().unwrap()) as u64;
    let cd_offset = u32::from_le_bytes(tail_bytes[eocd_idx+16..eocd_idx+20].try_into().unwrap()) as u64;

    // TODO: Support Zip64 if cd_size/offset are 0xFFFFFFFF
    if cd_size == 0xFFFFFFFF || cd_offset == 0xFFFFFFFF {
        return Err("Zip64 not yet supported for remote preview".to_string());
    }

    // 4. Fetch Central Directory
    // Note: cd_offset is absolute from start of file.
    let cd_bytes = if cd_offset >= start_range && (cd_offset + cd_size) <= content_len {
        // CD is already in our tail buffer!
        let start_in_tail = (cd_offset - start_range) as usize;
        let end_in_tail = (cd_offset - start_range + cd_size) as usize;
        if end_in_tail > tail_bytes.len() {
             return Err("Central Directory bounds error in tail".to_string());
        }
        tail_bytes.slice(start_in_tail..end_in_tail)
    } else {
        // Need to fetch CD separately
        let cd_resp = client.get(&url)
            .header(RANGE, format!("bytes={}-{}", cd_offset, cd_offset + cd_size - 1))
            .send().await
            .map_err(|e| format!("Failed to fetch Central Directory: {}", e))?;
        
        cd_resp.bytes().await
            .map_err(|e| format!("Failed to read CD bytes: {}", e))?
    };

    // 5. Parse Central Directory Entries
    let mut files = Vec::new();
    let mut cursor = 0;
    let mut index = 0;

    // CD Signature: 0x02014b50
    while cursor + 46 <= cd_bytes.len() {
        if cd_bytes[cursor] != 0x50 || cd_bytes[cursor+1] != 0x4b || cd_bytes[cursor+2] != 0x01 || cd_bytes[cursor+3] != 0x02 {
            break; // End of CD headers or invalid
        }

        // Offset 20: Compressed size (4)
        // Offset 24: Uncompressed size (4)
        // Offset 28: Filename len (2)
        // Offset 30: Extra len (2)
        // Offset 32: Comment len (2)
        
        let uncompressed_size = u32::from_le_bytes(cd_bytes[cursor+24..cursor+28].try_into().unwrap()) as u64;
        let filename_len = u16::from_le_bytes(cd_bytes[cursor+28..cursor+30].try_into().unwrap()) as usize;
        let extra_len = u16::from_le_bytes(cd_bytes[cursor+30..cursor+32].try_into().unwrap()) as usize;
        let comment_len = u16::from_le_bytes(cd_bytes[cursor+32..cursor+34].try_into().unwrap()) as usize;

        let filename_start = cursor + 46;
        if filename_start + filename_len > cd_bytes.len() {
            files.push(TorrentFile {
                name: "Truncated Filename".to_string(),
                size: uncompressed_size,
                index,
            });
            break; 
        }

        let filename_bytes = &cd_bytes[filename_start..filename_start+filename_len];
        let filename = String::from_utf8_lossy(filename_bytes).to_string();

        files.push(TorrentFile {
            name: filename,
            size: uncompressed_size,
            index,
        });

        index += 1;
        cursor = filename_start + filename_len + extra_len + comment_len;
    }

    // Attempt to extract filename from URL or header? For info name.
    let url_parsed = url::Url::parse(&url).map_err(|_| "Invalid URL")?;
    let display_name = url_parsed.path_segments()
        .and_then(|s| s.last())
        .map(|s| percent_encoding::percent_decode_str(s).decode_utf8_lossy().to_string())
        .unwrap_or("archive.zip".to_string());

    Ok(ZipPreviewInfo {
        id: "zip_preview".to_string(), 
        name: display_name,
        total_size: content_len,
        files,
    })
}

#[tauri::command]
pub async fn download_zip_selection(
    app: AppHandle,
    url: String,
    indices: Vec<usize>,
    output_folder: Option<String>,
) -> Result<(), String> {
    let client = reqwest::Client::new();

    // Resolve output folder
    let target_dir = if let Some(f) = output_folder {
        PathBuf::from(f)
    } else {
        app.path().download_dir().map_err(|e| e.to_string())?
    };
    let output_folder_str = target_dir.to_string_lossy().to_string();

    // 1. Get Content-Length
    let head_resp = client.head(&url).send().await
        .map_err(|e| format!("Failed to HEAD url: {}", e))?;
    let content_len = head_resp.content_length()
        .ok_or("Server did not report Content-Length")?;

    // 2. Fetch Tail to parse CD (Same logic as preview, could be shared but dup for now)
    let tail_size = 65557.min(content_len);
    let start_range = content_len - tail_size;
    let tail_resp = client.get(&url)
        .header(RANGE, format!("bytes={}-{}", start_range, content_len - 1))
        .send().await
        .map_err(|e| format!("Failed to fetch tail: {}", e))?;
    let tail_bytes = tail_resp.bytes().await
        .map_err(|e| format!("Failed to read tail bytes: {}", e))?;

    let mut eocd_offset_in_tail = None;
    for i in (0..tail_bytes.len().saturating_sub(3)).rev() {
        if tail_bytes[i] == 0x50 && tail_bytes[i+1] == 0x4b && tail_bytes[i+2] == 0x05 && tail_bytes[i+3] == 0x06 {
            eocd_offset_in_tail = Some(i);
            break;
        }
    }
    let eocd_idx = eocd_offset_in_tail.ok_or("Not a valid ZIP (EOCD not found)")?;
    
    let cd_size = u32::from_le_bytes(tail_bytes[eocd_idx+12..eocd_idx+16].try_into().unwrap()) as u64;
    let cd_offset = u32::from_le_bytes(tail_bytes[eocd_idx+16..eocd_idx+20].try_into().unwrap()) as u64;

    let cd_bytes = if cd_offset >= start_range && (cd_offset + cd_size) <= content_len {
        let start_in_tail = (cd_offset - start_range) as usize;
        let end_in_tail = (cd_offset - start_range + cd_size) as usize;
        tail_bytes.slice(start_in_tail..end_in_tail)
    } else {
        let cd_resp = client.get(&url)
            .header(RANGE, format!("bytes={}-{}", cd_offset, cd_offset + cd_size - 1))
            .send().await
            .map_err(|e| format!("Failed to fetch Central Directory: {}", e))?;
        cd_resp.bytes().await.map_err(|e| format!("Failed to read CD: {}", e))?
    };

    // 3. Process Selected Files
    let mut cursor = 0;
    let mut current_index = 0;

    while cursor + 46 <= cd_bytes.len() {
        if cd_bytes[cursor] != 0x50 || cd_bytes[cursor+1] != 0x4b || cd_bytes[cursor+2] != 0x01 || cd_bytes[cursor+3] != 0x02 {
            break; 
        }

        let compression_method = u16::from_le_bytes(cd_bytes[cursor+10..cursor+12].try_into().unwrap());
        let compressed_size = u32::from_le_bytes(cd_bytes[cursor+20..cursor+24].try_into().unwrap()) as u64;
        let filename_len = u16::from_le_bytes(cd_bytes[cursor+28..cursor+30].try_into().unwrap()) as usize;
        let extra_len = u16::from_le_bytes(cd_bytes[cursor+30..cursor+32].try_into().unwrap()) as usize;
        let comment_len = u16::from_le_bytes(cd_bytes[cursor+32..cursor+34].try_into().unwrap()) as usize;
        let local_header_offset = u32::from_le_bytes(cd_bytes[cursor+42..cursor+46].try_into().unwrap()) as u64;

        let filename_start = cursor + 46;
        let filename = String::from_utf8_lossy(&cd_bytes[filename_start..filename_start+filename_len]).to_string();

        if indices.contains(&current_index) {
            // Process this file
            if compression_method != 0 && compression_method != 8 {
                 println!(" Skipping {} - unsupported compression {}", filename, compression_method);
            } else {
                 // Fetch Local File Header to find actual data start
                 // LFH is 30 bytes + filename + extra
                 let lfh_resp = client.get(&url)
                    .header(RANGE, format!("bytes={}-{}", local_header_offset, local_header_offset + 30 + 1024)) // Fetch enough for headers
                    .send().await
                    .map_err(|e| format!("Failed to fetch LFH: {}", e))?;
                let lfh_bytes = lfh_resp.bytes().await.map_err(|e| format!("Failed to read LFH: {}", e))?;

                if lfh_bytes[0] != 0x50 || lfh_bytes[1] != 0x4b || lfh_bytes[2] != 0x03 || lfh_bytes[3] != 0x04 {
                    return Err(format!("Invalid LFH for {}", filename));
                }
                
                let lfh_filename_len = u16::from_le_bytes(lfh_bytes[26..28].try_into().unwrap()) as u64;
                let lfh_extra_len = u16::from_le_bytes(lfh_bytes[28..30].try_into().unwrap()) as u64;
                
                let data_start = local_header_offset + 30 + lfh_filename_len + lfh_extra_len;
                
                // Fetch Compressed Data
                let data_resp = client.get(&url)
                    .header(RANGE, format!("bytes={}-{}", data_start, data_start + compressed_size - 1))
                    .send().await
                    .map_err(|e| format!("Failed to fetch data for {}: {}", filename, e))?;
                
                // Stream response to file
                let content = data_resp.bytes().await.map_err(|e| format!("Failed to download content: {}", e))?;
                
                // Write output
                let out_path = Path::new(&output_folder_str).join(&filename);
                if let Some(parent) = out_path.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }

                let mut out_file = File::create(&out_path).map_err(|e| format!("Failed to create file: {}", e))?;

                if compression_method == 8 {
                    // Deflate
                    let mut decoder = DeflateDecoder::new(&content[..]);
                    let mut buffer = Vec::new(); // Memory heavy for large files, but OK for MVP
                    decoder.read_to_end(&mut buffer).map_err(|e| format!("Decompression failed: {}", e))?;
                    out_file.write_all(&buffer).map_err(|e| format!("Write failed: {}", e))?;
                } else {
                    // Store
                    out_file.write_all(&content).map_err(|e| format!("Write failed: {}", e))?;
                }
            }
        }

        current_index += 1;
        cursor = filename_start + filename_len + extra_len + comment_len;
    }

    Ok(())
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
    let connections = download.connections as u8;

    // Create cancellation channel and signal
    let (tx, mut rx) = mpsc::channel(1);
    let is_cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    
    manager.add_active(id.clone(), tx).await;

    // Spawn download in background
    tokio::spawn(async move {
        let config = DownloadConfig {
            id: id.clone(),
            url,
            filepath: PathBuf::from(filepath),
            connections,
            chunk_size: 5 * 1024 * 1024,
            speed_limit: 0,
        };

        let downloader = Downloader::new(config)
            .with_db(db_path.clone())
            .with_cancel_signal(is_cancelled.clone()); // Pass signal

        let id_inner = id.clone();
        let db_path_inner = db_path.clone();
        let app_clone = app.clone();

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
                    }
                    Err(e) => {
                        let _ = db::update_download_status(&db_path_inner, &id_inner, DownloadStatus::Error);
                        let _ = app.emit("download-error", (id_inner.clone(), e.to_string()));
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
        .unwrap_or(8);
    
    download.connections = max_connections;

    if download.status == DownloadStatus::Completed {
        return Err("Download already completed".to_string());
    }

    db::update_download_status(&db_state.path, &id, DownloadStatus::Downloading).map_err(|e| e.to_string())?;
    db::log_event(&db_state.path, &id, "resumed", None).ok();

    if download.protocol == DownloadProtocol::Torrent {
        torrent_manager.resume_torrent(&id).await?;
    } else {
        start_download_task(app, db_state.path.clone(), manager.inner().clone(), download.clone()).await?;
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
pub fn show_in_folder(path: String) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let path_norm = path.replace("/", "\\");
        let p = Path::new(&path_norm);
        println!("Opening folder for: {}", path_norm);
        
        if p.exists() {
            // If file exists, try to select it
            let _ = std::process::Command::new("explorer.exe")
                .arg(format!("/select,{}", path_norm))
                .spawn();
        } else {
            // If the specific file doesn't exist, try opening its parent directory
            if let Some(parent) = p.parent() {
                println!("File not found, opening parent: {:?}", parent);
                let _ = std::process::Command::new("explorer.exe")
                    .arg(parent)
                    .spawn();
            } else {
                // Last ditch effort: open current dir if parent is somehow missing
                let _ = std::process::Command::new("explorer.exe")
                    .arg(".")
                    .spawn();
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
