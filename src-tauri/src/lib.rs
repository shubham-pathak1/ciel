//! Ciel Download Manager - Library Root
//!
//! This module exposes the core functionality of Ciel:
//! - Database operations (downloads, settings, history)
//! - Download engine (HTTP multi-connection downloads)
//! - Tauri commands for IPC

pub mod commands;
pub mod db;
pub mod downloader;
pub mod torrent;

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

            // Initialize and manage TorrentManager
            let torrent_manager = tauri::async_runtime::block_on(torrent::TorrentManager::new());
            app.manage(torrent_manager);

            // Apply window effects (vibrancy/acrylic on supported platforms)
            #[cfg(target_os = "windows")]
            {
                if let Some(window) = app.get_webview_window("main") {
                    use window_vibrancy::apply_mica;
                    let _ = apply_mica(&window, Some(true));
                }
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_downloads,
            commands::add_download,
            commands::add_torrent,
            commands::analyze_torrent,
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
