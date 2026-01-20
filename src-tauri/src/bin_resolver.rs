//! Binary Resolver Module
//! 
//! This module manages external executable dependencies (yt-dlp, ffmpeg).
//! It implements a "Sidecar-First" strategy, preferring binaries bundled 
//! with the application package, but falling back to the system PATH if 
//! necessary.

use tauri_plugin_shell::process::Command;
use tauri_plugin_shell::ShellExt;
use tauri::AppHandle;
use std::process::Command as StdCommand;

/// Supported external dependencies.
pub enum Binary {
    /// Used for video platform extraction and downloading.
    YtDlp,
    /// Used for muxing video and audio streams (transcoding).
    Ffmpeg,
}

impl Binary {
    /// Returns the internal Tauri sidecar identifier.
    pub fn name(&self) -> &'static str {
        match self {
            Self::YtDlp => "yt-dlp",
            Self::Ffmpeg => "ffmpeg",
        }
    }

    /// Returns the expected executable name in the system's global PATH.
    pub fn system_name(&self) -> &'static str {
        match self {
            Self::YtDlp => "yt-dlp",
            Self::Ffmpeg => "ffmpeg",
        }
    }
}

/// Resolves the best `Command` to use for a given binary.
/// 
/// 1. Attempts to locate a bundled sidecar (per-platform binary).
/// 2. Falls back to a global system command if the sidecar is missing.
pub fn resolve_bin(app: &AppHandle, bin: Binary) -> Command {
    let sidecar_name = bin.name();
    
    // Check if sidecar exists and is runnable.
    // Note: Tauri's sidecar() API automatically appends the target triple (e.g. -x86_64-pc-windows-msvc).
    match app.shell().sidecar(sidecar_name) {
        Ok(cmd) => cmd,
        Err(_) => {
            // Fallback to searching the OS system PATH.
            app.shell().command(bin.system_name())
        }
    }
}

/// Readiness Check: Verifies if a binary is accessible on the host machine.
pub fn is_bin_available(app: &AppHandle, bin: Binary) -> bool {
    // Check if sidecar is bundled.
    if app.shell().sidecar(bin.name()).is_ok() {
        return true;
    }

    // Check system path by attempting to call '--version'.
    StdCommand::new(bin.system_name())
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
