use crate::db::{self, DbState, Download, DownloadStatus, DownloadProtocol};
use crate::commands::{DownloadManager, resolve_download_path};
use serde::{Deserialize, Serialize};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tauri::{AppHandle, Emitter, State};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoMetadata {
    pub title: String,
    pub thumbnail: String,
    pub duration: Option<f64>,
    pub formats: Vec<VideoFormat>,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoFormat {
    pub format_id: String,
    pub extension: String,
    pub resolution: String,
    pub filesize: Option<u64>,
    pub protocol: String,
    pub note: Option<String>,
}

#[tauri::command]
pub async fn analyze_video_url(url: String) -> Result<VideoMetadata, String> {
    let output = tokio::process::Command::new("yt-dlp")
        .arg("--dump-json")
        .arg("--no-playlist")
        .arg("--flat-playlist")
        .arg("--no-warnings")
        .arg("--no-check-certificates")
        .arg("--quiet")
        .arg(&url)
        .output()
        .await
        .map_err(|e| format!("Failed to execute yt-dlp: {}. Is it installed?", e))?;

    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(format!("yt-dlp error: {}", err));
    }

    let json: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("Failed to parse yt-dlp output: {}", e))?;

    let title = json["title"].as_str().unwrap_or("Unknown Title").to_string();
    let thumbnail = json["thumbnail"].as_str().unwrap_or("").to_string();
    let duration = json["duration"].as_f64();

    let mut formats = Vec::new();
    if let Some(formats_array) = json["formats"].as_array() {
        for f in formats_array {
            let ext = f["ext"].as_str().unwrap_or("").to_string();
            let format_id = f["format_id"].as_str().unwrap_or("").to_string();
            
            // Filter out mhtml and unwanted formats
            if ext == "mhtml" || format_id.contains("mhtml") || ext == "webm" {
                continue;
            }

            let resolution = f["resolution"].as_str().unwrap_or("audio only").to_string();
            let filesize = f["filesize"].as_u64().or_else(|| f["filesize_approx"].as_u64());
            let protocol = f["protocol"].as_str().unwrap_or("").to_string();
            let note = f["format_note"].as_str().map(|s| s.to_string());

            formats.push(VideoFormat {
                format_id,
                extension: ext,
                resolution,
                filesize,
                protocol,
                note,
            });
        }
    }

    Ok(VideoMetadata {
        title,
        thumbnail,
        duration,
        formats,
        url,
    })
}

#[tauri::command]
pub async fn add_video_download(
    app: AppHandle,
    db_state: State<'_, DbState>,
    manager: State<'_, DownloadManager>,
    url: String,
    format_id: String,
    filepath: String,
) -> Result<(), String> {
    let id = uuid::Uuid::new_v4().to_string();
    
    // Ensure the filepath has the correct extension for muxed output (mp4)
    // unless it's an audio-only format.
    let mut adjusted_filepath = filepath.clone();
    let is_audio = filepath.ends_with(".m4a") || filepath.ends_with(".mp3") || filepath.ends_with(".aac") || filepath.ends_with(".opus");
    
    if !is_audio && !filepath.to_lowercase().ends_with(".mp4") {
        // Change or add .mp4 extension
        if let Some(pos) = filepath.rfind('.') {
            adjusted_filepath = format!("{}.mp4", &filepath[..pos]);
        } else {
            adjusted_filepath = format!("{}.mp4", filepath);
        }
    }

    let final_path = resolve_download_path(&app, &db_state.path, &adjusted_filepath, None);
    
    let download = Download {
        id: id.clone(),
        url: url.clone(),
        filename: adjusted_filepath.clone(),
        filepath: final_path.clone(),
        size: 0,
        downloaded: 0,
        status: DownloadStatus::Downloading,
        protocol: DownloadProtocol::Video,
        speed: 0,
        connections: 1,
        created_at: chrono::Utc::now().to_rfc3339(),
        completed_at: None,
        error_message: None,
        info_hash: None,
        metadata: Some(format_id.clone()),
    };

    db::insert_download(&db_state.path, &download).map_err(|e| e.to_string())?;

    start_video_download_task(app, db_state.path.clone(), manager.inner().clone(), download).await
}

pub async fn start_video_download_task(
    app: AppHandle,
    db_path: String,
    manager: DownloadManager,
    download: Download,
) -> Result<(), String> {
    let id = download.id.clone();
    let url = download.url.clone();
    let final_path = download.filepath.clone();
    let format_id = download.metadata.clone().unwrap_or_else(|| "best".to_string());
    let db_path_clone = db_path.clone();
    let app_clone = app.clone();
    let id_clone = id.clone();
    let manager_clone = manager.clone();

    // Create cancellation channel
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    manager.add_active(id.clone(), tx).await;

    let max_connections = db::get_setting(&db_path, "max_connections")
        .ok()
        .flatten()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(8);

    tokio::spawn(async move {
        let mut child = tokio::process::Command::new("yt-dlp")
            .arg("-f")
            .arg(&format!("{}+bestaudio/best", format_id))
            .arg("--merge-output-format")
            .arg("mp4")
            .arg("--embed-subs")
            .arg("--all-subs")
            .arg("--concurrent-fragments")
            .arg(max_connections.to_string())
            .arg("--no-mtime")
            .arg("--no-check-certificates")
            .arg("--no-warnings")
            .arg("--no-playlist")
            .arg("--newline")
            .arg("--progress")
            .arg("-o")
            .arg(&final_path)
            .arg(&url)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("Failed to start yt-dlp");

        let stdout = child.stdout.take().unwrap();
        let mut reader = BufReader::new(stdout).lines();

        let mut max_total_size = 0;
        let mut aborted = false;

        loop {
            tokio::select! {
                line_res = reader.next_line() => {
                    match line_res {
                        Ok(Some(line)) => {
                            if line.contains("[download]") && line.contains("%") {
                                let parts: Vec<&str> = line.split_whitespace().collect();
                                let mut progress_pct = 0.0;
                                let mut total_size_bytes = 0;
                                let mut speed_bytes = 0;
                                let mut eta_secs = 0;

                                for (i, part) in parts.iter().enumerate() {
                                    if part.contains('%') {
                                        progress_pct = part.replace("%", "").parse::<f64>().unwrap_or(0.0);
                                    }
                                    if *part == "of" && i + 1 < parts.len() {
                                        total_size_bytes = parse_size(parts[i+1]);
                                    }
                                    if *part == "at" && i + 1 < parts.len() {
                                        speed_bytes = parse_size(parts[i+1].replace("/s", "").as_str());
                                    }
                                    if *part == "ETA" && i + 1 < parts.len() {
                                        eta_secs = parse_eta(parts[i+1]);
                                    }
                                }

                                if total_size_bytes > max_total_size {
                                    max_total_size = total_size_bytes;
                                }

                                let downloaded = (progress_pct / 100.0 * max_total_size as f64) as i64;
                                
                                let _ = app_clone.emit("download-progress", serde_json::json!({
                                    "id": id_clone,
                                    "total": max_total_size,
                                    "downloaded": downloaded,
                                    "speed": speed_bytes,
                                    "eta": eta_secs,
                                    "connections": max_connections,
                                }));

                                let _ = db::update_download_progress(&db_path_clone, &id_clone, downloaded, speed_bytes as i64);
                                if max_total_size > 0 {
                                    let _ = db::update_download_size(&db_path_clone, &id_clone, max_total_size as i64);
                                }
                            }
                        }
                        Ok(None) => break,
                        Err(_) => break,
                    }
                }
                _ = rx.recv() => {
                    let _ = child.kill().await;
                    aborted = true;
                    let _ = db::update_download_status(&db_path_clone, &id_clone, DownloadStatus::Paused);
                    let _ = app_clone.emit("download-paused", id_clone.clone());
                    break;
                }
            }
        }

        if !aborted {
            let status = child.wait().await;
            if status.map_or(false, |s| s.success()) {
                // Update final size from disk
                if let Ok(meta) = std::fs::metadata(&final_path) {
                    let final_size = meta.len() as i64;
                    let _ = db::update_download_size(&db_path_clone, &id_clone, final_size);
                    let _ = db::update_download_progress(&db_path_clone, &id_clone, final_size, 0);
                }
                
                let _ = db::update_download_status(&db_path_clone, &id_clone, DownloadStatus::Completed);
                let _ = app_clone.emit("download-completed", id_clone.clone());
            } else {
                let _ = db::update_download_status(&db_path_clone, &id_clone, DownloadStatus::Error);
                let _ = app_clone.emit("download-error", (id_clone.clone(), "yt-dlp failed"));
            }
        }

        manager_clone.remove_active(&id_clone).await;
    });

    Ok(())
}

fn parse_size(s: &str) -> u64 {
    let s = s.to_lowercase();
    let factor = if s.contains("gib") { 1024 * 1024 * 1024 }
    else if s.contains("mib") { 1024 * 1024 }
    else if s.contains("kib") { 1024 }
    else { 1 };
    
    let num = s.chars().take_while(|c| c.is_digit(10) || *c == '.').collect::<String>();
    (num.parse::<f64>().unwrap_or(0.0) * factor as f64) as u64
}

fn parse_eta(s: &str) -> u64 {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() == 2 {
        let m = parts[0].parse::<u64>().unwrap_or(0);
        let s = parts[1].parse::<u64>().unwrap_or(0);
        m * 60 + s
    } else if parts.len() == 3 {
        let h = parts[0].parse::<u64>().unwrap_or(0);
        let m = parts[1].parse::<u64>().unwrap_or(0);
        let s = parts[2].parse::<u64>().unwrap_or(0);
        h * 3600 + m * 60 + s
    } else {
        0
    }
}
