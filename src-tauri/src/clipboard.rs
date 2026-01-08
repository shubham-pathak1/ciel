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
