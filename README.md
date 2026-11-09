# StreamVault

<p align="center">
  <strong>Portable 64-bit Windows Browser, Progressive Media Streamer &amp; Hardware-Accelerated <code>libmpv</code> Player</strong>
</p>

<p align="center">
  <img alt="Platform: Windows 10/11 x64" src="https://img.shields.io/badge/Platform-Windows%2010%2F11%20(x64)-0078D4?logo=windows" />
  <img alt="Portable: Zero Install" src="https://img.shields.io/badge/Deployment-100%25%20Portable%20(No%20AppData)-10b981" />
  <img alt="Built with Rust & Tauri 2" src="https://img.shields.io/badge/Rust%20%2B%20Tauri-v2-f97316?logo=rust" />
  <img alt="Media Engine: libmpv D3D11" src="https://img.shields.io/badge/Media%20Engine-libmpv%20(D3D11VA)-6366f1" />
  <a href="./LICENSE"><img alt="License: MIT" src="https://img.shields.io/badge/License-MIT-38bdf8.svg" /></a>
</p>

---

**StreamVault** is a self-contained, zero-installer 64-bit Windows 10/11 desktop application built in **Rust + Tauri 2** with a lightweight **Vanilla HTML/CSS/JavaScript** UI. It unifies a real multi-tab **WebView2** browser, smart direct-media link detection (ideal for LAN/BDIX/HTTP directory servers as well as the public web), an embedded **Direct3D 11 `libmpv`** media player with progressive disk-backed HTTP `Range` streaming, external player handoff, and an isolated resumable download manager—all storing **100% of state inside its own portable folder**.

---

## Key Features

### 1. Multi-Tab Portable WebView2 Browser
- Chromium-powered multi-tab browsing across public HTTPS sites and private HTTP/LAN/BDIX media index servers, with **Middle-Click / `Ctrl+Click` Background Tabs** and **`Ctrl+F` Find-in-Page** highlighting.
- Built-in **Batch Media Grabber (`⬇ Grab`)** to filter and queue whole seasons of episodes + subtitles into a single folder (with configurable concurrency cap & speed limit) or launch them as a playlist.
- Address bar omnibox with Bookmark & History autocomplete, Bookmark Bar, hierarchical Bookmark Folder Manager, and Browsing History with time-range clearing.
- **Strict Portable Profile Isolation**: Shell UI (`data/webview2/shell`) and remote browser tabs (`data/webview2/tabs`) are completely isolated inside the portable folder—zero `%LOCALAPPDATA%` footprint.

### 2. Smart Media Link Detection & Webpage Highlighting
- Detects direct video (`MKV, MP4, AVI, MOV, WMV, FLV, WEBM, M4V, MPEG`) and audio (`MP3, FLAC, WAV, AAC, M4A, OGG`) links via URL inspection and non-destructive HTTP `HEAD` / `Range: bytes=0-1` probing.
- Right-click any media link on a webpage to **Play in StreamVault**, **Open in External Player**, **Download Media**, or **Copy Media URL**.
- Highlights the currently active media link directly inside the webpage so you never lose track of which episode or movie is playing.

### 3. Built-In Hardware-Accelerated `libmpv` Player (`D3D11`)
- Dynamically loads `runtime/media-engine/libmpv-2.dll` onto a dedicated Win32 child surface (`gpu-context=d3d11`, `hwdec=auto-safe`) supporting MKV, HEVC/H.265, AV1 (`dav1d`), H.264, VP9, styled ASS/SSA/SRT/VTT subtitles, FLAC, DTS, AAC, and Opus.
- Timeline buffered-range visualization, **MKV Chapter Markers** with chapter titles inside the native Win32 hover time tooltip, **Episode Playlist Queue & Auto-Play Next**, **Auto-Matched Server & Local Subtitles**, **Subtitle/Audio Sync (`Z`/`X`) & Subtitle Scale**, **Night / Dialogue Boost (`dynaudnorm`)**, **One-Key Frame Screenshots (`S`)**, **Local File Open & Drag-and-Drop (`Ctrl+O`)**, **Windows Hardware Media Keys & SMTC Flyout**, flicker-free fullscreen with auto-hiding HUD, and topmost floating **Picture-in-Picture (`PiP`)** window.
- Automatic per-media **Playback Position Memory** with Resume / Start-from-00:00 prompt and Grid/List **Recent Media** library.

### 4. Instant Progressive HTTP `Range` Disk Streaming Cache
- Starts playback of multi-gigabyte `.mkv` / `.mp4` streams in milliseconds—never waits for full file download and never buffers whole files in RAM (~4 MiB constant memory overhead).
- Pre-fetches Matroska `Cues` / MP4 `moov` tail metadata via a dedicated bounded Range request so full-timeline seeking works immediately on large MKV files.
- Protects active playback sessions, cleans temporary `.svcache` files automatically on stop/exit, and recovers orphaned cache files after unexpected power loss.

### 5. Resumable Permanent Download Manager (Strictly Isolated from Cache)
- Native Windows **Save As** dialog pre-populated with exact server filenames (`Content-Disposition` RFC 5987/6266 or decoded URL filename, preserving spaces, `()`, `[]`, and Unicode).
- Sequential `.svpart` downloads with `Range` + `If-Range` **Pause / Resume / Retry**, duplicate-safe naming (`Movie (1).mkv`), **Open File**, and native Windows Explorer **Show in Folder** (`SHOpenFolderAndSelectItems`).

### 6. External Media Player Auto-Discovery
- Automatically detects installed **PotPlayer**, **VLC**, **MPC-HC**, **MPC-BE**, **KMPlayer**, **MPV**, and **Windows Media Player** via Windows Registry and standard install directories, plus user-added custom `.exe` players.
- Passes exact raw URLs directly via OS process argument vectors (`Command::arg`) without `cmd.exe` shell mangling or `%20` corruption.

### 7. Crash-Resilient Portable SQLite Storage
- Embedded SQLite 3 database (`WAL` mode) storing Bookmarks, History, Recent Media, Resume Positions, Downloads, Session Tabs, and Settings inside `data/database/streamvault.db`.
- Startup `PRAGMA quick_check` integrity verification with automatic recovery from a single, atomically overridden backup (`data/backups/streamvault.backup.db`).

---

## Download & Quick Start (End Users)

1. Download the latest **`StreamVault-v*-win64-portable.zip`** from [**GitHub Releases**](../../releases).
2. Extract the `StreamVault` folder to **any writable directory** on 64-bit Windows 10 (1809+) or Windows 11 (e.g., `D:\StreamVault`, Desktop, or an external USB SSD).
   > **Note**: Do **not** place a portable folder inside `C:\Program Files`, as Windows UAC blocks portable writes to `data\` without elevation.
3. *(Optional)* Double-click `Check-Dependencies.bat` to verify folder write permissions, WebView2 runtime, and `libmpv-2.dll`.
4. Double-click **`StreamVault.exe`**.

> **Windows Defender SmartScreen Note**: Because StreamVault is a free, community open-source project distributed without a commercial EV code-signing certificate, Windows SmartScreen may display *"Windows protected your PC"* on first launch. Click **More info → Run anyway**, or verify the archive against `SHA256SUMS.txt` on the Releases page (or compile `StreamVault.exe` directly from source below).

---

## Player Keyboard & Mouse Controls

| Input | Action |
|---|---|
| `Space` / Single-Click Video | Play / Pause |
| `Left` / `Right` Arrow | Seek backward / forward (default `10s`, configurable in Settings) |
| `Up` / `Down` Arrow / Mouse Wheel | Volume up / down (`0%`–`150%`) |
| `M` | Mute / Unmute |
| `F` / Double-Click Video | Toggle Fullscreen |
| `Esc` | Exit Fullscreen / Close Find Bar |
| `[` / `]` | Decrease / Increase Playback Speed |
| `Z` / `X` | Subtitle Sync Delay `-100 ms` / `+100 ms` |
| `S` | Capture Video Frame Screenshot (`.png`) |
| `N` / `P` | Next / Previous Episode in Playlist Queue |
| `PageDown` / `PageUp` | Next / Previous MKV Chapter |
| `Ctrl+O` | Open Local Video / Audio / Subtitle Files |
| `Ctrl+F` | Find in Webpage |
| Hardware Media Keys | `Play/Pause`, `Next`, `Previous`, `Stop` (Windows SMTC) |

---

## Repository Structure

```text
streamvault/
├── Cargo.toml                        # Rust workspace manifest
├── crates/
│   └── streamvault-core/             # Core Rust engine + integration tests
│       ├── src/
│       │   ├── db/                   # SQLite WAL engine, migrations & single atomic backup
│       │   ├── downloader.rs         # Resumable permanent download manager (.svpart)
│       │   ├── external_player.rs    # Registry/path player discovery & safe launcher
│       │   ├── logging.rs            # Rotating JSONL structured logger
│       │   ├── media_detect.rs       # URL & HTTP Range/MIME media link classifier
│       │   ├── mpv_engine.rs         # libmpv C ABI bridge, D3D11 surface, PiP & native Win32 tooltip
│       │   ├── net_monitor.rs        # Live interface throughput sampler
│       │   ├── portable.rs           # Portable folder layout & runtime checks
│       │   └── stream_cache.rs       # Progressive HTTP Range disk cache & loopback proxy
│       └── tests/
│           └── integration_suite.rs  # End-to-end automated test suite
├── src-tauri/                        # Tauri 2 Windows shell & capability security boundary
│   ├── capabilities/                 # Strict ACL separation: shell-local vs tab-media-context
│   └── src/
│       ├── main.rs                   # Portable WebView2 bootstrap, telemetry loop & shutdown
│       ├── browser_tabs.rs           # Multi-tab child WebView2 manager
│       ├── commands.rs               # Strongly typed IPC command handlers
│       └── tab_content_script.js     # Nonce-authenticated media link context bridge
├── ui/                               # Zero-bundler modular HTML + CSS + Vanilla JS UI
│   ├── index.html
│   ├── css/
│   └── js/
├── runtime-src/                      # Minimal 2 KB Vulkan fallback stub for GPU-less VMs
├── packaging/                        # Portable bundle templates & dependency checker scripts
├── scripts/                          # Clean portable release ZIP & SHA-256 packager
└── docs/                             # Architecture, licensing & GitHub publishing guides
```

---

## Building from Source

StreamVault requires **zero Node.js build steps**—the `ui/` assets are plain modular HTML/CSS/JS embedded directly by `tauri-build`.

### Native Windows 10/11 Build (`MSVC`)

```powershell
# 1. Run automated integration tests
cargo test -p streamvault-core

# 2. Compile optimized 64-bit Windows GUI binary
cargo build --manifest-path src-tauri/Cargo.toml --target x86_64-pc-windows-msvc --release

# 3. Assemble clean portable folder + ZIP (requires libmpv-2.dll path or existing runtime folder)
powershell -ExecutionPolicy Bypass -File scripts\package-portable-release.ps1 `
  -MpvDllPath "C:\path\to\libmpv-2.dll"
```

For Linux-to-Windows cross-compilation (`cargo-xwin` / `clang-cl` + `lld-link`) and detailed runtime notes, see:
- [`BUILD_INSTRUCTIONS.md`](./BUILD_INSTRUCTIONS.md)
- [`STREAMING_ARCHITECTURE.md`](./STREAMING_ARCHITECTURE.md)
- [`WINDOWS_DEPENDENCIES.md`](./WINDOWS_DEPENDENCIES.md)

---

## Security & Privacy

- **Zero Telemetry / Zero Cloud Sync**: StreamVault makes zero analytics, crash-reporting, or background calls; network traffic occurs exclusively to URLs you browse, stream, or download.
- **Strict Tauri v2 Capability Isolation**: External webpages loaded inside `tab-*` WebView2 instances cannot invoke privileged Rust IPC commands. Only the local `main` shell webview holds `shell-local` permissions (`src-tauri/capabilities/`).
- See [`SECURITY.md`](./SECURITY.md) for full architecture details and vulnerability reporting.

---

## Contributing & License

Contributions, bug reports, and feature ideas are welcome! Please read [`CONTRIBUTING.md`](./CONTRIBUTING.md) and [`CODE_OF_CONDUCT.md`](./CODE_OF_CONDUCT.md) before submitting a Pull Request.

- **StreamVault Source Code**: Licensed under the [**MIT License**](./LICENSE).
- **Third-Party Components & Bundled `libmpv-2.dll` (`LGPL-2.1-or-later`)**: See [`THIRD_PARTY_NOTICES.md`](./THIRD_PARTY_NOTICES.md).
