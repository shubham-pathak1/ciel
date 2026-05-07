use std::time::Duration;

fn millis(value: Option<Duration>) -> u128 {
    value.map(|duration| duration.as_millis()).unwrap_or(0)
}

pub(super) fn startup_slow_detail(
    is_resume: bool,
    startup_elapsed: Duration,
    metadata_at: Option<Duration>,
    live_at: Option<Duration>,
    peers_at: Option<Duration>,
    first_network_at: Option<Duration>,
    initial_peers_count: usize,
) -> String {
    format!(
        "resume={}, reason=timeout_no_first_byte, elapsed_ms={}, metadata_ms={}, live_ms={}, peers_ms={}, first_network_ms={}, initial_peers={}",
        is_resume,
        startup_elapsed.as_millis(),
        millis(metadata_at),
        millis(live_at),
        millis(peers_at),
        millis(first_network_at),
        initial_peers_count,
    )
}

pub(super) fn startup_first_byte_detail(
    is_resume: bool,
    first_byte_at: Duration,
    first_network_at: Option<Duration>,
    metadata_at: Option<Duration>,
    live_at: Option<Duration>,
    peers_at: Option<Duration>,
    initial_peers_count: usize,
) -> String {
    let first_verified_lag_ms = first_network_at
        .map(|duration| first_byte_at.saturating_sub(duration).as_millis())
        .unwrap_or(0);

    format!(
        "resume={}, reason=first_byte, first_byte_ms={}, first_network_ms={}, first_verified_lag_ms={}, metadata_ms={}, live_ms={}, peers_ms={}, initial_peers={}",
        is_resume,
        first_byte_at.as_millis(),
        millis(first_network_at),
        first_verified_lag_ms,
        millis(metadata_at),
        millis(live_at),
        millis(peers_at),
        initial_peers_count,
    )
}
