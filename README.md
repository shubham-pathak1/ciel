# Ciel

> **Status**: 🚧 In Development (Alpha)

**Ciel** is a high-performance download manager made using tauri. Built for users who value speed, privacy, and distraction-free software.

## Philosophy
- **Performance**: Native Rust backend via Tauri.
- **Privacy**: Zero telemetry. All data stays local.

## Tech Stack
- **Core**: Tauri v2 + Rust
- **Frontend**: React + TypeScript + Vite
- **Styling**: Tailwind CSS (Zinc/Slate theme)
- **Engine**: `librqbit` (Torrents) + `reqwest` (HTTP)

## Features
- [x] **Smart Link Resolution**: Automatically resolve Google Drive links and bypass virus scan confirmations.
- [x] **HTTP Download Preview**: Interactive metadata preview (filename, size) before starting a download.
- [x] **Multi-connection Engine**: High-speed multi-threaded downloads via `reqwest`.
- [x] **Selective Torrenting**: Detailed file selection for Magnet links and `.torrent` files.
- [x] **Dynamic Path Resolution**: Smart handling of output folders and file naming.

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
