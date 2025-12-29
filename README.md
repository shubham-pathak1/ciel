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
- [x] Multi-connection HTTP/HTTPS downloads
- [x] Magnet link & Torrent support
- [x] Auto-resolving download paths
- [x] Minimalist "Claude Desktop-inspired" UI

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
