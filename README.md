# Ciel

> **Status**: 🚧 In Development (Alpha)

**Ciel** is a high-performance download manager built with Tauri. Designed for users who value speed, privacy, and distraction-free software.

## Philosophy
- **Performance**: Native Rust backend via Tauri 2.0
- **Privacy**: Zero telemetry. All data stays local.
- **Minimalism**: Clean, focused UI without unnecessary bloat.

## Tech Stack
- **Core**: Tauri v2 + Rust
- **Frontend**: React + TypeScript + Vite
- **Styling**: Tailwind CSS (Monochrome Zinc/Slate theme)
- **Engines**: `librqbit` (Torrents) + `reqwest` (HTTP)

## Features

### Downloads
- **Multi-Connection HTTP Engine**: High-speed parallel downloads with automatic retry
- **Smart Filename Extraction**: RFC 5987 compliant parsing of `Content-Disposition` headers
- **Configurable Download Location**: Choose default folder or ask every time

### Torrents
- **Selective File Download**: Preview and select specific files from torrents before downloading
- **Metadata Preview**: View torrent contents and file sizes before committing
- **Peer Statistics**: Real-time peer count and download speed

### UI/UX
- **Monochrome Design**: Clean, matte dark theme inspired by modern desktop apps
- **Progress Tracking**: Visual progress bars with speed, ETA, and connection count
- **Open in Folder**: One-click access to downloaded files in explorer

## Development

```bash
# Install dependencies
npm install

# Run in development mode
npm run tauri dev

# Build production bundle
npm run tauri build
```

## License
MIT
