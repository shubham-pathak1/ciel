//! System Tray Integration
//! 
//! This module provides the system tray (notification area) implementation.
//! It allows the application to remain active and accessible even when
//! the main window is hidden.

use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, Runtime,
};
use crate::scheduler;

/// Bootstraps the system tray icon, context menu, and event handlers.
/// 
/// The tray includes:
/// - "Show Ciel": Restores and focuses the main window.
/// - "Quit": Completely exits the application.
/// - Left-click handler: Conveniently toggles window visibility.
pub fn create_tray<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    // Define context menu items
    let summary_i = MenuItem::with_id(app, "summary", "📥 0 Active • 0 B/s", false, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let pause_all_i = MenuItem::with_id(app, "pause_all", "Pause All", true, None::<&str>)?;
    let resume_all_i = MenuItem::with_id(app, "resume_all", "Resume All", true, None::<&str>)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let show_i = MenuItem::with_id(app, "show", "Show Ciel", true, None::<&str>)?;
    let quit_i = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    
    let menu = Menu::with_items(app, &[
        &summary_i, 
        &sep1, 
        &pause_all_i, 
        &resume_all_i, 
        &sep2, 
        &show_i, 
        &quit_i
    ])?;

    // Background loop to update the tray summary in real-time
    let app_handle = app.clone();
    let summary_clone = summary_i.clone();
    
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            
            let manager = app_handle.state::<crate::commands::DownloadManager>();
            let torrent_manager = app_handle.state::<crate::torrent::TorrentManager>();
            
            let (h_count, h_speed) = manager.get_global_status().await;
            let (t_count, t_speed) = torrent_manager.get_global_status().await;
            
            let total_count = h_count + t_count;
            let total_speed = h_speed + t_speed;
            
            let speed_text = format_speed(total_speed);
            let text = format!("📥 {} Active • {}", total_count, speed_text);
            
            let _ = summary_clone.set_text(text);
        }
    });

    let _ = TrayIconBuilder::<R>::with_id("main")
        .tooltip("Ciel Download Manager")
        .icon(app.default_window_icon().unwrap().clone())
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| {
            let app_handle = app.clone();
            match event.id.as_ref() {
                "quit" => app.exit(0),
                "show" => {
                    if let Some(window) = app.get_webview_window("main") {
                        let _ = window.show();
                        let _ = window.set_focus();
                    }
                }
                "pause_all" => {
                    tauri::async_runtime::spawn(async move {
                        scheduler::pause_all_downloads(&app_handle).await;
                    });
                }
                "resume_all" => {
                    tauri::async_runtime::spawn(async move {
                        scheduler::resume_all_downloads(&app_handle).await;
                    });
                }
                _ => {}
            }
        })
        .on_tray_icon_event(|tray, event| match event {
            // Restore window on simple left click
            TrayIconEvent::Click {
                button: MouseButton::Left,
                ..
            } => {
                let app = tray.app_handle();
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
            _ => {}
        })
        .build(app)?;

    Ok(())
}

/// Helper: Formats bytes per second into a human-readable string (e.g. 5.2 MB/s).
fn format_speed(bps: u64) -> String {
    if bps == 0 {
        return "0 B/s".to_string();
    }
    
    if bps < 1024 {
        format!("{} B/s", bps)
    } else if bps < 1024 * 1024 {
        format!("{:.1} KB/s", bps as f64 / 1024.0)
    } else if bps < 1024 * 1024 * 1024 {
        format!("{:.1} MB/s", bps as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.2} GB/s", bps as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}
