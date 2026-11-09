================================================================================
  StreamVault v1.1.0 — Portable 64-bit Windows Browser & Media Streaming Player
================================================================================

1. QUICK START (ZERO INSTALLATION REQUIRED)
-------------------------------------------
1. Copy or extract this entire `StreamVault` folder to any writable location on
   64-bit Windows 10 or Windows 11 (for example: Desktop, `D:\StreamVault`, or
   an external USB SSD).
   IMPORTANT: Do NOT place a portable folder inside `C:\Program Files` unless
   running as Administrator, because Windows blocks portable database/cache
   writes inside Program Files.
2. (Optional) Double-click `Check-Dependencies.bat` to verify your Windows 10/11
   WebView2 runtime, folder write permissions, and bundled `libmpv-2.dll`.
3. Double-click `StreamVault.exe` to start browsing, streaming, and downloading!

2. PORTABLE DIRECTORY LAYOUT (NO APPDATA DEPENDENCY)
----------------------------------------------------
StreamVault stores 100% of its persistent state inside its own `data\` folder:

  StreamVault\
    StreamVault.exe                       Main 64-bit Windows GUI executable
    Check-Dependencies.bat                One-click Windows runtime checker
    README.txt                            This quick-start guide
    data\
      database\
        streamvault.db                    SQLite database (Bookmarks, History,
                                          Recent Media, Resume Positions,
                                          Downloads, Settings, Session Tabs)
      settings\
        portable-mode.json                Portable configuration descriptor
      cache\
        media\                            TEMPORARY progressive streaming cache
                                          (auto-cleaned when StreamVault exits)
      downloads\                          Default PERMANENT downloads directory
      logs\
        streamvault.log                   Rotating structured diagnostic logs
      backups\
        streamvault.backup.db             Single rolling SQLite backup (overridden
                                          atomically on each checkpoint/update)
      webview2\                           Self-contained WebView2 browser profile
    runtime\
      media-engine\
        libmpv-2.dll                      Bundled 64-bit mpv/FFmpeg media engine
        vulkan-1.dll                      2 KB VM fallback stub (real GPU PCs use
                                          System32\vulkan-1.dll automatically)
        LICENSE-libmpv.txt                LGPL v2.1+ license & build notice
      check-windows-dependencies.ps1      PowerShell diagnostic script

Moving the entire `StreamVault` folder to another drive or another Windows PC
preserves your Bookmarks, Recent Media, Resume Positions, Download History,
Browser Tabs, and Settings automatically.

3. CORE WORKFLOWS
-----------------
A. General & Local-Network Web Browsing:
   - Browse any public HTTPS website (`https://youtube.com`, `https://google.com`,
     `https://facebook.com`) or private HTTP/BDIX/LAN media index server
     (e.g. `http://172.16.50.12/` or `http://10.16.100.244/`).
   - Multi-tab browsing, Bookmark Bar, Bookmark Folders, Browsing History, and
     address bar autocomplete are built in.

B. Smart Media Link Detection & Right-Click Context Menu:
   - Right-click any video/audio link (`.mkv`, `.mp4`, `.avi`, `.mov`, `.webm`,
     `.mp3`, `.flac`, `.wav`, `.aac`, `.m4a`, `.ogg`, etc.) on any webpage to:
       * Play in StreamVault (progressive instant streaming via libmpv)
       * Open in External Player (PotPlayer, VLC, MPC-HC, MPC-BE, WMP, or custom)
       * Download Media (native Windows Save As dialog with exact server filename)
       * Copy Media URL
   - The webpage link currently playing is highlighted in real time so you can
     always spot which episode or movie is active.

C. Progressive Streaming vs. Permanent Downloads:
   - TEMPORARY STREAMING: Clicking "Play in StreamVault" starts playback
     immediately while buffering byte ranges to `data\cache\media\`. Seeking
     ahead fetches the requested HTTP `Range` immediately. Temporary cache files
     are automatically deleted when StreamVault closes.
   - PERMANENT DOWNLOADS: Clicking "Download Media" opens the native Windows
     Save As dialog pre-filled with the exact original server filename (including
     spaces, brackets, parentheses, and Unicode) and saves permanently to your
     chosen folder with Pause / Resume / Retry / Show in Folder support.

4. BUILT-IN PLAYER & BROWSER KEYBOARD / HARDWARE CONTROLS
---------------------------------------------------------
  Space                 Play / Pause
  Left / Right Arrow    Seek backward / forward (default 10s, configurable)
  Up / Down Arrow       Volume up / down
  M                     Mute / Unmute
  F                     Toggle Fullscreen
  Esc                   Exit Fullscreen / Close Find Bar
  [ / ]                 Decrease / Increase Playback Speed (0.25x – 4.0x)
  Z / X                 Subtitle Sync Offset -100 ms / +100 ms
  S                     Capture Video Frame Screenshot (.png in Screenshots\)
  N / P                 Next / Previous Episode in Playlist Queue
  PageDown / PageUp     Next / Previous MKV Chapter
  Ctrl + O              Open Local Video / Audio / Subtitle Files (or Drag & Drop)
  Ctrl + F              Find in Webpage (CSS Custom Highlight, non-destructive)
  Middle-Click / Ctrl+Click  Open Webpage Link or Bookmark in Background Tab
  Hardware Media Keys   Play/Pause, Next, Previous, Stop (Windows SMTC Flyout)
  Single Click Video    Play / Pause
  Double Click Video    Toggle Fullscreen
  Mouse Wheel on Video  Adjust Volume

5. WINDOWS SYSTEM REQUIREMENTS
------------------------------
- OS: 64-bit Windows 10 (1809+) or Windows 11
- VC++ Redistributable: NOT required (statically linked into `StreamVault.exe`)
- Media Codecs: NOT required (`libmpv-2.dll` is bundled in `runtime\media-engine\`)
- Browser Engine: Microsoft Edge WebView2 Runtime (pre-installed on Windows 11
  and updated Windows 10 PCs; if missing on an offline LTSC install, download
  from https://developer.microsoft.com/en-us/microsoft-edge/webview2/ or extract
  a WebView2 Fixed Version runtime into `StreamVault\runtime\webview2\`).
