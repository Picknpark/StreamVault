# StreamVault — Progressive Streaming Cache & Download Isolation Architecture

This document details how StreamVault streams large direct HTTP media files (such as multi-gigabyte BDIX/LAN `.mkv` movies with complex percent-encoded or Unicode URLs) instantaneously without buffering whole files in RAM, and how temporary streaming data is strictly isolated from permanent user downloads.

---

## 1. End-to-End Progressive Streaming Pipeline (`stream_cache.rs`)

```text
Remote HTTP/HTTPS Media Server (e.g. http://172.16.50.12/.../Movie%20(1080p)%20[Dual].mkv)
        │
        │  1. Async HTTP Range requests (`Range: bytes=<start>-`)
        │     streamed in 256 KiB chunks (constant ~2–4 MiB RAM footprint)
        ▼
┌────────────────────────────────────────────────────────────────────────────┐
│ StreamCacheEngine (`crates/streamvault-core/src/stream_cache.rs`)          │
│                                                                            │
│  • Active Session Lock (`active_sessions` registry)                        │
│  • Sparse Disk File: `StreamVault/data/cache/media/session_<uuid>.svcache` │
│  • Coalesced Byte-Interval Map: `session_<uuid>.meta.json`                 │
│    tracks exact downloaded `[start, end)` intervals on disk                │
│  • Loopback HTTP/1.1 Range Proxy (`http://127.0.0.1:<ephemeral>/stream/id`)│
│    ├─ Serves already-cached byte spans directly from `.svcache` on disk    │
│    ├─ Dedicated Tail Side-Fetcher (<= 8 MiB near EOF) so `libmpv` reads    │
│    │  Matroska `Cues` / MP4 `moov` atoms at end-of-file in milliseconds    │
│    │  without interrupting the main forward progressive download           │
│    └─ Uncached Mid-File Seek Repositioning: signals main downloader via    │
│       `seek_tx` channel to jump immediately to the user's target byte      │
└────────────────────────────────────────────────────────────────────────────┘
        │
        │  2. Local HTTP/1.1 `206 Partial Content` stream
        ▼
Built-in `libmpv` Engine (`mpv_engine.rs` -> Win32 `HWND` D3D11 Video Surface)
```

### Key Engineering Properties

1. **Instant Playback Start**:
   `libmpv` begins demuxing as soon as the initial header bytes (~64 KiB–512 KiB) land in `.svcache`. Playback never waits for a multi-GB file to finish downloading.
2. **Zero Full-File RAM Buffering**:
   Network chunks (`bytes::Bytes`) are written directly to `.svcache` via `SeekFrom::Start(offset)` + `write_all` and dropped immediately. Streaming a 25 GB 4K MKV uses the same tiny ~4 MiB Rust memory footprint as a 10 MB MP3.
3. **Smart Matroska (`MKV`) & MP4 Tail Indexing**:
   When opening an MKV or non-faststart MP4, `libmpv` issues a rapid `Range: bytes=<total - N>-` request to read the Matroska `Cues` seek table or MP4 `moov` atom at the end of the file, and then immediately returns to byte `0`. `StreamCacheEngine` detects tail requests (`total_len - req_start <= 8 MiB` while uncached) and satisfies them with a bounded dedicated HTTP `Range` fetch written into `.svcache`, keeping the primary sequential stream at byte `0` uninterrupted.
4. **Arbitrary Timeline Seeking (`HTTP 206 Partial Content`)**:
   - **Cached Region Seek**: Served instantaneously from local disk (`.svcache`) with zero network traffic.
   - **Uncached Region Seek (`supports_range == true`)**: `serve_client` sends `seek_tx.send(req_start)`, causing the upstream worker to abort its previous stream and open a new `Range: bytes=<req_start>-` request at the exact target offset, notifying waiting readers via ` tokio::sync::Notify` as bytes arrive.
5. **Graceful Non-Range Server Fallback (`supports_range == false`)**:
   - If the upstream server responds with `200 OK` instead of `206 Partial Content` on a Range probe, `StreamCacheEngine` marks `supports_range = false` and surfaces a clear warning badge in the player UI (*"Linear Stream (Server lacks HTTP Range — forward seeking restricted)"*).
   - Backward seeks into already-buffered disk bytes still work; uncached forward seeks beyond a small jump window fail fast instead of hanging the player.
6. **Network Stall Retry**:
   Transient socket drops on Range-capable servers automatically retry up to 4 times with exponential backoff from the exact `cursor` byte without corrupting `.svcache` or interrupting `libmpv`.

---

## 2. Lifecycle, Active Protection & Crash Recovery

| Event | Behavior |
|---|---|
| **Active Playback** | Session ID is registered in `active_sessions`; `clear_inactive_cache()` skips all `.svcache` and `.meta.json` files belonging to active sessions. |
| **Switching Media / Stopping Player** | Previous session is closed and its `.svcache` + `.meta.json` files are deleted immediately to reclaim disk space. |
| **Normal Application Exit** | `StreamVaultCore::graceful_shutdown()` stops `libmpv`, closes active sessions, and—when `clear_temp_cache_on_exit` is enabled (default `true`)—deletes all temporary files in `data/cache/media/`. |
| **Unexpected Crash / Power Loss** | On next launch, `StreamCacheEngine::recover_orphaned_cache_on_startup()` scans `data/cache/media/`, logs recovered orphan count and bytes to `data/logs/streamvault.log`, and removes leftover `.svcache` / `.meta.json` / `.tmp` files. |

---

## 3. Strict Isolation Between Temporary Cache & Permanent Downloads

StreamVault enforces an architectural wall between temporary streaming and permanent saving:

| Dimension | Temporary Streaming Cache (`stream_cache.rs`) | Permanent Downloads (`downloader.rs`) |
|---|---|---|
| **Storage Path** | `StreamVault/data/cache/media/session_<uuid>.svcache` | User-chosen destination via Windows **Save As** dialog (default `StreamVault/data/downloads/<Original Server Filename>`) |
| **Filename Policy** | Internal UUID session filename | Exact original server filename preserved (`Content-Disposition` `filename*` / `filename` or percent-decoded URL segment, preserving spaces, `()`, `[]`, Unicode) |
| **Duplicate Protection** | Ephemeral per session | Never silently overwrites existing files; auto-selects `Movie (1).mkv` when saving to default folder |
| **In-Progress Format** | Sparse `.svcache` + `.meta.json` | Sequential `<Filename>.svpart` renamed atomically on completion |
| **Pause / Resume** | Managed automatically by playback engine | User-controlled **Pause**, **Resume** (`Range` + `If-Range`), **Retry**, and **Cancel** persisted in `download_history` table |
| **Cleanup Policy** | Deleted automatically on stop/exit & crash recovery | **NEVER** touched by cache cleanup, Recent Media clear, or application exit |
