use std::time::Duration;
use tauri::{AppHandle, Manager};
use crate::db;
use crate::commands::{self, DownloadManager};
use crate::torrent::TorrentManager;
use chrono::{Local, Timelike};

pub fn start_scheduler(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            // Check every 30 seconds
            tokio::time::sleep(Duration::from_secs(30)).await;

            let db_state = app.state::<db::DbState>();
            let settings = db::get_all_settings(&db_state.path).unwrap_or_default();

            let enabled = settings.get("scheduler_enabled")
                .map(|v| v == "true")
                .unwrap_or(false);

            if !enabled {
                continue;
            }

            let start_time_str = settings.get("scheduler_start_time")
                .cloned()
                .unwrap_or_else(|| "02:00".to_string());
            let pause_time_str = settings.get("scheduler_pause_time")
                .cloned()
                .unwrap_or_else(|| "08:00".to_string());

            let now = Local::now();
            let current_time_str = format!("{:02}:{:02}", now.hour(), now.minute());

            if current_time_str == start_time_str {
                resume_all_downloads(&app).await;
                // Sleep extra to avoid re-triggering in the same minute
                tokio::time::sleep(Duration::from_secs(61)).await;
            } else if current_time_str == pause_time_str {
                pause_all_downloads(&app).await;
                 // Sleep extra to avoid re-triggering in the same minute
                tokio::time::sleep(Duration::from_secs(61)).await;
            }
        }
    });
}

async fn resume_all_downloads(app: &AppHandle) {
    let db_state = app.state::<db::DbState>();
    let manager = app.state::<DownloadManager>();
    let torrent_manager = app.state::<TorrentManager>();

    if let Ok(downloads) = db::get_all_downloads(&db_state.path) {
        for download in downloads {
            if download.status == db::DownloadStatus::Paused || download.status == db::DownloadStatus::Queued {
                let _ = commands::resume_download(
                    app.clone(),
                    db_state.clone(),
                    manager.clone(),
                    torrent_manager.clone(),
                    download.id
                ).await;
            }
        }
    }
}

async fn pause_all_downloads(app: &AppHandle) {
    let db_state = app.state::<db::DbState>();
    let manager = app.state::<DownloadManager>();
    let torrent_manager = app.state::<TorrentManager>();

    if let Ok(downloads) = db::get_all_downloads(&db_state.path) {
        for download in downloads {
            if download.status == db::DownloadStatus::Downloading {
                let _ = commands::pause_download(
                    db_state.clone(),
                    manager.clone(),
                    torrent_manager.clone(),
                    download.id
                ).await;
            }
        }
    }
}
