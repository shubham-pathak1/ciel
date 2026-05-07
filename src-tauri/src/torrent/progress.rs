use super::files;
use super::phases::{PhaseInput, PhaseState};
use super::telemetry;
use super::TorrentManager;
use std::collections::HashSet;
use std::path::Path;
use tauri::{AppHandle, Emitter, Runtime};

impl TorrentManager {
    /// Adds a new magnet link or torrent file to the active session.
    pub async fn add_magnet<R: Runtime>(
        &self,
        app: AppHandle<R>,
        id: String,
        magnet: String,
        output_folder: String,
        db_path: String,
        indices: Option<Vec<usize>>,
        total_size: u64,
        known_downloaded_baseline: u64,
        is_resume: bool,
        start_paused: bool,
        source_torrent_bytes: Option<Vec<u8>>,
    ) -> Result<(), String> {
        let session_guard = self.session.lock().await;
        let session = session_guard
            .as_ref()
            .ok_or("Torrent session is not yet initialized. Please wait a moment.")?
            .clone();
        drop(session_guard);

        let initial_peers = Self::extract_initial_peers_from_magnet(&magnet);
        let initial_peers_opt = if initial_peers.is_empty() {
            None
        } else {
            Some(initial_peers.clone())
        };
        if let Some(peers) = initial_peers_opt.as_ref() {
            tracing::info!(
                "[Torrent] {}: seeding {} initial peer(s) from magnet x.pe",
                id,
                peers.len()
            );
        }

        if start_paused {
            let mut paused = self.paused_downloads.lock().await;
            paused.insert(id.clone());
        }

        let local_torrent_bytes = if source_torrent_bytes.is_none() {
            Self::read_local_torrent_bytes(&magnet)?
        } else {
            None
        };

        let response = match source_torrent_bytes.or(local_torrent_bytes) {
            Some(torrent_bytes) => {
                let options = librqbit::AddTorrentOptions {
                    only_files: indices.clone(),
                    output_folder: Some(output_folder.clone()),
                    overwrite: is_resume,
                    initial_peers: initial_peers_opt.clone(),
                    ..Default::default()
                };
                session
                    .add_torrent(
                        librqbit::AddTorrent::from_bytes(torrent_bytes),
                        Some(options),
                    )
                    .await
            }
            None => {
                let options = librqbit::AddTorrentOptions {
                    only_files: indices.clone(),
                    output_folder: Some(output_folder.clone()),
                    overwrite: is_resume,
                    initial_peers: initial_peers_opt.clone(),
                    ..Default::default()
                };
                session
                    .add_torrent(librqbit::AddTorrent::from_url(&magnet), Some(options))
                    .await
            }
        }
        .map_err(|e| e.to_string())?;

        let handle = response
            .into_handle()
            .ok_or("Failed to get torrent handle")?;

        if start_paused {
            let _ = session.pause(&handle).await;
        } else if is_resume {
            // Restart resume path: ensure handle is actively unpaused.
            // AlreadyManaged handles can be left paused depending on recovered state.
            if let Err(e) = session.unpause(&handle).await {
                let msg = e.to_string();
                if !msg.contains("not paused")
                    && !msg.contains("already running")
                    && !msg.contains("already live")
                {
                    tracing::error!("[Torrent] Resume unpause failed for {}: {}", id, msg);
                }
            }
        }

        {
            let mut active = self.active_torrents.lock().await;
            active.insert(id.clone(), handle.clone());
        }

        // Store indices in metadata for resumption support
        if let Some(idx) = &indices {
            let db_p = db_path.clone();
            let id_p = id.clone();
            let meta_json = serde_json::json!({ "indices": idx }).to_string();
            tokio::task::spawn_blocking(move || {
                if let Ok(conn) = crate::db::open_db(db_p) {
                    let _ = conn.execute(
                        "UPDATE downloads SET metadata = ?1 WHERE id = ?2",
                        (meta_json, id_p),
                    );
                }
            });
        }

        let id_clone = id.clone();
        let session_for_monitor = session.clone();

        let db_path_clone = db_path.clone();
        let output_folder_clone = output_folder;
        let selected_indices_for_cleanup = indices;
        let active_torrents = self.active_torrents.clone();
        let paused_downloads = self.paused_downloads.clone();
        let initial_peers_count = initial_peers.len();
        tokio::spawn(async move {
            let mut name_updated = false;
            let mut last_downloaded = handle.stats().progress_bytes;
            let mut last_time = std::time::Instant::now();
            let mut speed_u64 = 0u64;
            let mut verified_speed_u64 = 0u64;
            let mut smoothed_speed = 0.0f64;
            let mut phase_state = PhaseState::new(is_resume);
            let completion_handled = false;
            let mut stalled_since: Option<std::time::Instant> = None;
            let mut live_stalled_since: Option<std::time::Instant> = None;
            let mut last_recovery_poke: Option<std::time::Instant> = None;
            let mut last_progress_seen = handle.stats().progress_bytes;
            let mut last_db_flush = std::time::Instant::now();
            let mut last_db_bytes = handle.stats().progress_bytes;
            let startup_started_at = std::time::Instant::now();
            let startup_baseline_bytes = if is_resume {
                known_downloaded_baseline
            } else {
                handle.stats().progress_bytes
            };
            let startup_baseline_fetched = handle
                .stats()
                .live
                .as_ref()
                .map(|l| l.snapshot.fetched_bytes)
                .unwrap_or(startup_baseline_bytes);
            let mut startup_metadata_at: Option<std::time::Duration> = None;
            let mut startup_live_at: Option<std::time::Duration> = None;
            let mut startup_peers_at: Option<std::time::Duration> = None;
            let mut startup_first_network_at: Option<std::time::Duration> = None;
            let mut startup_first_byte_at: Option<std::time::Duration> = None;
            let mut startup_timeout_logged = false;
            let mut startup_first_byte_logged = false;

            // First immediate emission to clear UI "Paused" state
            let stats = handle.stats();
            let connections = stats
                .live
                .as_ref()
                .map(|l| l.snapshot.peer_stats.live)
                .unwrap_or(0) as u64;
            let initial_network_received = stats
                .live
                .as_ref()
                .map(|l| l.snapshot.fetched_bytes)
                .unwrap_or(startup_baseline_bytes);
            let initial_downloaded = if is_resume {
                startup_baseline_bytes
            } else {
                stats.progress_bytes
            };
            let _ = app.emit(
                "download-progress",
                serde_json::json!({
                    "id": id_clone,
                    "total": if stats.total_bytes > 0 { stats.total_bytes } else { total_size },
                    "downloaded": initial_downloaded,
                    "network_received": initial_network_received.max(initial_downloaded),
                    "verified_speed": 0u64,
                    "speed": 0,
                    "eta": 0,
                    "connections": connections,
                    "status_text": Some(if is_resume { "Resuming..." } else { "Initializing..." }),
                    "status_phase": phase_state.current_phase(),
                    "phase_elapsed_secs": 0u64,
                }),
            );

            loop {
                // CANCELLATION CHECK: If not in active_torrents anymore, exit loop
                {
                    let active = active_torrents.lock().await;
                    if !active.contains_key(&id_clone) {
                        break;
                    }
                }
                let stats = handle.stats();
                let connections = stats
                    .live
                    .as_ref()
                    .map(|l| l.snapshot.peer_stats.live)
                    .unwrap_or(0) as u64;

                // Calculate speed manually
                let now = std::time::Instant::now();
                let elapsed = now.duration_since(last_time).as_secs_f64();
                let downloaded_now = stats.progress_bytes;
                let startup_elapsed = now.duration_since(startup_started_at);
                let fetched_now = stats
                    .live
                    .as_ref()
                    .map(|l| l.snapshot.fetched_bytes)
                    .unwrap_or(startup_baseline_fetched);

                if startup_metadata_at.is_none() && stats.total_bytes > 0 {
                    startup_metadata_at = Some(startup_elapsed);
                }
                if startup_live_at.is_none() && stats.live.is_some() {
                    startup_live_at = Some(startup_elapsed);
                }
                if startup_peers_at.is_none() && connections > 0 {
                    startup_peers_at = Some(startup_elapsed);
                }
                if startup_first_network_at.is_none() && fetched_now > startup_baseline_fetched {
                    startup_first_network_at = Some(startup_elapsed);
                }
                if startup_first_byte_at.is_none()
                    && stats.live.is_some()
                    && downloaded_now > startup_baseline_bytes
                {
                    startup_first_byte_at = Some(startup_elapsed);
                }
                let is_restore_verifying =
                    is_resume && stats.live.is_none() && downloaded_now > startup_baseline_bytes;
                let display_downloaded = if is_restore_verifying {
                    startup_baseline_bytes
                } else {
                    downloaded_now.max(startup_baseline_bytes)
                };
                let network_received = if is_restore_verifying {
                    startup_baseline_bytes
                } else {
                    fetched_now.max(display_downloaded)
                };

                if elapsed >= 0.5 {
                    let diff = downloaded_now.saturating_sub(last_downloaded);
                    let mut verified_speed = diff as f64 / elapsed;
                    if stats.live.is_none() || connections == 0 {
                        verified_speed = 0.0;
                    }
                    verified_speed_u64 = verified_speed as u64;

                    let mut current_speed = verified_speed;
                    let live_speed_bps = stats
                        .live
                        .as_ref()
                        .map(|l| (l.download_speed.mbps.max(0.0) * 1024.0 * 1024.0) as u64)
                        .unwrap_or(0);

                    // Verification/initialization can advance progress counters without network transfer.
                    // Clamp speed while there is no live peer activity.
                    if stats.live.is_none() || connections == 0 {
                        current_speed = 0.0;
                    } else if live_speed_bps > 0 {
                        // Prefer engine-estimated fetch speed to avoid "0 speed until first verified piece" UX.
                        current_speed = live_speed_bps as f64;
                    }

                    // Keep an additional startup spike guard around quick resume transitions.
                    if connections == 0
                        && phase_state.last_resume_time().elapsed().as_secs() < 10
                        && current_speed > 5_000_000.0
                    {
                        current_speed = 0.0;
                    }

                    // Faster alpha (0.7) for first 5 seconds after resume to ramp up, then 0.3 for stability
                    let alpha = if phase_state.last_resume_time().elapsed().as_secs() < 5 {
                        0.7
                    } else {
                        0.3
                    };

                    if smoothed_speed == 0.0 && current_speed > 0.0 {
                        smoothed_speed = current_speed;
                    } else {
                        smoothed_speed = smoothed_speed * (1.0 - alpha) + current_speed * alpha;
                    }

                    speed_u64 = smoothed_speed as u64;
                    last_downloaded = downloaded_now;
                    last_time = now;
                }

                if stats.live.is_none() || connections == 0 {
                    speed_u64 = 0;
                    verified_speed_u64 = 0;
                }

                // Calculate ETA
                let eta = if speed_u64 > 0 {
                    stats.total_bytes.saturating_sub(stats.progress_bytes) / speed_u64
                } else {
                    0
                };

                // 1. Update Size & Info Hash on Metadata discovery
                // NOTE: We do NOT update filename/filepath here - they are already set correctly
                // by commands.rs with unique paths like "Movie (1).mkv"
                if !name_updated && stats.total_bytes > 0 {
                    let meta_result =
                        handle.with_metadata(|m| (m.name.clone(), m.file_infos.len()));
                    if let Ok((_real_name, file_count)) = meta_result {
                        let total_size = stats.total_bytes;

                        // Update DB size
                        let _ = crate::db::update_download_size(
                            &db_path_clone,
                            &id_clone,
                            total_size as i64,
                        );

                        // Update info_hash in DB
                        let db_p = db_path_clone.clone();
                        let id_p = id_clone.clone();
                        let info_hash_hex = hex::encode(handle.info_hash().0);

                        tracing::info!(
                            "[Torrent][Meta][{}] total_bytes={} files={} selected_files={} info_hash={}",
                            id_clone,
                            total_size,
                            file_count,
                            selected_indices_for_cleanup.as_ref().map(|v| v.len()).unwrap_or(0),
                            info_hash_hex
                        );

                        tokio::task::spawn_blocking(move || {
                            if let Ok(conn) = crate::db::open_db(db_p) {
                                let _ = conn.execute(
                                    "UPDATE downloads SET info_hash = ?1 WHERE id = ?2",
                                    (info_hash_hex, id_p),
                                );
                            }
                        });

                        name_updated = true;
                    }
                }

                if !stats.finished {
                    let is_cached_paused = {
                        let paused = paused_downloads.lock().await;
                        paused.contains(&id_clone)
                    };
                    let is_verifying = stats.live.is_none()
                        && startup_first_byte_at.is_none()
                        && stats.progress_bytes > startup_baseline_bytes;
                    let bytes_delta = stats.progress_bytes.saturating_sub(last_db_bytes);
                    let should_flush_db = !is_verifying
                        && (stats.total_bytes > 0 && stats.progress_bytes >= stats.total_bytes
                            || bytes_delta >= 1_048_576
                            || last_db_flush.elapsed() >= std::time::Duration::from_secs(1)
                            || is_cached_paused);

                    if should_flush_db {
                        let _ = crate::db::update_download_progress(
                            &db_path_clone,
                            &id_clone,
                            stats.progress_bytes as i64,
                            speed_u64 as i64,
                        );
                        last_db_flush = now;
                        last_db_bytes = stats.progress_bytes;
                    }

                    if !is_cached_paused
                        && !startup_timeout_logged
                        && startup_first_byte_at.is_none()
                        && startup_first_network_at.is_none()
                        && startup_elapsed >= std::time::Duration::from_secs(45)
                    {
                        let detail = telemetry::startup_slow_detail(
                            is_resume,
                            startup_elapsed,
                            startup_metadata_at,
                            startup_live_at,
                            startup_peers_at,
                            startup_first_network_at,
                            initial_peers_count,
                        );
                        tracing::info!("[Torrent][Startup][{}] {}", id_clone, detail);
                        let db_p = db_path_clone.clone();
                        let id_p = id_clone.clone();
                        tokio::task::spawn_blocking(move || {
                            let _ = crate::db::log_event(
                                &db_p,
                                &id_p,
                                "startup_slow",
                                Some(detail.as_str()),
                            );
                        });
                        startup_timeout_logged = true;
                    }

                    if !startup_first_byte_logged {
                        if let Some(first_byte_at) = startup_first_byte_at {
                            let detail = telemetry::startup_first_byte_detail(
                                is_resume,
                                first_byte_at,
                                startup_first_network_at,
                                startup_metadata_at,
                                startup_live_at,
                                startup_peers_at,
                                initial_peers_count,
                            );
                            tracing::info!("[Torrent][Startup][{}] {}", id_clone, detail);
                            let db_p = db_path_clone.clone();
                            let id_p = id_clone.clone();
                            tokio::task::spawn_blocking(move || {
                                let _ = crate::db::log_event(
                                    &db_p,
                                    &id_p,
                                    "startup_profile",
                                    Some(detail.as_str()),
                                );
                            });
                            startup_first_byte_logged = true;
                        }
                    }

                    if stats.progress_bytes > last_progress_seen {
                        last_progress_seen = stats.progress_bytes;
                        stalled_since = None;
                        live_stalled_since = None;
                    } else if stats.live.is_none() && !is_cached_paused {
                        stalled_since.get_or_insert(now);
                        live_stalled_since = None;
                    } else if stats.live.is_some() && connections > 0 && !is_cached_paused {
                        stalled_since = None;
                        live_stalled_since.get_or_insert(now);
                    } else {
                        stalled_since = None;
                        live_stalled_since = None;
                    }

                    if stats.live.is_none() && !is_cached_paused {
                        if let Some(stalled_at) = stalled_since {
                            let stalled_for = now.duration_since(stalled_at);
                            let can_poke = last_recovery_poke
                                .map(|t| {
                                    now.duration_since(t) >= std::time::Duration::from_secs(12)
                                })
                                .unwrap_or(true);

                            if stalled_for >= std::time::Duration::from_secs(20) && can_poke {
                                if let Err(e) = session_for_monitor.unpause(&handle).await {
                                    let msg = e.to_string();
                                    if !msg.contains("not paused")
                                        && !msg.contains("already running")
                                        && !msg.contains("already live")
                                    {
                                        tracing::error!(
                                            "[Torrent] Recovery unpause failed for {}: {}",
                                            id_clone,
                                            msg
                                        );
                                    }
                                }
                                last_recovery_poke = Some(now);
                            }
                        }
                    } else {
                        last_recovery_poke = None;
                    }

                    let phase_update = phase_state.evaluate(PhaseInput {
                        now,
                        total_bytes: stats.total_bytes,
                        progress_bytes: stats.progress_bytes,
                        has_live: stats.live.is_some(),
                        connections,
                        is_cached_paused,
                        is_resume,
                        speed_bps: speed_u64,
                        startup_first_byte_seen: startup_first_byte_at.is_some(),
                        stalled_since,
                        live_stalled_since,
                        startup_baseline_bytes,
                    });
                    if phase_update.reset_speed_baseline {
                        last_downloaded = stats.progress_bytes;
                        last_time = now;
                    }

                    if let Some(previous_phase) = phase_update.phase_changed_from.as_ref() {
                        tracing::info!(
                            "[Torrent][Phase][{}] {} -> {} at {}ms peers={} speed={}Bps rx={} verified={}",
                            id_clone,
                            previous_phase,
                            phase_update.phase_key,
                            startup_elapsed.as_millis(),
                            connections,
                            speed_u64,
                            network_received,
                            display_downloaded
                        );
                    }

                    let _ = app.emit(
                        "download-progress",
                        serde_json::json!({
                            "id": id_clone,
                            "total": stats.total_bytes,
                            "downloaded": display_downloaded,
                            "network_received": network_received,
                            "verified_speed": verified_speed_u64,
                            "speed": speed_u64,
                            "eta": eta,
                            "connections": connections,
                            "status_text": phase_update.status_text,
                            "status_phase": phase_update.phase_key,
                            "phase_elapsed_secs": phase_update.phase_elapsed_secs,
                        }),
                    );
                }

                let complete_by_stats = stats.finished;
                let complete_by_bytes = stats.live.is_some()
                    && stats.total_bytes > 0
                    && stats.progress_bytes >= stats.total_bytes;

                if (complete_by_stats || complete_by_bytes) && !completion_handled {
                    let file_entries_for_cleanup = if selected_indices_for_cleanup.is_some() {
                        handle
                            .with_metadata(|m| {
                                m.file_infos
                                    .iter()
                                    .enumerate()
                                    .map(|(idx, f)| {
                                        (idx, f.relative_filename.to_string_lossy().to_string())
                                    })
                                    .collect::<Vec<(usize, String)>>()
                            })
                            .ok()
                    } else {
                        None
                    };

                    // 1. Update status to Completed in DB (Block until done to prevent race with frontend)
                    let db_p = db_path_clone.clone();
                    let id_p = id_clone.clone();
                    let total_bytes_final = stats.total_bytes; // Capture explicit current size
                    let _ = tokio::task::spawn_blocking(move || {
                        if let Err(e) = crate::db::mark_download_completed(&db_p, &id_p) {
                            tracing::error!(
                                "CRITICAL DB ERROR: Failed to mark as completed: {}",
                                e
                            );
                        }

                        // Also ensure progress is capped at 100%
                        let _ = crate::db::update_download_progress(
                            &db_p,
                            &id_p,
                            total_bytes_final as i64,
                            0,
                        );
                    })
                    .await;

                    // 2. Emit completion event only AFTER DB is updated
                    let _ = app.emit("download-completed", id_clone.clone());

                    // 3. Remove the torrent from in-memory/session state to release file handles.
                    {
                        let mut active = active_torrents.lock().await;
                        active.remove(&id_clone);
                    }
                    {
                        let mut paused = paused_downloads.lock().await;
                        paused.remove(&id_clone);
                    }
                    let info_hash = handle.info_hash();
                    if let Err(e) = session_for_monitor
                        .delete(librqbit::api::TorrentIdOrHash::Hash(info_hash), false)
                        .await
                    {
                        tracing::error!(
                            "[Torrent] Failed to remove completed torrent {} from session: {}",
                            id_clone,
                            e
                        );
                    }

                    // 4. Remove unselected placeholders after handle release.
                    if let (Some(selected_indices), Some(file_entries)) = (
                        selected_indices_for_cleanup.as_ref(),
                        file_entries_for_cleanup.as_ref(),
                    ) {
                        for attempt in 0..8 {
                            files::cleanup_unselected_placeholder_files(
                                &output_folder_clone,
                                selected_indices,
                                file_entries,
                            );
                            let selected: HashSet<usize> =
                                selected_indices.iter().copied().collect();
                            let has_remaining_unselected =
                                file_entries.iter().any(|(idx, relative_path)| {
                                    if selected.contains(idx) {
                                        return false;
                                    }
                                    Path::new(&output_folder_clone).join(relative_path).exists()
                                });
                            if !has_remaining_unselected {
                                break;
                            }
                            if attempt < 7 {
                                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                            }
                        }
                    }

                    // completion_handled = true; // Unused as we break immediately

                    // 5. Post-Download Actions
                    // We need the full Download record to know the filepath
                    if let Ok(downloads) = crate::db::get_all_downloads(&db_path_clone) {
                        if let Some(download) = downloads.into_iter().find(|d| d.id == id_clone) {
                            crate::commands::execute_post_download_actions(
                                app.clone(),
                                db_path_clone.clone(),
                                download,
                            )
                            .await;
                        }
                    }
                    break;
                }

                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            }
        });

        Ok(())
    }
}
