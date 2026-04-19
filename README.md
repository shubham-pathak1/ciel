# Ciel

Ciel is a Windows download manager built with Tauri, Rust, and React.

It focuses on two things:
- fast HTTP downloads with parallel connections
- torrent downloads with pause, resume, and file selection

Ciel is still moving toward a proper public beta, but the core app is already usable and the recent work has been focused on stability rather than feature sprawl.

## What Ciel currently does
- Parallel HTTP downloading
- Safe fallback to single connection when a server rejects range requests
- HTTP pause/resume and restart recovery
- Magnet torrent support
- Torrent file selection before starting
- Torrent pause/resume in the same session
- Torrent resume after restart and crash recovery improvements
- Clipboard autocatch
- Download scheduler
- Category-based organization
- Local-first settings and SQLite-backed state

## Current focus
Right now the project is mainly about making the existing downloader more reliable:
- cleaner HTTP fallback behavior
- better torrent restore and verification UX
- fewer misleading states in the UI
- getting the app ready for a stable beta

## Notes
- Ciel is sidecar-free.
- The HTTP engine is custom.
- Torrent support is powered by `librqbit`.
- YouTube / `yt-dlp` support was intentionally removed to keep the app leaner and easier to maintain.

## Development
```bash
npm install
npm run tauri dev
```

Production build:
```bash
npm run tauri build
```

If you want a separate Rust target directory, copy:
- `src-tauri/.cargo/config.toml.example`

to:
- `src-tauri/.cargo/config.toml`

and set your preferred `target-dir`.

## Contributing
Issues and pull requests are welcome. If you run into a bug, a clear repro and logs help a lot.

## License
MIT
