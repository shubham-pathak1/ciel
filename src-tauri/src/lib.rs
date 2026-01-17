//! Ciel Download Manager - Library Root
//!
//! This module exposes the core functionality of Ciel:
//! - Database operations (downloads, settings, history)
//! - Download engine (HTTP multi-connection downloads)
//! - Tauri commands for IPC

pub mod commands;
pub mod db;
pub mod downloader;
mod torrent;
mod video;
mod scheduler;
pub mod clipboard;
pub mod tray;

use tauri::Manager;

/// Initialize and run the Tauri application
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // Initialize the database
            let app_handle = app.handle().clone();
            let db_path = app_handle
                .path()
                .app_data_dir()
                .expect("Failed to get app data dir")
                .join("ciel.db");

            // Ensure parent directory exists
            if let Some(parent) = db_path.parent() {
                std::fs::create_dir_all(parent).ok();
            }

            // Initialize database
            db::init_db(&db_path).expect("Failed to initialize database");

            // Store db path in app state
            app.manage(db::DbState {
                path: db_path.to_string_lossy().to_string(),
            });

            // Initialize and manage DownloadManager
            app.manage(commands::DownloadManager::new());
            
            // Read torrent encryption setting
            let force_encryption = db::get_setting(&db_path, "torrent_encryption")
                .ok()
                .flatten()
                .map(|v| v == "true")
                .unwrap_or(false);

            // Initialize and manage TorrentManager
            let torrent_manager = tauri::async_runtime::block_on(torrent::TorrentManager::new(force_encryption));
            app.manage(torrent_manager);

            // Apply window effects (vibrancy/acrylic on supported platforms)
            {
                if let Some(window) = app.get_webview_window("main") {
                    use window_vibrancy::apply_mica;
                    let _ = apply_mica(&window, Some(true));
                }
            }

            // Initialize System Tray
            tray::create_tray(app.handle()).expect("Failed to create tray");

            // Initialize Notifications
            app.handle().plugin(tauri_plugin_notification::init())?;

            // Initialize Clipboard Monitor
            clipboard::start_clipboard_monitor(app.handle().clone());
            scheduler::start_scheduler(app.handle().clone());
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                // Minimize to tray instead of quitting
                window.hide().unwrap();
                api.prevent_close();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_downloads,
            commands::add_download,
            commands::add_torrent,
            commands::analyze_torrent,
            video::analyze_video_url,
            video::add_video_download,
            commands::validate_url_type,
            commands::start_selective_torrent,
            commands::pause_download,
            commands::resume_download,
            commands::delete_download,
            commands::get_history,
            commands::get_download_events,
            commands::get_settings,
            commands::update_setting,
            commands::show_in_folder,
            commands::clear_finished,
            clipboard::get_clipboard,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
