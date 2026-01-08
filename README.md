# Ciel

> **High-performance, minimal download manager for Windows.** Built with Tauri & Rust.

Ciel is designed for users who demand extreme speed, privacy, and a distraction-free environment. It strips away the bloat of traditional download managers, focusing on core throughput and a premium user experience.

---

## 🔥 Key Features

### ⚡ Extreme Throughput
- **64-Thread HTTP Engine**: Multi-connection parallel downloading with intelligent chunk management for wire-speed performance.
  > [!CAUTION]
  > Using extremely high thread counts (e.g., 64) may lead to temporary IP bans or rate-limiting by major platforms like YouTube or Google Drive. Use with discretion.
- **Fast Pre-allocation**: Uses `tokio::fs::File::set_len` for instant file creation, preventing disk fragmentation.
- **Smart Headers**: RFC 5987 compliant parsing for accurate filename extraction even on complex servers.

### 🎥 Pro Video Integration
- **Universal Support**: Powered by `yt-dlp`, supporting YouTube, Twitter, Instagram, and 1000+ other sites.
- **Ultra-HD Quality**: Download up to 8K resolution with support for VP9/AV1 codecs.
- **Auto-Muxing & Subs**: Automatic high-quality audio merging and subtitle embedding via FFmpeg.
- **Fast Analysis**: Optimized handshakes for nearly instant video metadata retrieval.

### 🔗 Autocatch (Clipboard Monitoring)
- **Instant Detection**: Monitor the clipboard for HTTP, Magnet, and Video URLs.
- **Quiet Workflow**: A minimal, non-intrusive prompt allows you to add downloads without leaving your browser.

### 🧪 Advanced Torrents
- **Selective Fetching**: Preview torrent contents and select only the files you need.
- **Metadata Polling**: Robust analysis of magnet links before the download begins.

### 🎨 Premium Desktop Experience
- **Fluid UI**: Built with React & Framer Motion for smooth, layout-aware transitions.
- **Native Look**: Deep Windows integration with Mica effect, custom title bars, and "Show in Folder" shortcuts.
- **Persistent Queue**: SQLite-backed queue that survives app restarts and computer crashes.

---

## 🛠️ Tech Stack
- **Backend**: Tauri v2 + Rust (`tokio`, `reqwest`, `librqbit`)
- **Frontend**: React + TypeScript + Vite
- **Styling**: Vanilla CSS + Tailwind (Custom Matte Zinc/Slate theme)
- **Persistence**: SQLite (via `rusqlite`)

---

## 🚀 Getting Started

> [!IMPORTANT]
> For high-resolution video downloads (1080p+), ensure `yt-dlp` and `ffmpeg` are installed and added to your system PATH.

```bash
# 1. Install dependencies
npm install

# 2. Run in development mode
npm run tauri dev

# 3. Build for production (Windows)
npm run tauri build
```

### 🛠️ Advanced: Build Customization (Optional)
Rust build directories (`target`) can grow quite large. To redirect compilation to a different drive (e.g., to save space on `C:` or improve performance):
1. Copy `src-tauri/.cargo/config.toml.example` to `src-tauri/.cargo/config.toml`.
2. Edit the `target-dir` path to your preferred location.

## 🔒 Privacy
Ciel is 100% offline-first. No tracking, no telemetry, no accounts. All download history and settings are stored in a local SQLite database in your app data directory.

## 📄 License
MIT
