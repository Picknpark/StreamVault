# Contributing to StreamVault

Thank you for your interest in contributing to **StreamVault**! Whether you are fixing a bug, improving Windows D3D11/`libmpv` playback, enhancing LAN/HTTP media detection, or polishing documentation, your contributions are welcome.

---

## 1. Development Environment Setup

StreamVault is intentionally designed to be simple to build and hack on:
- **Backend & Shell**: Rust (Edition 2021, stable toolchain) + Tauri 2 (`crates/streamvault-core` and `src-tauri`).
- **Frontend (`ui/`)**: Plain ES Modules Vanilla JavaScript, HTML5, and CSS3—**no Node.js bundler, no Webpack/Vite step, and no framework lock-in**.

### Windows 10 / 11 (Recommended for Full GUI + `libmpv` Testing)
1. Install [Rust](https://rustup.rs/) (`stable-x86_64-pc-windows-msvc`).
2. Install **Visual Studio 2022 Build Tools** (*Desktop development with C++*).
3. Download an **LGPL** 64-bit `libmpv-2.dll` build (`mpv-dev-lgpl-x86_64-*.7z`) from [`zhongfly/mpv-winbuild`](https://github.com/zhongfly/mpv-winbuild/releases).

### Linux (Core Engine Development & Cross-Compilation)
You can run the entire `streamvault-core` integration test suite and cross-compile `StreamVault.exe` on Debian/Ubuntu using `clang`, `lld`, and `cargo-xwin`. See [`BUILD_INSTRUCTIONS.md`](./BUILD_INSTRUCTIONS.md).

---

## 2. Architectural Guardrails (Please Read Before Modifying)

When working on StreamVault, please preserve these core invariants:

1. **100% Portable Storage (`No %APPDATA%`)**:
   - Every persistent file (SQLite DB, single backup, temporary stream cache, WebView2 user data folders, logs) **must** resolve relative to `PortablePaths` (`<exe_dir>/data/...`). Never write to `%APPDATA%` or `%LOCALAPPDATA%`.
2. **Temporary Stream Cache vs. Permanent Downloads Isolation**:
   - Temporary playback buffers live exclusively in `data/cache/media/` (`stream_cache.rs`) and are auto-cleaned.
   - Permanent user downloads (`downloader.rs`) must never be touched by cache cleanup routines.
3. **WebView2 & Win32 Threading / Z-Order Rules**:
   - Never call `window.add_child(...)` synchronously inside a blocking IPC handler on Windows (causes WebView2 UI message pump deadlock); spawn tab webviews asynchronously on the main window loop.
   - Keep `data/webview2/shell` and `data/webview2/tabs` strictly separated so profile locks never collide.
   - Remember that HTML/CSS `z-index` inside WebView2 cannot paint above a sibling native Win32 child `HWND` (`StreamVaultMpvSurface`). Any overlay extending over the video canvas must use an owned Win32 popup (`WS_POPUP`) or hide the surface while modal dialogs are open.
4. **Tauri v2 Capability Boundary (`src-tauri/capabilities/`)**:
   - Never grant privileged `allow-*` commands to `tab-*` webviews in `tab-media-context.json`. Any new UI command must be added to `src-tauri/build.rs` and `src-tauri/capabilities/shell-local.json` (`main` webview only).
5. **LGPL Compliance for `libmpv-2.dll`**:
   - Always dynamically load `libmpv-2.dll` via `libloading` and only bundle `LGPL-2.1+` builds (`mpv-dev-lgpl-*`) in official releases.

---

## 3. Running Verification & Tests Before Opening a PR

Please ensure all checks pass locally:

```bash
# 1. Check Rust formatting
cargo fmt --all -- --check

# 2. Run the automated core integration test suite
cargo test -p streamvault-core

# 3. Validate syntax of all Vanilla JS ES modules (if Node.js is installed)
node --check ui/js/*.js
```

---

## 4. Submitting a Pull Request

1. Fork the repository and create a focused branch from `main` (e.g., `fix/mkv-subtitle-selection` or `feat/playlist-queue`).
2. Keep commits atomic and descriptive (e.g., `fix(player): prevent HUD resize spam during fullscreen mousemove`).
3. Never commit `.exe`, `.dll`, `.db`, `.svcache`, or `data/webview2/` runtime artifacts.
4. Open a Pull Request against `main` and fill out the PR checklist.

By submitting a contribution to this repository, you agree that your contribution will be licensed under the same [LICENSE](./LICENSE) as the project.
