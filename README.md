# Ciel

Ciel is a high-performance, open-source download manager for Windows built with **Tauri** and **Rust**. I've designed it to provide a clean, bloat-free experience focused on core efficiency and ease of use.

## Core Features
- **Parallel Downloading**: Optimized multi-threaded HTTP engine with segment-based chunk management.
- **Torrent Support**: Full magnet link support with content preview and metadata polling.
- **Clipboard Monitoring**: "Autocatch" technology detects URLs in your clipboard for seamless link addition (respects privacy settings).
- **Smart Categorization**: Automatically organizes downloads into Music, Archives, Software, and Documents based on file extensions.
- **Download Scheduler**: Plan your queue to start or pause at specific times for better bandwidth management.

## Advanced Capabilities
- **Session Support**: Provide custom User-Agents and Cookies to bypass restrictions on premium file hosts.
- **Automation**: Automatic folder reveal upon completion, system shutdown options, and sound notifications.
- **Privacy Focus**: Completely offline-first. No tracking, no telemetry, no accounts. All data stays local.

## Reason behind removing yt-dlp(youtube downloads feature)
Ciel previously included a specialized YouTube download feature integration via `yt-dlp` and `FFmpeg`. However, due to the constant changes in platform bot-detection, cookie-locking, and the instability of maintaining specialized sidecars (which bloated the binary by over 100MB), I have made the strategic decision to **strip this feature** from the core application. 

My goal is to keep Ciel as a **lean, 100% stable, and sidecar-free** download manager. 

> [!TIP]
> **Extension Support?** If the community demands it, I may release a separate YouTube/Video extension that adds these sidecars back as optional plugins. For now, Ciel remains focused on being the fastest and most reliable manager for Direct (HTTP) and Torrent downloads.

## Internal Engines
Ciel is now completely **sidecar-free**. 
- **HTTP Engine**: A custom Rust-based multi-threaded downloader.
- **Torrent Engine**: Powered by a lightweight, memory-efficient BitTorrent implementation.

## Development Setup

I welcome contributions! As a solo developer, I'm always looking for extra hands to help polish features or fix bugs. To get started with the development environment:

```bash
# 1. Install dependencies
npm install

# 2. Run in development mode
npm run tauri dev

# 3. Build for production (Windows)
npm run tauri build
```

### Custom Build Directory
Rust target directories can be quite large. You can customize your build path:
1. Copy `src-tauri/.cargo/config.toml.example` to `src-tauri/.cargo/config.toml`.
2. Update the `target-dir` key to your preferred location.

## Contributing
Ciel is an open-source project. If you find a bug or have a suggestion, please feel free to open an issue or submit a pull request. As I'm maintaining this project on my own, I truly appreciate any help in making Ciel better for everyone.

## License
MIT
