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

pub mod commands;
pub mod db;
pub mod downloader;
mod torrent;
mod scheduler;
pub mod clipboard;
pub mod tray;

use tauri::Manager;
use tauri::Listener;

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
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            // Window may have been destroyed to save RAM, so we recreate if needed
            tray::show_or_create_window(app);
        }))
        .setup(|app| {
            let app_handle = app.handle().clone();
            
            // 1. Resolve Paths (CPU only - very fast)
            let app_data_path = app_handle.path().app_data_dir()
                .map_err(|e| format!("Failed to get app data dir: {}", e))?;
            let db_path = app_data_path.join("ciel.db");
            let torrent_session_dir = app_data_path.join("torrents");

            // 2. Immediate State Management (Zero I/O)
            app.manage(db::DbState {
                path: db_path.to_string_lossy().to_string(),
            });
            app.manage(commands::DownloadManager::new());
            
            // Start TorrentManager with "Optimistic" defaults.
            // It will warm up its engine in its own background task.
            let torrent_manager = torrent::TorrentManager::new(torrent_session_dir, false);
            app.manage(torrent_manager);

            // 3. WINDOW DECORATION (Sync - Cheap Win32 calls)
            if let Some(window) = app.get_webview_window("main") {
                #[cfg(target_os = "windows")]
                {
                    use window_vibrancy::apply_mica;
                    let _ = apply_mica(&window, Some(true));
                }
            }

            // 4. BACKGROUND WARMUP (All heavy I/O goes here)
            let handle = app_handle.clone();
            let db_path_clone = db_path.clone();
            tauri::async_runtime::spawn(async move {
                // Ensure directories exist
                if let Some(parent) = db_path_clone.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }

                // Database migrations and Tray/Clipboard/Scheduler
                let _ = db::init_db(&db_path_clone);
                let _ = tray::create_tray(&handle);
                clipboard::start_clipboard_monitor(handle.clone());
                scheduler::start_scheduler(handle.clone());
                
                // Note: The torrent engine has its own background init in TorrentManager::new
            });

            // QUEUE MANAGEMENT
            // Listen for completion/error events to trigger the queue processor
            let handle = app.handle().clone();
            app.listen("download-completed", move |_| {
                let handle_clone = handle.clone();
                tauri::async_runtime::spawn(async move {
                    commands::process_queue(handle_clone).await;
                });
            });

            let handle = app.handle().clone();
            app.listen("download-error", move |_| {
                let handle_clone = handle.clone();
                tauri::async_runtime::spawn(async move {
                    commands::process_queue(handle_clone).await;
                });
            });

            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                // RAM OPTIMIZATION: Destroy the webview entirely instead of hiding it.
                // This drops WebView2 memory from ~200MB to near zero.
                // The window will be recreated when the user clicks the tray icon.
                let _ = window.destroy();
                api.prevent_close();
            }
        })
        .invoke_handler(tauri::generate_handler![
            // Registration of all commands exposed via tauri.invoke()
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
            commands::clear_finished,
            clipboard::get_clipboard,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
