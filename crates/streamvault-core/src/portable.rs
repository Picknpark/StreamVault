use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Complete set of portable directories resolved strictly relative to the
/// StreamVault application folder (never `%APPDATA%` or `%LOCALAPPDATA%`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortablePaths {
    pub root_dir: PathBuf,
    pub data_dir: PathBuf,
    pub database_dir: PathBuf,
    pub database_file: PathBuf,
    pub settings_dir: PathBuf,
    pub portable_marker_file: PathBuf,
    pub cache_dir: PathBuf,
    pub media_cache_dir: PathBuf,
    pub downloads_dir: PathBuf,
    pub logs_dir: PathBuf,
    pub log_file: PathBuf,
    pub backups_dir: PathBuf,
    pub webview2_data_dir: PathBuf,
    pub webview2_shell_dir: PathBuf,
    pub webview2_tabs_dir: PathBuf,
    pub runtime_dir: PathBuf,
    pub media_engine_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DependencyStatus {
    pub portable_root: String,
    pub portable_writable: bool,
    pub free_disk_bytes: Option<u64>,
    pub webview2_available: bool,
    pub webview2_version: Option<String>,
    pub webview2_source: String,
    pub webview2_instructions: Option<String>,
    pub libmpv_available: bool,
    pub libmpv_path: Option<String>,
    pub libmpv_api_version: Option<String>,
    pub libmpv_instructions: Option<String>,
    pub uses_appdata: bool,
}

impl PortablePaths {
    /// Resolves portable paths from an explicit directory (for testing/relocation)
    /// or automatically from the directory containing `StreamVault.exe`.
    pub fn resolve(explicit_root: Option<PathBuf>) -> Result<Self> {
        let root_dir = if let Some(root) = explicit_root {
            root
        } else if let Ok(env_root) = std::env::var("STREAMVAULT_PORTABLE_ROOT") {
            let trimmed = env_root.trim();
            if !trimmed.is_empty() {
                PathBuf::from(trimmed)
            } else {
                Self::exe_parent_dir()?
            }
        } else {
            Self::exe_parent_dir()?
        };

        let root_dir = if root_dir.is_relative() {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(root_dir)
        } else {
            root_dir
        };

        let data_dir = root_dir.join("data");
        let database_dir = data_dir.join("database");
        let database_file = database_dir.join("streamvault.db");
        let settings_dir = data_dir.join("settings");
        let portable_marker_file = settings_dir.join("portable-mode.json");
        let cache_dir = data_dir.join("cache");
        let media_cache_dir = cache_dir.join("media");
        let downloads_dir = data_dir.join("downloads");
        let logs_dir = data_dir.join("logs");
        let log_file = logs_dir.join("streamvault.log");
        let backups_dir = data_dir.join("backups");
        let webview2_data_dir = data_dir.join("webview2");
        let webview2_shell_dir = webview2_data_dir.join("shell");
        let webview2_tabs_dir = webview2_data_dir.join("tabs");
        let runtime_dir = root_dir.join("runtime");
        let media_engine_dir = runtime_dir.join("media-engine");

        let paths = Self {
            root_dir,
            data_dir,
            database_dir,
            database_file,
            settings_dir,
            portable_marker_file,
            cache_dir,
            media_cache_dir,
            downloads_dir,
            logs_dir,
            log_file,
            backups_dir,
            webview2_data_dir,
            webview2_shell_dir,
            webview2_tabs_dir,
            runtime_dir,
            media_engine_dir,
        };

        paths.ensure_directories()?;
        Ok(paths)
    }

    fn exe_parent_dir() -> Result<PathBuf> {
        let exe = std::env::current_exe().context("Failed to determine StreamVault executable path")?;
        let parent = exe
            .parent()
            .context("Executable path has no parent directory")?
            .to_path_buf();
        Ok(normalize_native_path(parent))
    }

    /// Creates all required portable directories if missing (recovers automatically
    /// even if a user deleted `data/cache` or `data/logs`).
    pub fn ensure_directories(&self) -> Result<()> {
        for dir in [
            &self.root_dir,
            &self.data_dir,
            &self.database_dir,
            &self.settings_dir,
            &self.cache_dir,
            &self.media_cache_dir,
            &self.downloads_dir,
            &self.logs_dir,
            &self.backups_dir,
            &self.webview2_data_dir,
            &self.webview2_shell_dir,
            &self.webview2_tabs_dir,
            &self.runtime_dir,
            &self.media_engine_dir,
        ] {
            fs::create_dir_all(dir)
                .with_context(|| format!("Failed to create portable directory: {}", dir.display()))?;
        }

        if !self.portable_marker_file.exists() {
            let marker = serde_json::json!({
                "application": "StreamVault",
                "portable_mode": true,
                "uses_appdata": false,
                "created_at": Utc::now().to_rfc3339(),
                "storage_layout_version": 1
            });
            let _ = fs::write(
                &self.portable_marker_file,
                serde_json::to_string_pretty(&marker).unwrap_or_default(),
            );
        }

        Ok(())
    }

    /// Resolves a stored path that may be relative to `root_dir` so moving the
    /// entire `StreamVault/` folder to another drive or PC keeps downloads & settings valid.
    /// Always normalizes path separators to native OS separators (`\` on Windows, `/` on Unix)
    /// and strips extended-length `\\?\` prefixes so Windows Shell / Explorer APIs work cleanly.
    pub fn resolve_portable_path(&self, raw: &str) -> PathBuf {
        let trimmed = raw.trim();
        let cleaned = strip_windows_verbatim_prefix(trimmed);
        let native_sep_str = if cfg!(windows) {
            cleaned.replace('/', "\\")
        } else {
            cleaned.to_string()
        };

        let p = Path::new(&native_sep_str);
        let resolved = if p.is_relative() {
            let mut joined = self.root_dir.clone();
            for part in cleaned.split(['/', '\\']) {
                if !part.is_empty() && part != "." {
                    joined.push(part);
                }
            }
            joined
        } else if p.exists() || p.parent().map(|d| d.exists()).unwrap_or(false) {
            p.components().collect::<PathBuf>()
        } else {
            // If moved from e.g. D:\Apps\StreamVault\data\downloads to E:\USB\StreamVault\data\downloads,
            // remap internal `data/...` suffix automatically!
            let normalized = cleaned.replace('\\', "/");
            if let Some(idx) = normalized.find("/data/") {
                let rel = &normalized[idx + 1..];
                let mut candidate = self.root_dir.clone();
                for part in rel.split('/') {
                    if !part.is_empty() && part != "." {
                        candidate.push(part);
                    }
                }
                if candidate.exists() || candidate.parent().map(|d| d.exists()).unwrap_or(false) {
                    return normalize_native_path(candidate);
                }
            }
            p.components().collect::<PathBuf>()
        };

        normalize_native_path(resolved)
    }

    /// Converts a path inside `root_dir` to a portable relative string (`data/downloads`)
    /// while preserving external paths (such as `D:\Movies`) as-is.
    pub fn to_portable_string(&self, target: &Path) -> String {
        if let Ok(rel) = target.strip_prefix(&self.root_dir) {
            rel.to_string_lossy().replace('\\', "/")
        } else {
            target.to_string_lossy().to_string()
        }
    }

    /// Locates `libmpv-2.dll` (or platform equivalent) inside `runtime/media-engine/` first.
    pub fn find_libmpv_library(&self) -> Option<PathBuf> {
        let candidates = [
            self.media_engine_dir.join("libmpv-2.dll"),
            self.media_engine_dir.join("mpv-2.dll"),
            self.media_engine_dir.join("libmpv.dll"),
            self.root_dir.join("libmpv-2.dll"),
            #[cfg(unix)]
            self.media_engine_dir.join("libmpv.so.2"),
            #[cfg(unix)]
            PathBuf::from("/usr/lib/x86_64-linux-gnu/libmpv.so.2"),
        ];

        for c in candidates {
            if c.is_file() {
                return Some(c);
            }
        }

        // Also check system PATH on Windows if user has mpv/libmpv installed globally
        if let Some(path_os) = std::env::var_os("PATH") {
            for dir in std::env::split_paths(&path_os) {
                let candidate = dir.join("libmpv-2.dll");
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }

        None
    }

    /// Locates bundled Fixed-Version WebView2 runtime if user placed one in `runtime/webview2/`.
    pub fn find_fixed_webview2_dir(&self) -> Option<PathBuf> {
        let fixed = self.runtime_dir.join("webview2");
        if fixed.join("msedgewebview2.exe").is_file() {
            Some(fixed)
        } else {
            None
        }
    }

    pub fn check_writable(&self) -> bool {
        let probe = self.data_dir.join(".sv_write_probe");
        match fs::write(&probe, b"ok") {
            Ok(_) => {
                let _ = fs::remove_file(&probe);
                true
            }
            Err(_) => false,
        }
    }

    pub fn query_free_disk_bytes(&self) -> Option<u64> {
        query_available_space(&self.root_dir)
    }

    /// Runs a comprehensive runtime & portability check.
    pub fn inspect_dependencies(&self) -> DependencyStatus {
        let portable_writable = self.check_writable();
        let free_disk_bytes = self.query_free_disk_bytes();

        let (webview2_available, webview2_version, webview2_source) = detect_webview2_runtime(self);
        let webview2_instructions = if !webview2_available {
            Some(
                "Microsoft Edge WebView2 Runtime was not detected on this Windows PC. \
                Install the Microsoft Edge WebView2 Evergreen Runtime from \
                https://developer.microsoft.com/en-us/microsoft-edge/webview2/ \
                or place a Fixed Version runtime inside StreamVault\\runtime\\webview2\\."
                    .to_string(),
            )
        } else {
            None
        };

        let libmpv_found = self.find_libmpv_library();
        let (libmpv_available, libmpv_api_version) = match &libmpv_found {
            Some(path) => match crate::mpv_engine::probe_libmpv_version(path) {
                Ok(ver) => (true, Some(ver)),
                Err(_) => (true, Some("PE64 DLL present".to_string())),
            },
            None => (false, None),
        };

        let libmpv_instructions = if !libmpv_available {
            Some(
                "Place 64-bit libmpv-2.dll inside StreamVault\\runtime\\media-engine\\libmpv-2.dll \
                to enable full MKV/HEVC/DTS/ASS built-in hardware-accelerated playback."
                    .to_string(),
            )
        } else {
            None
        };

        DependencyStatus {
            portable_root: self.root_dir.to_string_lossy().to_string(),
            portable_writable,
            free_disk_bytes,
            webview2_available,
            webview2_version,
            webview2_source,
            webview2_instructions,
            libmpv_available,
            libmpv_path: libmpv_found.map(|p| p.to_string_lossy().to_string()),
            libmpv_api_version,
            libmpv_instructions,
            uses_appdata: false,
        }
    }
}

#[cfg(windows)]
fn detect_webview2_runtime(paths: &PortablePaths) -> (bool, Option<String>, String) {
    if let Some(fixed) = paths.find_fixed_webview2_dir() {
        return (
            true,
            Some("Fixed Runtime".to_string()),
            format!("Portable ({})", fixed.display()),
        );
    }

    use winreg::enums::*;
    use winreg::RegKey;

    const WEBVIEW2_CLIENT_GUID: &str = "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";
    let reg_targets = [
        (
            HKEY_LOCAL_MACHINE,
            format!("SOFTWARE\\WOW6432Node\\Microsoft\\EdgeUpdate\\Clients\\{WEBVIEW2_CLIENT_GUID}"),
            "System Evergreen (WOW6432Node)",
        ),
        (
            HKEY_LOCAL_MACHINE,
            format!("SOFTWARE\\Microsoft\\EdgeUpdate\\Clients\\{WEBVIEW2_CLIENT_GUID}"),
            "System Evergreen (HKLM)",
        ),
        (
            HKEY_CURRENT_USER,
            format!("Software\\Microsoft\\EdgeUpdate\\Clients\\{WEBVIEW2_CLIENT_GUID}"),
            "User Evergreen (HKCU)",
        ),
    ];

    for (hive, subkey, label) in reg_targets {
        if let Ok(key) = RegKey::predef(hive).open_subkey(&subkey) {
            if let Ok(pv) = key.get_value::<String, _>("pv") {
                let pv_trim = pv.trim();
                if !pv_trim.is_empty() && pv_trim != "0.0.0.0" {
                    return (true, Some(pv_trim.to_string()), label.to_string());
                }
            }
        }
    }

    // Fallback: check standard Program Files (x86)\Microsoft\EdgeWebView\Application
    for env_key in ["ProgramFiles(x86)", "ProgramFiles"] {
        if let Ok(base) = std::env::var(env_key) {
            let app_dir = PathBuf::from(base)
                .join("Microsoft")
                .join("EdgeWebView")
                .join("Application");
            if let Ok(entries) = fs::read_dir(&app_dir) {
                for entry in entries.flatten() {
                    let candidate = entry.path().join("msedgewebview2.exe");
                    if candidate.is_file() {
                        let ver = entry.file_name().to_string_lossy().to_string();
                        return (true, Some(ver), "EdgeWebView Application Folder".to_string());
                    }
                }
            }
        }
    }

    (false, None, "Missing".to_string())
}

#[cfg(not(windows))]
fn detect_webview2_runtime(paths: &PortablePaths) -> (bool, Option<String>, String) {
    if let Some(fixed) = paths.find_fixed_webview2_dir() {
        return (
            true,
            Some("Fixed Runtime".to_string()),
            format!("Portable ({})", fixed.display()),
        );
    }
    (
        true,
        Some("Host WebView Runtime".to_string()),
        "Host Environment".to_string(),
    )
}

#[cfg(windows)]
fn query_available_space(path: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    wide.push(0);
    let mut free_available: u64 = 0;
    let mut total_bytes: u64 = 0;
    let mut total_free: u64 = 0;
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut free_available as *mut u64,
            &mut total_bytes as *mut u64,
            &mut total_free as *mut u64,
        )
    };
    if ok != 0 {
        Some(free_available)
    } else {
        None
    }
}

#[cfg(not(windows))]
fn query_available_space(path: &Path) -> Option<u64> {
    let c_path = std::ffi::CString::new(path.to_string_lossy().as_bytes()).ok()?;
    unsafe {
        let mut stat: libc_statvfs = std::mem::zeroed();
        if statvfs(c_path.as_ptr(), &mut stat) == 0 {
            Some(stat.f_bavail * stat.f_frsize)
        } else {
            None
        }
    }
}

#[cfg(not(windows))]
#[repr(C)]
struct libc_statvfs {
    f_bsize: u64,
    f_frsize: u64,
    f_blocks: u64,
    f_bfree: u64,
    f_bavail: u64,
    f_files: u64,
    f_ffree: u64,
    f_favail: u64,
    f_fsid: u64,
    f_flag: u64,
    f_namemax: u64,
    __f_spare: [i32; 6],
}

#[cfg(not(windows))]
extern "C" {
    fn statvfs(path: *const std::os::raw::c_char, buf: *mut libc_statvfs) -> std::os::raw::c_int;
}

/// Shows a native Win32 warning dialog if WebView2 Runtime is missing on a target Windows PC,
/// giving clear portable & installer instructions instead of failing silently.
#[cfg(windows)]
pub fn show_missing_webview2_alert(portable_root: &Path) {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};

    let msg = format!(
        "StreamVault requires the Microsoft Edge WebView2 Runtime, which was not found on this PC.\n\n\
        To run StreamVault:\n\
        1. Install Microsoft Edge WebView2 Evergreen Runtime (included by default on Windows 11 and updated Windows 10):\n\
           https://go.microsoft.com/fwlink/p/?LinkId=2124703\n\n\
        OR for 100% offline portability:\n\
        2. Extract the WebView2 Fixed Version Runtime (x64) into:\n\
           {}\\runtime\\webview2\\\n\n\
        See README.txt in the StreamVault folder for complete instructions.",
        portable_root.display()
    );
    let title = "StreamVault — WebView2 Runtime Required";
    let wide_msg: Vec<u16> = std::ffi::OsStr::new(&msg)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let wide_title: Vec<u16> = std::ffi::OsStr::new(title)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            wide_msg.as_ptr(),
            wide_title.as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}

pub fn strip_windows_verbatim_prefix(raw: &str) -> String {
    if let Some(rest) = raw.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = raw.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        raw.to_string()
    }
}

pub fn normalize_native_path(path: PathBuf) -> PathBuf {
    let raw = path.to_string_lossy();
    let stripped = strip_windows_verbatim_prefix(&raw);
    if cfg!(windows) {
        let win_str = stripped.replace('/', "\\");
        Path::new(&win_str).components().collect::<PathBuf>()
    } else {
        Path::new(&stripped).components().collect::<PathBuf>()
    }
}

