use arboard::Clipboard;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use crate::db;

pub fn start_clipboard_monitor(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut clipboard = Clipboard::new().ok();
        let mut last_clipboard = String::new();

        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;

            // Check if autocatch is enabled
            let db_state = app.state::<db::DbState>();
            let enabled = db::get_setting(&db_state.path, "autocatch_enabled")
                .ok()
                .flatten()
                .map(|v| v == "true")
                .unwrap_or(true);

            if !enabled {
                continue;
            }

            if let Some(ref mut cb) = clipboard {
                if let Ok(text) = cb.get_text() {
                    let text = text.trim().to_string();
                    if !text.is_empty() && text != last_clipboard {
                        if is_valid_url(&text) {
                            let _ = app.emit("autocatch-url", &text);
                            last_clipboard = text;
                        }
                    }
                }
            } else {
                // Retry initializing clipboard if it failed
                clipboard = Clipboard::new().ok();
            }
        }
    });
}

fn is_valid_url(url: &str) -> bool {
    // Basic check for HTTP/HTTPS, Magnet, or common video site URLs
    let url_lower = url.to_lowercase();
    
    if url_lower.starts_with("http://") || url_lower.starts_with("https://") {
        // Exclude common non-downloadable sites if needed, but for now we'll be broad
        return true;
    }
    
    if url_lower.starts_with("magnet:") {
        return true;
    }

    false
}
