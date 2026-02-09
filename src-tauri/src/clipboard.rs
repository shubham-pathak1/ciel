//! Clipboard Monitoring Module
//! 
//! This module implements the "Auto-Catch" feature, which monitors the system 
//! clipboard for magnet links or downloadable URLs and notifies the frontend.

use arboard::Clipboard;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, Runtime};
use crate::db;

/// Starts a background loop that polls the clipboard every second.
/// 
/// It implements:
/// - **Setting Polling**: Checks the `autocatch_enabled` setting every 5 seconds.
/// - **Deduplication**: Only emits the `autocatch-url` event if the clipboard 
///   content has changed since the last catch.
/// - **URL Validation**: Heuristically determines if the clipboard contains a 
///   link relevant to Ciel.
pub fn start_clipboard_monitor<R: Runtime>(app: AppHandle<R>) {
    tauri::async_runtime::spawn(async move {
        let mut clipboard = Clipboard::new().ok();
        let mut last_clipboard = String::new();

        let mut last_settings_check = std::time::Instant::now() - Duration::from_secs(10);
        let mut cached_enabled = true;

        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;

            // PERFORMANCE: Cache the 'autocatch' setting to avoid redundant DB reads on every tick.
            if last_settings_check.elapsed() > Duration::from_secs(5) {
                let db_state = app.state::<db::DbState>();
                let settings = db::get_all_settings(&db_state.path).unwrap_or_default();
                cached_enabled = settings.get("autocatch_enabled")
                    .map(|v| v == "true")
                    .unwrap_or(true);
                last_settings_check = std::time::Instant::now();
            }

            if !cached_enabled {
                last_clipboard.clear(); 
                continue;
            }

            if let Some(ref mut cb) = clipboard {
                match cb.get_text() {
                    Ok(text) => {
                        let text = text.trim().to_string();
                        if !text.is_empty() && text != last_clipboard {
                            if is_valid_url(&text) {
                                // Inform the frontend that a potential download was found.
                                let _ = app.emit("autocatch-url", &text);
                                last_clipboard = text;
                            }
                        }
                    },
                    Err(_) => {
                        // Silent fail for non-text clipboard data.
                    }
                }
            } else {
                clipboard = Clipboard::new().ok();
            }
        }
    });
}

/// Bridge: Explicitly retrieves the current clipboard text if it contains a valid URL.
#[tauri::command]
pub fn get_clipboard() -> Result<Option<String>, String> {
    let mut cb = Clipboard::new().map_err(|e| e.to_string())?;
    match cb.get_text() {
        Ok(text) => {
            let text = text.trim().to_string();
            if is_valid_url(&text) {
                Ok(Some(text))
            } else {
                Ok(None)
            }
        },
        Err(_) => Ok(None)
    }
}

/// Heuristic: Determines if a string is a download-ready URL or Magnet link.
fn is_valid_url(url: &str) -> bool {
    let url_lower = url.to_lowercase();
    
    // Check for explicit protocols.
    if url_lower.starts_with("http://") || 
       url_lower.starts_with("https://") || 
       url_lower.starts_with("magnet:") {
        return true;
    }
    
    // Context-free check for strings like "mediafire.com/..." or "yts.mx/..."
    if url.contains('.') && !url.contains(' ') && url.len() > 3 {
        return true;
    }

    false
}
