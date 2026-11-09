# Changelog

All notable changes to **StreamVault** will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [1.1.0] - 2026-11-08

### Added
- **Tier 1 — Series & Batch Workflows**:
  - **Auto-Play Next Episode & Playlist Queue**: Automatically scans sibling episode/media links on directory pages in DOM order, adds `⏮ Prev`, `⏭ Next`, `Ep X / N` badge, and `☰ Queue` modal, and auto-advances on natural `MPV_END_FILE_REASON_EOF` (configurable via `auto_play_next_episode`).
  - **Auto-Matched Server & Local Sidecar Subtitles**: Automatically discovers and attaches stem-matching `.srt`, `.ass`, `.ssa`, `.vtt`, and `.sub` subtitles (including language suffixes like `Show.S01E02.en.srt`) from both remote HTTP directory pages and local folders (`auto_load_matching_subtitle`).
  - **Batch Media Grabber (`⬇ Grab`)**: Scans the active browser tab for all video, audio, and subtitle links with live count badge, filename filter, media/subtitle checkboxes, single destination folder picker, **Queue Selected Downloads** (honoring `max_concurrent_downloads` and `download_speed_limit_kbps`), and **Play Selected as Playlist**.
  - **Direct Local File Open & Drag-and-Drop**: Open (`📂 Open File` / `Ctrl+O`) or drag-and-drop local `.mkv`, `.mp4`, `.avi`, `.mp3`, `.flac`, etc. directly into `libmpv` bypassing the HTTP streaming cache, plus live drag-and-drop of `.srt`/`.ass`/`.vtt` subtitles onto active playback.
- **Tier 2 — `libmpv` Cinema Controls**:
  - **Subtitle & Audio Sync + Subtitle Scale (`⏱ Sync`)**: Live `sub-delay` (`Z`/`X` ±100ms + modal controls), `audio-delay` lip-sync offset, and `sub-scale` (`50%`–`240%`).
  - **MKV Chapter Markers & Hover Titles**: Parses `chapter-list` into timeline chapter tick markers, displays `"MM:SS • Chapter Title"` inside the native Win32 `StreamVaultTimeTip` hover popup, and supports `PageUp` / `PageDown` chapter jumps.
  - **One-Key Frame Screenshot (`📷` / `S`)**: Captures PNG screenshots via `libmpv` `screenshot-to-file` into `<default_download_dir>/Screenshots/` with a one-click **Reveal** toast button (`SHOpenFolderAndSelectItems`).
  - **Night / Dialogue Boost (`🌙 Boost`)**: One-click `lavfi=[dynaudnorm=f=150:g=15:p=0.92]` dynamic audio normalizer for clear quiet speech and tamed loud peaks.
- **Tier 3 — Browser & Open-Source Polish**:
  - **Browser `Ctrl+F` Find-in-Page & Middle-Click Background Tabs**: Non-destructive CSS Custom Highlight Find-in-Page bar (`#browser-find-bar`) with match counts and `Enter`/`Shift+Enter` stepping, plus middle-click (`button === 1`) and `Ctrl+Click` background tab opening (`active: false`) on webpage links and bookmarks.
  - **Windows System Media Transport Controls (`SMTC`) & Hardware Media Keys**: WinRT `ISystemMediaTransportControlsInterop::GetForWindow` integration + `WM_APPCOMMAND` / `VK_MEDIA_*` routing for OS Volume/Media Flyout metadata and hardware `Play/Pause`, `Next`, `Previous`, and `Stop` keys.
  - **Portable-Safe GitHub Update Notifier**: On-demand GitHub Releases version check in *Settings → Portability & Runtime* with explicit preservation of `data\` and the single rolling `data\backups\streamvault.backup.db` backup.

---

## [1.0.0] - 2026-11-08

### Added
- **Portable Architecture (`streamvault-core/portable.rs`)**: 100% self-contained folder layout (`data/{database,settings,cache/media,downloads,logs,backups,webview2}`) with startup write-permission checks, free-space diagnostics, and zero `%APPDATA%` pollution.
- **Multi-Tab WebView2 Browser (`src-tauri/browser_tabs.rs`, `ui/js/browser.js`)**: Tab management, Back/Forward/Reload/Home navigation, omnibox with Bookmark & History autocomplete, isolated `shell` vs `tabs` WebView2 profiles, and off-screen instant tab switching.
- **Smart Media Link Detection (`media_detect.rs`, `tab_content_script.js`)**: Right-click webpage media actions (*Play in StreamVault*, *Open in External Player*, *Download Media*, *Copy Media URL*), HTTP `Range`/`HEAD` MIME probing, and live active-media link highlighting.
- **Hardware-Accelerated `libmpv` Player (`mpv_engine.rs`, `ui/js/player.js`)**:
  - Dynamic `libmpv-2.dll` C ABI integration on a dedicated Direct3D 11 (`gpu-context=d3d11`, `hwdec=auto-safe`) Win32 surface (`StreamVaultMpvSurface`).
  - Flicker-free Windows 10/11 DWM fullscreen (`d3d11-flip=yes` composition with 1px DWM DirectFlip guard, `d3d11-exclusive-fs=no`, `WM_ERASEBKGND` suppression, Z-order-verified viewport deduplication, and idle WebView2 repaint suppression).
  - Native DPI-scaled Win32 popup **Time Tooltip** (`StreamVaultTimeTip`) rendered cleanly over the video frame on timeline hover.
  - Topmost floating **Picture-in-Picture (`PiP`)** window, audio/subtitle track selector, external `.srt`/`.ass`/`.vtt` loader, aspect ratio & speed (`0.25x`–`3.0x`) controls, and playback resume memory.
- **Progressive HTTP `Range` Streaming Cache (`stream_cache.rs`)**: Disk-backed sparse `.svcache` streaming with Matroska `Cues` / MP4 `moov` tail side-fetching, buffered-interval timeline rendering, non-Range server fallback banner, active-session lock protection, exit cleanup, and startup crash orphan recovery.
- **Resumable Permanent Download Manager (`downloader.rs`, `ui/js/downloads.js`)**: Isolated from temporary cache, native Windows Save As dialog with exact server filename preservation, `.svpart` atomic completion, `Range` + `If-Range` Pause/Resume/Retry, Open File, and native Win32 `SHOpenFolderAndSelectItems` Explorer reveal.
- **External Media Player Integration (`external_player.rs`)**: Automatic Registry/path detection for PotPlayer, VLC, MPC-HC, MPC-BE, KMPlayer, MPV, and Windows Media Player plus custom `.exe` support.
- **Bookmarks, History, Recent Media & Settings**: Hierarchical bookmark folders, bookmark bar, Grid/List Recent Media views, native Win32 clipboard copy, live sidebar network throughput monitor (`GetIfTable2`), and single-file atomic SQLite online backup (`data/backups/streamvault.backup.db`) with automatic corruption recovery.
