# StreamVault — Build & Packaging Instructions

StreamVault is written in **Rust (Edition 2021)** with **Tauri 2** and a zero-build-step **plain modular HTML + CSS + Vanilla JavaScript** frontend (`ui/`). No Node.js bundler, TypeScript compiler, or frontend framework is required.

---

## 1. Building Natively on 64-bit Windows 10 / Windows 11 (Recommended)

### Prerequisites
1. **Rust Toolchain (stable `x86_64-pc-windows-msvc`)**:
   Install from <https://rustup.rs/>:
   ```powershell
   rustup default stable-x86_64-pc-windows-msvc
   ```
2. **Visual Studio 2022 Build Tools** (Desktop development with C++ — Windows 10/11 SDK + MSVC v143 x64 tools).
3. **64-bit `libmpv-2.dll`**:
   - Already included in the portable folder at `StreamVault\runtime\media-engine\libmpv-2.dll`, or download the latest `mpv-dev-lgpl-x86_64-*.7z` release from <https://github.com/zhongfly/mpv-winbuild/releases>.

### Compile & Run Tests
```powershell
cd streamvault-project

# 1. Run the core engine integration test suite (SQLite migrations, Dhaka-Flix MKV URLs,
#    HTTP Range streaming & seeking, non-Range fallback, resumable downloads, portability):
cargo test -p streamvault-core

# 2. Build the optimized 64-bit Windows GUI executable (embeds ../ui assets automatically
#    via the default "custom-protocol" feature and statically links the MSVC CRT):
cargo build --release --target x86_64-pc-windows-msvc -p streamvault
```
The compiled GUI binary is output to:
```text
target\x86_64-pc-windows-msvc\release\StreamVault.exe
```

### Assemble Portable Folder on Windows
```powershell
New-Item -ItemType Directory -Force -Path `
  ..\StreamVault\data\database, `
  ..\StreamVault\data\settings, `
  ..\StreamVault\data\cache\media, `
  ..\StreamVault\data\downloads, `
  ..\StreamVault\data\logs, `
  ..\StreamVault\data\backups, `
  ..\StreamVault\data\webview2, `
  ..\StreamVault\runtime\media-engine | Out-Null

Copy-Item target\x86_64-pc-windows-msvc\release\StreamVault.exe ..\StreamVault\StreamVault.exe -Force
Copy-Item path\to\libmpv-2.dll ..\StreamVault\runtime\media-engine\libmpv-2.dll -Force

# Optional: pre-initialize streamvault.db before first launch (StreamVault.exe also does this automatically on first boot)
cargo run -p streamvault-core --example init_portable_db -- ..\StreamVault
```

---

## 2. Cross-Compiling for Windows x64 from Linux (`cargo-xwin`)

StreamVault supports deterministic cross-compilation from Linux (`x86_64-unknown-linux-gnu`) to 64-bit Windows (`x86_64-pc-windows-msvc`) using `clang`, `lld-link`, and `cargo-xwin`.

### Prerequisites (Debian / Ubuntu)
```bash
sudo apt-get update
sudo apt-get install -y clang lld llvm pkg-config p7zip-full curl build-essential

curl https://sh.rustup.rs -sSf | sh -s -- -y --default-toolchain stable
source "$HOME/.cargo/env"
rustup target add x86_64-pc-windows-msvc
cargo install cargo-xwin
```

### Run Integration Tests & Cross-Compile `StreamVault.exe`
```bash
cd streamvault-project

# 1. Run all integration tests natively:
cargo test -p streamvault-core

# 2. Cross-compile optimized Windows x64 GUI binary:
cargo xwin build --release --target x86_64-pc-windows-msvc

# 3. Verify PE32+ GUI subsystem and zero non-OS DLL dependencies:
llvm-objdump -p target/x86_64-pc-windows-msvc/release/StreamVault.exe | grep -E "Subsystem|DLL Name:"

# 4. (Optional) Build the 2 KB zero-CRT Vulkan fallback stub for GPU-less Windows VMs:
clang --target=x86_64-pc-windows-msvc -O2 -nostdlib -Wl,/DLL -Wl,/NOENTRY \
  runtime-src/vulkan_fallback_stub.c -fuse-ld=lld-link \
  -o ../StreamVault/runtime/media-engine/vulkan-1.dll
```

---

## 3. Critical Build Notes

- **`custom-protocol` Feature**: `src-tauri/Cargo.toml` sets `default = ["custom-protocol"]` (`tauri/custom-protocol`). This instructs `tauri::generate_context!()` to embed all HTML, CSS, and Vanilla JS modules from `../ui` directly into `StreamVault.exe` even when building with `cargo build` / `cargo xwin build` without `tauri-cli`.
- **Static MSVC Runtime (`libcmt.lib` / `libvcruntime.lib`)**: `tauri-build` enables `static_vcruntime` on `x86_64-pc-windows-msvc`, so `StreamVault.exe` runs on clean Windows 10/11 installations without requiring the Visual C++ Redistributable installer.
- **Static `WebView2LoaderStatic.lib`**: `webview2-com-sys` statically links `WebView2LoaderStatic.lib` on MSVC targets, eliminating any loose `WebView2Loader.dll` requirement.
- **Bundled SQLite (`rusqlite` `bundled` + `backup`)**: SQLite 3 is compiled directly into `StreamVault.exe` with WAL and online backup APIs enabled.
