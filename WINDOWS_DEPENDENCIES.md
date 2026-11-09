# StreamVault — Windows Runtime Dependencies & Portability Reference

StreamVault is engineered for **strict folder portability** on 64-bit Windows 10 (version 1809+) and Windows 11.

---

## 1. Binary Dependency Audit (`StreamVault.exe`)

Inspecting `StreamVault.exe` with `llvm-objdump -p` confirms:
- **Format**: `PE32+ executable (GUI) x86-64` (`Subsystem: Windows GUI`)
- **MSVC C/C++ Runtime (`VCRUNTIME140.dll` / `MSVCP140.dll`)**: **NOT REQUIRED** — statically linked via `libcmt.lib` and `libvcruntime.lib`.
- **SQLite (`sqlite3.dll`)**: **NOT REQUIRED** — compiled statically via `rusqlite` (`bundled` + `backup`).
- **TLS / Crypto (`openssl.dll`)**: **NOT REQUIRED** — uses pure-Rust/C `rustls` + `ring` + Windows native root store (`rustls-native-certs`).
- **WebView2 Loader (`WebView2Loader.dll`)**: **NOT REQUIRED** — statically linked via `WebView2LoaderStatic.lib`.
- **Imported OS DLLs**: Exclusively built-in Windows 10/11 `System32` libraries (`kernel32.dll`, `user32.dll`, `gdi32.dll`, `ole32.dll`, `oleaut32.dll`, `combase.dll`, `shell32.dll`, `shlwapi.dll`, `comctl32.dll`, `dwmapi.dll`, `imm32.dll`, `advapi32.dll`, `ws2_32.dll`, `bcrypt.dll`, `bcryptprimitives.dll`, `crypt32.dll`, `ntdll.dll`, and built-in Windows 10/11 Universal CRT `api-ms-win-crt-*.dll`).

---

## 2. Browser Engine: Microsoft Edge WebView2 Runtime

StreamVault uses Windows WebView2 to render both its application shell and real multi-tab Chromium webpages (`https://youtube.com`, `https://facebook.com`, LAN HTTP index servers, etc.).

### Detection Order (`crates/streamvault-core/src/portable.rs` & `src-tauri/src/main.rs`)
1. **Bundled Fixed Version Runtime (Optional Offline Mode)**:
   If `StreamVault\runtime\webview2\` contains `msedgewebview2.exe`, StreamVault sets `WEBVIEW2_BROWSER_EXECUTABLE_FOLDER` to that directory before starting Tauri. This allows 100% air-gapped operation even on stripped Windows 10 IoT/LTSC installations.
2. **System WebView2 Evergreen Runtime (Default on Windows 10/11)**:
   Pre-installed by default on **all Windows 11 systems** and **updated Windows 10 systems**. StreamVault checks the standard EdgeUpdate registry keys (`HKLM\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}` and `HKCU\Software\Microsoft\EdgeUpdate\Clients\...`).
3. **Graceful Missing-Runtime Dialog**:
   If neither is present, `StreamVault.exe` displays a native Win32 dialog (`MessageBoxW`) with exact instructions to install the Microsoft Edge WebView2 Evergreen Runtime (<https://developer.microsoft.com/en-us/microsoft-edge/webview2/>) or place a Fixed Version runtime in `StreamVault\runtime\webview2\`, rather than crashing silently.

### Self-Contained WebView2 Profile (`No AppData Pollution`)
Before initializing WebView2, `main.rs` sets `WEBVIEW2_USER_DATA_FOLDER=<exe_dir>\data\webview2` **and** passes `.data_directory(<exe_dir>\data\webview2)` to every `WebviewBuilder`. Cookies, local storage, HTTP cache, and browser state never spill into `%LOCALAPPDATA%`.

---

## 3. Built-in Media Engine: `libmpv-2.dll`

- **Bundled Location**: `StreamVault\runtime\media-engine\libmpv-2.dll` (64-bit LGPL v2.1+ build with FFmpeg, `libass` styled subtitle renderer, `dav1d` AV1 decoder, HEVC/H.264/VP9, Matroska/MKV, FLAC/DTS/AAC/Opus/MP3).
- **Dynamic Loading (`mpv_engine.rs`)**: Loaded dynamically via `libloading` against the stable `mpv` C API (`mpv_create`, `mpv_set_option("wid", ...)`, `mpv_initialize`, `mpv_command`, `mpv_observe_property`, `mpv_wait_event`).
- **Video Output (`gpu-context=d3d11`)**: Bound to a dedicated Win32 child `HWND` (`StreamVaultMpvSurface`) using Direct3D 11 (`d3d11`) + `hwdec=auto-safe`, which works across NVIDIA, AMD, Intel, and Windows WARP software rasterizers.
- **GPU-less VM Compatibility (`vulkan-1.dll` Fallback Stub)**:
  Upstream `libmpv-2.dll` imports 10 Vulkan loader symbols in its PE import table. On real GPU PCs, `C:\Windows\System32\vulkan-1.dll` is loaded first by Windows' `SetDllDirectoryW` search order. On clean GPU-less Hyper-V / VirtualBox / Windows Sandbox VMs where `System32\vulkan-1.dll` is absent, Windows falls through to `StreamVault\runtime\media-engine\vulkan-1.dll` (a 2 KB stub returning `VK_ERROR_INITIALIZATION_FAILED`), allowing `libmpv-2.dll` to load cleanly and render via Direct3D 11 (`d3d11`).

---

## 4. External Media Player Discovery (`external_player.rs`)

StreamVault never performs slow full-disk scans. Instead, it discovers installed players deterministically via:
1. `HKLM` / `HKCU` `SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\<exe>` (`PotPlayerMini64.exe`, `PotPlayerMini.exe`, `vlc.exe`, `mpc-hc64.exe`, `mpc-be64.exe`, `kmplayer.exe`, `mpv.exe`, `wmplayer.exe`).
2. `HKLM\SOFTWARE\Clients\Media\*\shell\open\command` (plus `WOW6432Node`).
3. Standard `%ProgramFiles%`, `%ProgramFiles(x86)%`, and `%LOCALAPPDATA%\Programs` installation paths.
4. `PATH` environment lookup.
5. User-configured custom `.exe` players stored in `custom_external_players` inside `streamvault.db`.

---

## 5. Verification Tool

Users can verify all dependencies at any time either:
- Before launching, by double-clicking `StreamVault\Check-Dependencies.bat` (runs `runtime\check-windows-dependencies.ps1`), or
- Inside the app under **Settings → Diagnostics**, which displays live portable write status, free disk space, WebView2 runtime version, `libmpv` C API version, and structured logs.
