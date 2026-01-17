use arboard::Clipboard;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use crate::db;

pub fn start_clipboard_monitor(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut clipboard = Clipboard::new().ok();
        let mut last_clipboard = String::new();

        let mut last_settings_check = std::time::Instant::now() - Duration::from_secs(10);
        let mut cached_enabled = true;

        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;

            // Check if autocatch is enabled (cached for 5s to reduce DB polling)
            if last_settings_check.elapsed() > Duration::from_secs(5) {
                let db_state = app.state::<db::DbState>();
                let settings = db::get_all_settings(&db_state.path).unwrap_or_default();
                cached_enabled = settings.get("autocatch_enabled")
                    .map(|v| v == "true")
                    .unwrap_or(true);
                last_settings_check = std::time::Instant::now();
            }

            if !cached_enabled {
                last_clipboard.clear(); // Clear so it catches fresh if re-enabled
                continue;
            }

            if let Some(ref mut cb) = clipboard {
                match cb.get_text() {
                    Ok(text) => {
                        let text = text.trim().to_string();
                        if !text.is_empty() && text != last_clipboard {
                            if is_valid_url(&text) {
                                let _ = app.emit("autocatch-url", &text);
                                last_clipboard = text;
                            }
                        }
                    },
                    Err(_) => {
                        // Don't spam error if clipboard is empty or non-text
                    }
                }
            } else {
                clipboard = Clipboard::new().ok();
            }
        }
    });
}

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

fn is_valid_url(url: &str) -> bool {
    let url_lower = url.to_lowercase();
    
    // Very broad check: starts with common protocols or contains a dot and no spaces
    if url_lower.starts_with("http://") || 
       url_lower.starts_with("https://") || 
       url_lower.starts_with("magnet:") {
        return true;
    }
    
    // Handle cases like "youtu.be/xxx" without protocol
    if url.contains('.') && !url.contains(' ') && url.len() > 3 {
        return true;
    }

    false
}
