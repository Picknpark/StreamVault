# Third-Party Open-Source Notices

StreamVault's first-party source code (`crates/`, `src-tauri/`, `ui/`, `runtime-src/`, `packaging/`, `scripts/`) is licensed under the **MIT License** (see [`LICENSE`](./LICENSE)).

Portable Windows release archives (`StreamVault-v*-win64-portable.zip`) additionally bundle or link against the open-source software components listed below. We gratefully acknowledge their authors and maintainers.

---

## 1. Dynamically Loaded Media Engine (`runtime/media-engine/libmpv-2.dll`)

- **Project**: [mpv](https://mpv.io/) (`libmpv` client shared library)
- **Upstream Source**: <https://github.com/mpv-player/mpv>
- **Windows LGPL Build Source**: <https://github.com/zhongfly/mpv-winbuild> (`mpv-dev-lgpl-x86_64`)
- **License**: **GNU Lesser General Public License v2.1 or later (`LGPL-2.1-or-later`)**
  - Full license text: <https://www.gnu.org/licenses/old-licenses/lgpl-2.1.html>

### LGPL v2.1+ Dynamic Linking & User Replacement Notice
1. `StreamVault.exe` does **not** statically link `libmpv` or `FFmpeg`. Instead, `crates/streamvault-core/src/mpv_engine.rs` resolves `runtime/media-engine/libmpv-2.dll` dynamically at runtime via the public `mpv` C ABI (`mpv/client.h`).
2. Users are completely free to inspect, modify, upgrade, or replace `runtime/media-engine/libmpv-2.dll` in any portable StreamVault folder with any ABI-compatible 64-bit Windows `libmpv-2.dll` build without recompiling `StreamVault.exe`.
3. Complete source code and build recipes for the LGPL-configured `libmpv-2.dll` (including LGPL builds of FFmpeg, `libass`, `dav1d`, `libplacebo`, `freetype2`, `fribidi`, and `harfbuzz`) are available at <https://github.com/mpv-player/mpv> and <https://github.com/zhongfly/mpv-winbuild>.

> **Maintainer Note**: Official StreamVault portable releases **must** bundle the **`lgpl`** variant (`mpv-dev-lgpl-x86_64-*.7z`), **not** the GPLv3 variant, whenever StreamVault itself is distributed under a permissive license (such as MIT or Apache-2.0).

---

## 2. Embedded Database Engine (SQLite 3)

- **Project**: [SQLite](https://www.sqlite.org/) (via [`rusqlite`](https://github.com/rusqlite/rusqlite))
- **SQLite C Library License**: **Public Domain** (<https://www.sqlite.org/copyright.html>)
- **`rusqlite` Rust Bindings License**: **MIT License**

---

## 3. Desktop Shell & Browser Runtime

- **Tauri (`tauri`, `tauri-runtime`, `wry`, `tao`, `muda`)**
  - Upstream: <https://github.com/tauri-apps/tauri>
  - License: **Apache-2.0 OR MIT**
- **Microsoft Edge WebView2 Loader (`webview2-com` / `WebView2LoaderStatic.lib`)**
  - Upstream: <https://github.com/wravery/webview2-rs> & Microsoft WebView2 SDK
  - License: **MIT** (Rust bindings) & **Microsoft WebView2 SDK BSD-Style Redistributable License**
  - Note: StreamVault uses the Microsoft Edge WebView2 Evergreen Runtime pre-installed on Windows 10/11 (or an optional user-supplied Fixed Version runtime in `runtime/webview2/`).

---

## 4. Core Rust Ecosystem Crates

All Rust crates compiled into `StreamVault.exe` are permissively licensed (or file-level MPL-2.0 for root CA data):

| Crate(s) | License | Purpose |
|---|---|---|
| `tokio`, `tokio-util` | MIT | Async runtime, TCP loopback Range proxy, file/stream I/O |
| `reqwest`, `hyper`, `http` | MIT OR Apache-2.0 | HTTP/1.1 & HTTP/2 client, `Range` streaming, resumable downloads |
| `rustls` | Apache-2.0 OR ISC OR MIT | Pure-Rust TLS implementation |
| `ring` | ISC / BoringSSL-style | Cryptographic primitives for TLS |
| `webpki-roots` | MPL-2.0 | Mozilla Root CA certificate bundle fallback |
| `serde`, `serde_json` | MIT OR Apache-2.0 | IPC, settings, and structured log serialization |
| `libloading` | ISC | Runtime DLL symbol loader for `libmpv-2.dll` |
| `windows-sys`, `winreg` | MIT OR Apache-2.0 | Win32 APIs (`HWND`, D3D11 host, Explorer shell, Clipboard, IP Helper) |
| `rfd` | MIT | Native Windows `IFileOpenDialog` / `IFileSaveDialog` dialogs |
| `parking_lot`, `bytes`, `url`, `uuid`, `chrono`, `sha2`, `anyhow`, `thiserror` | MIT OR Apache-2.0 | Concurrency, URL preservation, hashing, and error handling |

To generate a machine-readable bill of materials (SBOM) or full license report at any time, run [cargo-about](https://github.com/EmbarkStudios/cargo-about) or [cargo-deny](https://github.com/EmbarkStudios/cargo-deny) from the repository root:

```bash
cargo install cargo-deny
cargo deny list
```
