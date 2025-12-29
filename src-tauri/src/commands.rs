use crate::db::{self, DbState, Download, DownloadStatus, DownloadProtocol};
use crate::downloader::{Downloader, DownloadConfig};
use crate::torrent::TorrentManager;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use directories::UserDirs;

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

/// Helper to resolve authentic filepath
fn resolve_download_path(db_path: &str, provided_path: &str) -> String {
    let path = Path::new(provided_path);
    if path.is_absolute() {
        return provided_path.to_string();
    }

    // Get configured download path
    let configured_path = db::get_setting(db_path, "download_path")
        .unwrap_or(None)
        .unwrap_or_default();

    let base_dir = if !configured_path.is_empty() {
        let path = PathBuf::from(&configured_path);
        if path.is_absolute() {
            path
        } else {
            // Resolve relative path against current directory or executable location
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")).join(path)
        }
    } else {
        UserDirs::new()
            .and_then(|dirs| dirs.download_dir().map(|d| d.to_path_buf()))
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    };

    // If provided path is simply a filename or relative like ./file
    let file_name = path.file_name().unwrap_or_default();
    base_dir.join(file_name).to_string_lossy().to_string()
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
    filepath: String,
) -> Result<Download, String> {
    let resolved_path = resolve_download_path(&db_state.path, &filepath);

    // Get max connections from settings
    let max_connections = db::get_setting(&db_state.path, "max_connections")
        .ok()
        .flatten()
        .and_then(|v| v.parse::<i32>().ok())
        .unwrap_or(8);
    
    let id = uuid::Uuid::new_v4().to_string();
    let download = Download {
        id: id.clone(),
        url: url.clone(),
        filename,
        filepath: resolved_path,
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
    filename: String,
    filepath: String,
) -> Result<Download, String> {
    let resolved_path = resolve_download_path(&db_state.path, &filepath);

    let id = uuid::Uuid::new_v4().to_string();
    let download = Download {
        id: id.clone(),
        url: url.clone(),
        filename,
        filepath: resolved_path.clone(),
        size: 0,
        downloaded: 0,
        status: DownloadStatus::Downloading,
        protocol: DownloadProtocol::Torrent,
        speed: 0,
        connections: 0,
        created_at: chrono::Utc::now().to_rfc3339(),
        completed_at: None,
        error_message: None,
        info_hash: None, // We'll update this once rqbit gives us the infohash if needed
    };

    db::insert_download(&db_state.path, &download).map_err(|e| e.to_string())?;
    db::log_event(&db_state.path, &download.id, "created", Some("Torrent download initiated")).ok();

    torrent_manager.add_magnet(app, id, url, resolved_path).await?;

    Ok(download)
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
        std::process::Command::new("explorer")
            .args(["/select,", &path]) // Comma is important
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .args(["-R", &path])
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "linux")]
    {
        // Try xdg-open (might just open folder, not select)
        // Or implement specific file managers like nautilus, dolphin
        std::process::Command::new("xdg-open")
            .arg(std::path::Path::new(&path).parent().unwrap_or(std::path::Path::new("/")))
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
