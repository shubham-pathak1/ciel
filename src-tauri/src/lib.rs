//! Ciel Download Manager - Core Library Root
//!
//! This module serves as the central orchestration point for the Ciel application.
//! It handles the initialization of all major subsystems and integrates them
//! into the Tauri application lifecycle.
//!
//! Subsystems included:
//! - **Database (`db`)**: SQLite-based persistence for downloads and settings.
//! - **Commands (`commands`)**: The bridge between the Frontend and Backend logic.
//! - **Downloader (`downloader`)**: Multi-connection HTTP download engine.
//! - **Torrent (`torrent`)**: BitTorrent protocol support via `librqbit`.
//! - **Video (`video`)**: Specialized handling for YouTube and other video platforms.
//! - **Tray (`tray`) & Clipboard (`clipboard`)**: OS-level integrations for better UX.

pub mod bin_resolver;
pub mod commands;
pub mod db;
pub mod downloader;
mod torrent;
mod video;
mod scheduler;
pub mod clipboard;
pub mod tray;

use tauri::Manager;

/// The primary entry point to initialize and launch the Ciel application.
/// 
/// This function:
/// 1. Bootstraps the database and runs necessary migrations.
/// 2. Initializes the HTTP and Torrent download managers.
/// 3. Sets up system-level hooks (Tray, Clipboard Monitor, Scheduler).
/// 4. Registers all IPC commands accessible from the frontend.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // DATABASE INITIALIZATION
            // We resolve the app data directory to store the SQLite database.
            let app_handle = app.handle().clone();
            let db_path = app_handle
                .path()
                .app_data_dir()
                .expect("Failed to get app data dir")
                .join("ciel.db");

            // Ensure parent directory exists before opening the connection
            if let Some(parent) = db_path.parent() {
                std::fs::create_dir_all(parent).ok();
            }

            // init_db creates tables and handles schema migrations
            db::init_db(&db_path).expect("Failed to initialize database");

            // Store db path in app state for easy access in Tauri commands
            app.manage(db::DbState {
                path: db_path.to_string_lossy().to_string(),
            });

            // STATE MANAGEMENT
            // Initialize the HTTP download manager
            app.manage(commands::DownloadManager::new());
            
            // Resolve torrent settings before initializing the engine
            let force_encryption = db::get_setting(&db_path, "torrent_encryption")
                .ok()
                .flatten()
                .map(|v| v == "true")
                .unwrap_or(false);

            // Initialize the BitTorrent session (async)
            let torrent_manager = tauri::async_runtime::block_on(torrent::TorrentManager::new(force_encryption))
                .expect("Failed to initialize TorrentManager struct");
            app.manage(torrent_manager);

            // WINDOW DECORATION
            // Apply Mica effect on Windows for a modern, glassmorphic look.
            {
                if let Some(window) = app.get_webview_window("main") {
                    use window_vibrancy::apply_mica;
                    let _ = apply_mica(&window, Some(true));
                }
            }

            // OS INTEGRATIONS
            tray::create_tray(app.handle()).expect("Failed to create tray");
            app.handle().plugin(tauri_plugin_notification::init())?;
            clipboard::start_clipboard_monitor(app.handle().clone());
            scheduler::start_scheduler(app.handle().clone());

            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                // UX: Instead of exiting, minimize the app to the system tray.
                // This keeps active downloads running in the background.
                window.hide().unwrap();
                api.prevent_close();
            }
        })
        .invoke_handler(tauri::generate_handler![
            // Registration of all commands exposed via tauri.invoke()
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
