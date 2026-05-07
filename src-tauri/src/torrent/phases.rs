pub(super) struct PhaseState {
    phase_key: String,
    phase_started_at: std::time::Instant,
    paused_counter: u8,
    was_live: bool,
    last_resume_time: std::time::Instant,
}

pub(super) struct PhaseInput {
    pub(super) now: std::time::Instant,
    pub(super) total_bytes: u64,
    pub(super) progress_bytes: u64,
    pub(super) has_live: bool,
    pub(super) connections: u64,
    pub(super) is_cached_paused: bool,
    pub(super) is_resume: bool,
    pub(super) speed_bps: u64,
    pub(super) startup_first_byte_seen: bool,
    pub(super) stalled_since: Option<std::time::Instant>,
    pub(super) live_stalled_since: Option<std::time::Instant>,
    pub(super) startup_baseline_bytes: u64,
}

pub(super) struct PhaseUpdate {
    pub(super) status_text: Option<String>,
    pub(super) phase_key: String,
    pub(super) phase_changed_from: Option<String>,
    pub(super) phase_elapsed_secs: u64,
    pub(super) reset_speed_baseline: bool,
}

impl PhaseState {
    pub(super) fn new(is_resume: bool) -> Self {
        Self {
            phase_key: if is_resume {
                "restoring_session".to_string()
            } else {
                "initializing".to_string()
            },
            phase_started_at: std::time::Instant::now(),
            paused_counter: 0,
            was_live: false,
            last_resume_time: std::time::Instant::now(),
        }
    }

    pub(super) fn current_phase(&self) -> String {
        self.phase_key.clone()
    }

    pub(super) fn last_resume_time(&self) -> std::time::Instant {
        self.last_resume_time
    }

    pub(super) fn evaluate(&mut self, input: PhaseInput) -> PhaseUpdate {
        let PhaseInput {
            now,
            total_bytes,
            progress_bytes,
            has_live,
            connections,
            is_cached_paused,
            is_resume,
            speed_bps,
            startup_first_byte_seen,
            stalled_since,
            live_stalled_since,
            startup_baseline_bytes,
        } = input;

        let mut reset_speed_baseline = false;
        let (status_text, phase_next): (Option<String>, &'static str) = if total_bytes == 0 {
            (
                Some(format!("Fetching Metadata... ({} peers)", connections)),
                "fetching_metadata",
            )
        } else if is_cached_paused {
            self.paused_counter = 50;
            self.was_live = false;
            reset_speed_baseline = true;
            (Some("Paused".to_string()), "paused")
        } else if !has_live {
            self.paused_counter = self.paused_counter.saturating_add(1);
            self.was_live = false;
            reset_speed_baseline = true;

            if total_bytes > 0
                && progress_bytes > startup_baseline_bytes
                && progress_bytes < total_bytes
            {
                let stalled_for = stalled_since
                    .map(|t| now.duration_since(t))
                    .unwrap_or_default();

                if stalled_for >= std::time::Duration::from_secs(20) {
                    (Some("Finding peers...".to_string()), "finding_peers")
                } else {
                    let pct = (progress_bytes as f64 / total_bytes as f64) * 100.0;
                    (
                        Some(format!("Verifying local data... {:.1}%", pct.min(100.0))),
                        "verifying_data",
                    )
                }
            } else if total_bytes > 0 && progress_bytes >= total_bytes {
                (Some("Finding peers...".to_string()), "finding_peers")
            } else if is_resume {
                (Some("Resuming...".to_string()), "restoring_session")
            } else {
                (Some("Initializing...".to_string()), "initializing")
            }
        } else {
            if self.paused_counter >= 1 || !self.was_live {
                self.last_resume_time = now;
            }
            self.paused_counter = 0;
            self.was_live = true;

            if speed_bps == 0 {
                if connections == 0 {
                    (Some("Connecting...".to_string()), "connecting")
                } else {
                    let negotiating_for = live_stalled_since
                        .map(|t| now.duration_since(t))
                        .unwrap_or_default();
                    if negotiating_for >= std::time::Duration::from_secs(20) {
                        (
                            Some(format!(
                                "Negotiating peers... ({} peers, slow swarm)",
                                connections
                            )),
                            "negotiating_peers",
                        )
                    } else {
                        (
                            Some(format!("Negotiating peers... ({} peers)", connections)),
                            "negotiating_peers",
                        )
                    }
                }
            } else if !startup_first_byte_seen {
                (
                    Some(format!(
                        "Receiving data (unverified, {} peers)",
                        connections
                    )),
                    "preparing_first_piece",
                )
            } else {
                (
                    Some(format!("Downloading ({} peers)", connections)),
                    "downloading",
                )
            }
        };

        let mut phase_changed_from = None;
        if self.phase_key != phase_next {
            phase_changed_from = Some(self.phase_key.clone());
            self.phase_key = phase_next.to_string();
            self.phase_started_at = now;
        }

        let phase_elapsed_secs = now.duration_since(self.phase_started_at).as_secs();

        PhaseUpdate {
            status_text,
            phase_key: self.phase_key.clone(),
            phase_changed_from,
            phase_elapsed_secs,
            reset_speed_baseline,
        }
    }
}
