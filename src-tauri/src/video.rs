use crate::db::{self, DbState, Download, DownloadStatus, DownloadProtocol};
use crate::commands::{DownloadManager, resolve_download_path, ensure_unique_path, execute_post_download_actions};
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
    pub acodec: Option<String>,
    pub vcodec: Option<String>,
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
            let acodec = f["acodec"].as_str().map(|s| s.to_string());
            let vcodec = f["vcodec"].as_str().map(|s| s.to_string());

            formats.push(VideoFormat {
                format_id,
                extension: ext,
                resolution,
                filesize,
                protocol,
                note,
                acodec,
                vcodec,
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
    audio_id: Option<String>,
    total_size: Option<u64>,
    filepath: String,
    output_folder: Option<String>,
    user_agent: Option<String>,
    cookies: Option<String>,
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

    let resolved_path = resolve_download_path(&app, &db_state.path, &adjusted_filepath, output_folder);
    let final_path = ensure_unique_path(resolved_path);

    // Extract the final unique filename from the path
    let final_filename = std::path::Path::new(&final_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| adjusted_filepath.clone());
    
    // Store specific audio choice in metadata
    let meta_json = serde_json::json!({
        "format_id": format_id,
        "audio_id": audio_id,
        "total_size": total_size
    });

    let download = Download {
        id: id.clone(),
        url: url.clone(),
        filename: final_filename,
        filepath: final_path.clone(),
        size: total_size.map(|s| s as i64).unwrap_or(0),
        downloaded: 0,
        status: DownloadStatus::Downloading,
        protocol: DownloadProtocol::Video,
        speed: 0,
        connections: 1,
        created_at: chrono::Utc::now().to_rfc3339(),
        completed_at: None,
        error_message: None,
        info_hash: None,
        metadata: Some(meta_json.to_string()),
        user_agent,
        cookies,
        category: "Video".to_string(),
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
    let db_path_clone = db_path.clone();
    let app_clone = app.clone();
    let id_clone = id.clone();
    let manager_clone = manager.clone();

    // Parse format selection from metadata
    let (format_id, audio_id) = if let Some(meta_str) = &download.metadata {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(meta_str) {
            let fid = json["format_id"].as_str().unwrap_or("best").to_string();
            let aid = json["audio_id"].as_str().map(|s| s.to_string());
            (fid, aid)
        } else {
            // Legacy/Fallback: metadata is just the format_id string
            (meta_str.clone(), None)
        }
    } else {
        ("best".to_string(), None)
    };

    // Create cancellation channel
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    manager.add_active(id.clone(), tx).await;

    let max_connections = db::get_setting(&db_path, "max_connections")
        .ok()
        .flatten()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(8);

    // Construct format selector
    // If specific audio ID is provided, merge it.
    // If None, assume video file already has audio (progressive) OR user selected audio-only.
    // We NO LONGER blindly append +bestaudio. Smart frontend does the picking.
    let format_selector = if let Some(aid) = audio_id {
        format!("{}+{}", format_id, aid)
    } else {
        format_id
    };

    let speed_limit = db::get_setting(&db_path, "speed_limit")
        .ok()
        .flatten()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);

    tokio::spawn(async move {
        let mut cmd = tokio::process::Command::new("yt-dlp");
        cmd.arg("-f")
            .arg(&format_selector)
            .arg("--merge-output-format")
            .arg("mp4");
        
        if speed_limit > 0 {
            cmd.arg("--ratelimit").arg(format!("{}", speed_limit));
        }

        let mut child = cmd.arg("--concurrent-fragments")
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

        // Initial total from metadata if available
        let expected_total_size = if let Some(ref meta_str) = download.metadata {
             if let Ok(json) = serde_json::from_str::<serde_json::Value>(&meta_str) {
                 json["total_size"].as_u64().unwrap_or(0)
             } else { 0 }
        } else { 0 };

        let mut accumulated_completed_bytes: u64 = 0;
        let mut current_file_max_size: u64 = 0;
        let mut aborted = false;

        // Track the filename we are currently processing to detect switches
        // let mut current_destination = String::new();
        let mut status_text: Option<String> = Some("Starting...".to_string());

        loop {
            tokio::select! {
                line_res = reader.next_line() => {
                    match line_res {
                        Ok(Some(line)) => {
                            // Detect status/phase changes
                            if line.starts_with("[youtube]") {
                                status_text = Some("Extracting info...".to_string());
                            } else if line.starts_with("[Merger]") {
                                status_text = Some("Assembling...".to_string());
                            } else if line.starts_with("[ExtractAudio]") {
                                status_text = Some("Extracting Audio...".to_string());
                            } else if line.starts_with("[ffmpeg]") {
                                status_text = Some("Processing...".to_string());
                            }

                            // Detect new file start
                            if line.contains("[download] Destination:") {
                                // If we were tracking a previous file, assume it finished successfully
                                // and add its max size to accumulated. 
                                // (Unless it's the very first line).
                                if current_file_max_size > 0 {
                                    accumulated_completed_bytes += current_file_max_size;
                                    current_file_max_size = 0;
                                }
                                status_text = None; // Clear status text when downloading starts
                                
                                // Reset for new file
                                // current_destination = line.clone();
                            }
                            // Detect "already downloaded"
                            else if line.contains("has already been downloaded") {
                                // Try to parse size? Usually it says "100% of X MiB"
                            }

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

                                if total_size_bytes > current_file_max_size {
                                    current_file_max_size = total_size_bytes;
                                }

                                // Calculate reliable totals
                                let current_part_downloaded = (progress_pct / 100.0 * current_file_max_size as f64) as u64;
                                let total_downloaded_so_far = accumulated_completed_bytes + current_part_downloaded;
                                
                                // Dynamic Total: Whichever is larger (Expected vs Actual Running Sum)
                                let running_total = accumulated_completed_bytes + current_file_max_size;
                                let display_total = if expected_total_size > running_total { expected_total_size } else { running_total };

                                // Create "unified" view for the UI.
                                // If expected_total is accurate, this will smoothly go from 0 to 100%.
                                
                                let _ = app_clone.emit("download-progress", serde_json::json!({
                                    "id": id_clone,
                                    "total": display_total,
                                    "downloaded": total_downloaded_so_far,
                                    "speed": speed_bytes,
                                    "eta": eta_secs,
                                    "connections": max_connections,
                                    "status_text": status_text,
                                }));

                                let _ = db::update_download_progress(&db_path_clone, &id_clone, total_downloaded_so_far as i64, speed_bytes as i64);
                                if display_total > 0 {
                                    let _ = db::update_download_size(&db_path_clone, &id_clone, display_total as i64);
                                }
                            }
                            // If it's a status line but not download progress, we should emit an update too
                            else if status_text.is_some() && !line.contains("[download]") {
                                 let _ = app_clone.emit("download-progress", serde_json::json!({
                                    "id": id_clone,
                                    "total": if expected_total_size > 0 { expected_total_size } else { 0 },
                                    "downloaded": accumulated_completed_bytes, // Show what's done so far
                                    "speed": 0,
                                    "eta": 0,
                                    "connections": 0,
                                    "status_text": status_text,
                                }));
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
                // Wait briefly for OS to finalize file handle
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;

                // Update final size from disk
                if let Ok(meta) = std::fs::metadata(&final_path) {
                    let final_size = meta.len() as i64;
                    let _ = db::update_download_size(&db_path_clone, &id_clone, final_size);
                    let _ = db::update_download_progress(&db_path_clone, &id_clone, final_size, 0);
                }
                
                let _ = db::update_download_status(&db_path_clone, &id_clone, DownloadStatus::Completed);
                let _ = app_clone.emit("download-completed", id_clone.clone());

                // Post-Download Actions
                let download_clone = download.clone();
                execute_post_download_actions(app_clone.clone(), db_path_clone.clone(), download_clone).await;
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
    let factor = if s.contains("gb") || s.contains("gib") { 1024 * 1024 * 1024 }
    else if s.contains("mb") || s.contains("mib") { 1024 * 1024 }
    else if s.contains("kb") || s.contains("kib") { 1024 }
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
