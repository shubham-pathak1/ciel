use tauri_plugin_shell::process::Command;
use tauri_plugin_shell::ShellExt;
use tauri::AppHandle;
use std::process::Command as StdCommand;

pub enum Binary {
    YtDlp,
    Ffmpeg,
}

impl Binary {
    pub fn name(&self) -> &'static str {
        match self {
            Self::YtDlp => "yt-dlp",
            Self::Ffmpeg => "ffmpeg",
        }
    }

    pub fn system_name(&self) -> &'static str {
        match self {
            Self::YtDlp => "yt-dlp",
            Self::Ffmpeg => "ffmpeg",
        }
    }
}

/// Resolves the best command to use for a given binary.
/// Checks for a bundled sidecar first, then falls back to the system PATH.
pub fn resolve_bin(app: &AppHandle, bin: Binary) -> Command {
    let sidecar_name = bin.name();
    
    // Check if sidecar exists and is runnable
    // Tauri's sidecar() will handle the target triple automatically.
    match app.shell().sidecar(sidecar_name) {
        Ok(cmd) => cmd,
        Err(_) => {
            // Fallback to system command
            app.shell().command(bin.system_name())
        }
    }
}

/// Checks if a binary is available (either as sidecar or in system path)
pub fn is_bin_available(app: &AppHandle, bin: Binary) -> bool {
    // Check sidecar
    if app.shell().sidecar(bin.name()).is_ok() {
        return true;
    }

    // Check system path using standard library
    StdCommand::new(bin.system_name())
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
