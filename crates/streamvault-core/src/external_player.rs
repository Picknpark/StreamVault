use crate::db::Database;
use crate::logging::StructuredLogger;
use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;
#[cfg(windows)]
use std::path::PathBuf;
use std::process::{Command, Stdio};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalPlayerInfo {
    pub id: String,
    pub name: String,
    pub executable_path: String,
    pub source: String, // "registry" | "standard_path" | "path_env" | "custom"
    pub custom_id: Option<i64>,
    pub available: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerTestResult {
    pub success: bool,
    pub executable_path: String,
    pub message: String,
}

#[cfg(windows)]
struct KnownPlayerSpec {
    slug: &'static str,
    display_name: &'static str,
    exe_names: &'static [&'static str],
    relative_dirs: &'static [&'static str],
}

#[cfg(windows)]
const KNOWN_WINDOWS_PLAYERS: &[KnownPlayerSpec] = &[
    KnownPlayerSpec {
        slug: "potplayer",
        display_name: "PotPlayer",
        exe_names: &[
            "PotPlayerMini64.exe",
            "PotPlayer64.exe",
            "PotPlayerMini.exe",
            "PotPlayer.exe",
        ],
        relative_dirs: &["DAUM\\PotPlayer", "PotPlayer", "DAUM\\PotPlayer64"],
    },
    KnownPlayerSpec {
        slug: "vlc",
        display_name: "VLC Media Player",
        exe_names: &["vlc.exe"],
        relative_dirs: &["VideoLAN\\VLC"],
    },
    KnownPlayerSpec {
        slug: "mpc-hc",
        display_name: "MPC-HC",
        exe_names: &["mpc-hc64.exe", "mpc-hc.exe"],
        relative_dirs: &["MPC-HC", "K-Lite Codec Pack\\MPC-HC64", "K-Lite Codec Pack\\MPC-HC"],
    },
    KnownPlayerSpec {
        slug: "mpc-be",
        display_name: "MPC-BE",
        exe_names: &["mpc-be64.exe", "mpc-be.exe"],
        relative_dirs: &["MPC-BE x64", "MPC-BE"],
    },
    KnownPlayerSpec {
        slug: "wmp",
        display_name: "Windows Media Player",
        exe_names: &["wmplayer.exe"],
        relative_dirs: &["Windows Media Player"],
    },
    KnownPlayerSpec {
        slug: "mpv",
        display_name: "mpv Media Player",
        exe_names: &["mpv.exe"],
        relative_dirs: &["mpv"],
    },
    KnownPlayerSpec {
        slug: "kmplayer",
        display_name: "KMPlayer",
        exe_names: &["KMPlayer64.exe", "KMPlayer.exe"],
        relative_dirs: &["KMPlayer 64X", "KMPlayer"],
    },
    KnownPlayerSpec {
        slug: "gom",
        display_name: "GOM Player",
        exe_names: &["GOM64.exe", "GOM.exe"],
        relative_dirs: &["GRETECH\\GOMPlayer64", "GRETECH\\GOMPlayer"],
    },
    KnownPlayerSpec {
        slug: "smplayer",
        display_name: "SMPlayer",
        exe_names: &["smplayer.exe"],
        relative_dirs: &["SMPlayer"],
    },
];

/// Detects installed external media players without scanning the entire hard drive,
/// combining Windows Registry registration, known installation locations, `PATH`, and user custom players.
pub fn detect_external_players(db: &Database) -> Result<Vec<ExternalPlayerInfo>> {
    let mut results = Vec::new();
    let mut seen_paths: HashSet<String> = HashSet::new();

    #[cfg(windows)]
    {
        discover_windows_players(&mut results, &mut seen_paths);
    }

    #[cfg(not(windows))]
    {
        discover_host_players(&mut results, &mut seen_paths);
    }

    // Merge user-configured custom players from SQLite
    let custom_players = list_custom_players(db)?;
    for (cid, cname, cpath) in custom_players {
        let norm = normalize_exe_key(&cpath);
        let exists = Path::new(&cpath).is_file();
        if seen_paths.insert(norm) {
            results.push(ExternalPlayerInfo {
                id: format!("custom-{cid}"),
                name: cname,
                executable_path: cpath,
                source: "custom".to_string(),
                custom_id: Some(cid),
                available: exists,
            });
        }
    }

    Ok(results)
}

fn normalize_exe_key(p: &str) -> String {
    p.trim().replace('/', "\\").to_ascii_lowercase()
}

#[cfg(windows)]
fn discover_windows_players(out: &mut Vec<ExternalPlayerInfo>, seen: &mut HashSet<String>) {
    use winreg::enums::*;
    use winreg::RegKey;

    let mut add_candidate = |slug: &str, name: &str, path: PathBuf, source: &str| {
        if path.is_file() {
            let full = path.to_string_lossy().to_string();
            let key = normalize_exe_key(&full);
            if seen.insert(key) {
                out.push(ExternalPlayerInfo {
                    id: format!("{slug}-{}", out.len() + 1),
                    name: name.to_string(),
                    executable_path: full,
                    source: source.to_string(),
                    custom_id: None,
                    available: true,
                });
            }
        }
    };

    // 1. Windows Registry `App Paths` (HKLM, HKLM\WOW6432Node, HKCU)
    for spec in KNOWN_WINDOWS_PLAYERS {
        for exe in spec.exe_names {
            for (hive, prefix) in [
                (
                    HKEY_LOCAL_MACHINE,
                    "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\App Paths",
                ),
                (
                    HKEY_LOCAL_MACHINE,
                    "SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\App Paths",
                ),
                (
                    HKEY_CURRENT_USER,
                    "Software\\Microsoft\\Windows\\CurrentVersion\\App Paths",
                ),
            ] {
                let sub = format!("{prefix}\\{exe}");
                if let Ok(key) = RegKey::predef(hive).open_subkey(&sub) {
                    if let Ok(default_val) = key.get_value::<String, _>("") {
                        let cleaned = clean_registry_exe_path(&default_val);
                        add_candidate(spec.slug, spec.display_name, PathBuf::from(cleaned), "registry");
                    }
                }
            }
        }
    }

    // 2. Vendor-specific Windows Registry keys (PotPlayer, VLC, MPC-HC, MPC-BE)
    let vendor_keys = [
        (HKEY_CURRENT_USER, "Software\\DAUM\\PotPlayer64", "ProgramPath", "potplayer", "PotPlayer (64-bit)"),
        (HKEY_CURRENT_USER, "Software\\DAUM\\PotPlayerMini64", "ProgramPath", "potplayer", "PotPlayer Mini (64-bit)"),
        (HKEY_LOCAL_MACHINE, "SOFTWARE\\DAUM\\PotPlayer64", "ProgramPath", "potplayer", "PotPlayer (64-bit)"),
        (HKEY_LOCAL_MACHINE, "SOFTWARE\\VideoLAN\\VLC", "", "vlc", "VLC Media Player"),
        (HKEY_LOCAL_MACHINE, "SOFTWARE\\WOW6432Node\\VideoLAN\\VLC", "", "vlc", "VLC Media Player (32-bit)"),
        (HKEY_LOCAL_MACHINE, "SOFTWARE\\MPC-HC\\MPC-HC", "ExePath", "mpc-hc", "MPC-HC"),
        (HKEY_CURRENT_USER, "Software\\MPC-HC\\MPC-HC", "ExePath", "mpc-hc", "MPC-HC"),
        (HKEY_LOCAL_MACHINE, "SOFTWARE\\MPC-BE", "ExePath", "mpc-be", "MPC-BE"),
    ];

    for (hive, subkey, val_name, slug, label) in vendor_keys {
        if let Ok(k) = RegKey::predef(hive).open_subkey(subkey) {
            if let Ok(raw) = k.get_value::<String, _>(val_name) {
                let cleaned = clean_registry_exe_path(&raw);
                add_candidate(slug, label, PathBuf::from(cleaned), "registry");
            }
        }
    }

    // 3. Registered Windows Media Clients (`SOFTWARE\Clients\Media\*\shell\open\command`)
    for hive in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
        if let Ok(media_clients) = RegKey::predef(hive).open_subkey("SOFTWARE\\Clients\\Media") {
            for client_name in media_clients.enum_keys().flatten() {
                let cmd_sub = format!("{client_name}\\shell\\open\\command");
                if let Ok(cmd_key) = media_clients.open_subkey(&cmd_sub) {
                    if let Ok(cmd_str) = cmd_key.get_value::<String, _>("") {
                        let exe_str = clean_registry_exe_path(&cmd_str);
                        let p = PathBuf::from(&exe_str);
                        if p.is_file() {
                            let label = media_clients
                                .open_subkey(&client_name)
                                .and_then(|ck| ck.get_value::<String, _>(""))
                                .unwrap_or_else(|_| client_name.clone());
                            add_candidate("media-client", &label, p, "registry");
                        }
                    }
                }
            }
        }
    }

    // 4. Standard installation directories (ProgramFiles, ProgramFiles(x86), LocalAppData\Programs, Scoop, Chocolatey)
    let mut base_roots: Vec<PathBuf> = Vec::new();
    for var in ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"] {
        if let Ok(v) = std::env::var(var) {
            base_roots.push(PathBuf::from(v));
        }
    }
    if let Ok(local_app) = std::env::var("LOCALAPPDATA") {
        base_roots.push(PathBuf::from(&local_app).join("Programs"));
    }
    if let Ok(user_prof) = std::env::var("USERPROFILE") {
        base_roots.push(PathBuf::from(&user_prof).join("scoop").join("shims"));
        base_roots.push(PathBuf::from(&user_prof).join("scoop").join("apps").join("vlc").join("current"));
        base_roots.push(PathBuf::from(&user_prof).join("scoop").join("apps").join("potplayer").join("current"));
        base_roots.push(PathBuf::from(&user_prof).join("scoop").join("apps").join("mpv").join("current"));
    }
    base_roots.push(PathBuf::from("C:\\ProgramData\\chocolatey\\bin"));

    for spec in KNOWN_WINDOWS_PLAYERS {
        for root in &base_roots {
            // Direct binary in shim/bin folders
            for exe in spec.exe_names {
                add_candidate(spec.slug, spec.display_name, root.join(exe), "standard_path");
            }
            // Vendor subfolder
            for rel_dir in spec.relative_dirs {
                for exe in spec.exe_names {
                    add_candidate(
                        spec.slug,
                        spec.display_name,
                        root.join(rel_dir).join(exe),
                        "standard_path",
                    );
                }
            }
        }
    }

    // 5. System PATH lookup
    if let Some(path_os) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_os) {
            for spec in KNOWN_WINDOWS_PLAYERS {
                for exe in spec.exe_names {
                    add_candidate(spec.slug, spec.display_name, dir.join(exe), "path_env");
                }
            }
        }
    }
}

#[cfg(not(windows))]
fn discover_host_players(out: &mut Vec<ExternalPlayerInfo>, seen: &mut HashSet<String>) {
    let candidates = [
        ("mpv", "mpv Media Player", "mpv"),
        ("vlc", "VLC Media Player", "vlc"),
        ("smplayer", "SMPlayer", "smplayer"),
    ];
    if let Some(path_os) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_os) {
            for (slug, name, bin) in candidates {
                let p = dir.join(bin);
                if p.is_file() {
                    let full = p.to_string_lossy().to_string();
                    if seen.insert(normalize_exe_key(&full)) {
                        out.push(ExternalPlayerInfo {
                            id: format!("{slug}-{}", out.len() + 1),
                            name: name.to_string(),
                            executable_path: full,
                            source: "path_env".to_string(),
                            custom_id: None,
                            available: true,
                        });
                    }
                }
            }
        }
    }
}

pub fn clean_registry_exe_path(raw: &str) -> String {
    let trimmed = raw.trim();
    if let Some(rest) = trimmed.strip_prefix('"') {
        if let Some(end_quote) = rest.find('"') {
            return rest[..end_quote].to_string();
        }
    }
    // Unquoted command line e.g. `C:\Program Files\Windows Media Player\wmplayer.exe /prefetch:1`
    let lower = trimmed.to_ascii_lowercase();
    if let Some(idx) = lower.find(".exe") {
        return trimmed[..idx + 4].to_string();
    }
    trimmed.to_string()
}

pub fn add_custom_external_player(
    db: &Database,
    name: &str,
    executable_path: &str,
) -> Result<ExternalPlayerInfo> {
    let clean_path = executable_path.trim().trim_matches('"');
    if clean_path.is_empty() {
        anyhow::bail!("Executable path cannot be empty");
    }
    let path_obj = Path::new(clean_path);
    if !path_obj.is_file() {
        anyhow::bail!("Executable file does not exist: {clean_path}");
    }

    let derived_name = if name.trim().is_empty() {
        path_obj
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "Custom Player".to_string())
    } else {
        name.trim().to_string()
    };

    let now = Utc::now().timestamp_millis();
    let id = db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO custom_external_players (name, executable_path, created_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(executable_path) DO UPDATE SET name = excluded.name",
            params![&derived_name, clean_path, now],
        )?;
        let row_id: i64 = conn.query_row(
            "SELECT id FROM custom_external_players WHERE executable_path = ?1",
            params![clean_path],
            |r| r.get(0),
        )?;
        Ok(row_id)
    })?;

    Ok(ExternalPlayerInfo {
        id: format!("custom-{id}"),
        name: derived_name,
        executable_path: clean_path.to_string(),
        source: "custom".to_string(),
        custom_id: Some(id),
        available: true,
    })
}

pub fn remove_custom_external_player(db: &Database, custom_id: i64) -> Result<()> {
    db.with_conn(|conn| {
        conn.execute(
            "DELETE FROM custom_external_players WHERE id = ?1",
            params![custom_id],
        )?;
        Ok(())
    })
}

fn list_custom_players(db: &Database) -> Result<Vec<(i64, String, String)>> {
    db.with_conn(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, name, executable_path FROM custom_external_players ORDER BY id ASC",
        )?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
}

/// Builds the safe OS `Command` passing the EXACT original media URL as a discrete process argument
/// (never via `cmd.exe`, never decoding `%20` or modifying parentheses/Unicode/special characters).
pub fn build_external_player_command(executable_path: &str, original_media_url: &str) -> Result<Command> {
    let clean_exe = executable_path.trim().trim_matches('"');
    if clean_exe.is_empty() {
        anyhow::bail!("External player executable path is empty");
    }
    let exe_path = Path::new(clean_exe);
    if !exe_path.is_file() {
        anyhow::bail!(
            "Selected external player executable was not found at '{}'. Please re-detect or update the path in Settings.",
            clean_exe
        );
    }

    let url_trimmed = original_media_url.trim();
    if !url_trimmed.starts_with("http://") && !url_trimmed.starts_with("https://") {
        anyhow::bail!("External player launch only permits http:// or https:// media URLs");
    }

    let mut cmd = Command::new(exe_path);
    cmd.arg(url_trimmed)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    Ok(cmd)
}

/// Launches the external media player with the exact original URL, records Recent Media (`external`),
/// and logs structured diagnostic output.
pub fn launch_in_external_player(
    db: &Database,
    logger: &StructuredLogger,
    executable_path: &str,
    original_media_url: &str,
    custom_title: Option<&str>,
) -> Result<()> {
    let mut cmd = build_external_player_command(executable_path, original_media_url)?;
    let child = cmd.spawn().with_context(|| {
        format!("Failed to launch external media player '{executable_path}'")
    })?;

    logger.info_ctx(
        "external_player",
        "Launched media URL in external player",
        serde_json::json!({
            "executable": executable_path,
            "pid": child.id(),
            "url": original_media_url
        }),
    );

    let _ = db.record_recent_media_open(original_media_url, custom_title, None, "external");
    Ok(())
}

/// Tests an external player executable by verifying existence and spawning it cleanly.
pub fn test_external_player(
    executable_path: &str,
    logger: &StructuredLogger,
) -> Result<PlayerTestResult> {
    let clean_exe = executable_path.trim().trim_matches('"');
    let p = Path::new(clean_exe);
    if !p.is_file() {
        return Ok(PlayerTestResult {
            success: false,
            executable_path: clean_exe.to_string(),
            message: format!("Executable not found at: {clean_exe}"),
        });
    }

    let mut cmd = Command::new(p);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    match cmd.spawn() {
        Ok(child) => {
            logger.info_ctx(
                "external_player",
                "External player test succeeded",
                serde_json::json!({ "executable": clean_exe, "pid": child.id() }),
            );
            Ok(PlayerTestResult {
                success: true,
                executable_path: clean_exe.to_string(),
                message: format!(
                    "Successfully launched '{}' (PID {}).",
                    p.file_name().unwrap_or_default().to_string_lossy(),
                    child.id()
                ),
            })
        }
        Err(err) => {
            logger.error_ctx(
                "external_player",
                "External player test failed",
                serde_json::json!({ "executable": clean_exe, "error": err.to_string() }),
            );
            Ok(PlayerTestResult {
                success: false,
                executable_path: clean_exe.to_string(),
                message: format!("Failed to launch player: {err}"),
            })
        }
    }
}
