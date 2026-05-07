use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use thiserror::Error;

/// A shared token-bucket rate limiter to coordinate multiple download workers.
pub struct SharedRateLimiter {
    limit: u64,
    tokens: AtomicU64,
    last_update: std::sync::Mutex<std::time::Instant>,
}

impl SharedRateLimiter {
    pub fn new(limit: u64) -> Self {
        Self {
            limit,
            tokens: AtomicU64::new(limit),
            last_update: std::sync::Mutex::new(std::time::Instant::now()),
        }
    }

    pub async fn acquire(&self, amount: u64, cancel_signal: &Option<Arc<AtomicBool>>) {
        if self.limit == 0 {
            return;
        }

        let mut remaining = amount;
        while remaining > 0 {
            if let Some(sig) = cancel_signal {
                if sig.load(Ordering::Relaxed) {
                    return;
                }
            }

            {
                let mut last_update = self.last_update.lock().unwrap();
                let now = std::time::Instant::now();
                let elapsed = now.duration_since(*last_update).as_secs_f64();

                if elapsed >= 0.01 {
                    let refill = (self.limit as f64 * elapsed) as u64;
                    if refill > 0 {
                        let current = self.tokens.load(Ordering::Relaxed);
                        let new_tokens = (current + refill).min(self.limit);
                        self.tokens.store(new_tokens, Ordering::Relaxed);
                        *last_update = now;
                    }
                }
            }

            let current = self.tokens.load(Ordering::Relaxed);
            if current > 0 {
                let take = remaining.min(current);
                if self
                    .tokens
                    .compare_exchange(current, current - take, Ordering::SeqCst, Ordering::Relaxed)
                    .is_ok()
                {
                    remaining -= take;
                    if remaining == 0 {
                        break;
                    }
                }
            }

            if remaining > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }
    }
}

#[derive(Error, Debug, Clone, Serialize)]
pub enum DownloadError {
    #[error("Network error: {0}")]
    Network(String),
    #[error("IO error: {0}")]
    Io(String),
    #[error("Server does not support range requests")]
    NoRangeSupport,
    #[error("Download cancelled")]
    Cancelled,
    #[error("Invalid URL: {0}")]
    InvalidUrl(String),
}

impl From<reqwest::Error> for DownloadError {
    fn from(err: reqwest::Error) -> Self {
        DownloadError::Network(err.to_string())
    }
}

impl From<std::io::Error> for DownloadError {
    fn from(err: std::io::Error) -> Self {
        DownloadError::Io(err.to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadConfig {
    pub id: String,
    pub url: String,
    pub filepath: PathBuf,
    pub connections: u8,
    pub chunk_size: u64,
    pub speed_limit: u64,
    pub user_agent: Option<String>,
    pub cookies: Option<String>,
    pub force_multi: bool,
    pub size_hint: Option<u64>,
}

impl Default for DownloadConfig {
    fn default() -> Self {
        Self {
            id: "default".to_string(),
            url: String::new(),
            filepath: PathBuf::new(),
            connections: 8,
            chunk_size: 5 * 1024 * 1024,
            speed_limit: 0,
            user_agent: None,
            cookies: None,
            force_multi: false,
            size_hint: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadProgress {
    pub id: String,
    pub total: u64,
    pub downloaded: u64,
    pub speed: u64,
    pub eta: u64,
    pub connections: u8,
    pub speed_limit: u64,
    pub status_text: Option<String>,
    pub status_phase: Option<String>,
    pub phase_elapsed_secs: Option<u64>,
    pub filename: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkRecord {
    pub download_id: String,
    pub start: i64,
    pub end: i64,
    pub downloaded: i64,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct WorkChunk {
    pub(super) start: u64,
    pub(super) end: u64,
    pub(super) downloaded: u64,
    pub(super) _index: usize,
}
