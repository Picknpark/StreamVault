use anyhow::{Context, Result};
use percent_encoding::percent_decode_str;
use reqwest::header::{ACCEPT_RANGES, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, RANGE};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use url::Url;

pub const VIDEO_EXTENSIONS: &[&str] = &[
    "mkv", "mp4", "avi", "mov", "wmv", "flv", "webm", "m4v", "mpeg", "mpg", "ts", "m2ts", "ogv",
    "3gp", "vob", "divx", "rmvb", "asf", "m3u8", "mpd",
];

pub const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "flac", "wav", "aac", "m4a", "ogg", "opus", "wma", "ac3", "eac3", "dts", "alac",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaKind {
    Video,
    Audio,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaDetectionResult {
    pub is_media: bool,
    pub media_kind: Option<MediaKind>,
    pub original_url: String,
    pub suggested_filename: String,
    pub extension: Option<String>,
    pub mime_type: Option<String>,
    pub content_length: Option<u64>,
    pub supports_range: bool,
    pub detection_source: String, // "extension" | "mime_header" | "content_disposition" | "none"
}

/// Preserves already percent-encoded URLs (such as Dhaka-Flix `%20`, `%28`, `%E2%98%85`)
/// while safely encoding any raw spaces or raw non-ASCII characters if pasted unencoded.
pub fn preserve_or_sanitize_http_url(raw: &str) -> Result<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        anyhow::bail!("URL is empty");
    }

    // Encode raw literal spaces without touching existing `%XX` sequences
    let mut pre_cleaned = String::with_capacity(trimmed.len() + 16);
    for ch in trimmed.chars() {
        match ch {
            ' ' => pre_cleaned.push_str("%20"),
            c if c.is_control() => {}
            c => pre_cleaned.push(c),
        }
    }

    let parsed = Url::parse(&pre_cleaned).context("Invalid HTTP/HTTPS URL")?;
    match parsed.scheme() {
        "http" | "https" => Ok(parsed.to_string()),
        other => anyhow::bail!("Unsupported URL scheme '{other}' (expected http or https)"),
    }
}

/// Resolves user input typed into the browser Address/Search bar:
/// - Full `http://` or `https://` URLs -> preserved as-is
/// - Private/LAN IPs (`172.16.50.12`, `192.168.1.10`, `10.x.x.x`, `127.0.0.1`, `localhost`) -> `http://...`
/// - Public domains (`facebook.com`, `youtube.com`) -> `https://...`
/// - Search terms -> formatted via `search_template`
pub fn resolve_navigation_input(input: &str, search_template: &str) -> Result<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        anyhow::bail!("Address input cannot be empty");
    }
    if trimmed.eq_ignore_ascii_case("about:blank") {
        return Ok("about:blank".to_string());
    }

    let lower = trimmed.to_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return preserve_or_sanitize_http_url(trimmed);
    }

    // Detect if input has whitespace (search query unless it starts with scheme)
    if !trimmed.contains(char::is_whitespace) {
        let host_part = trimmed.split('/').next().unwrap_or(trimmed);
        let host_no_port = host_part.split(':').next().unwrap_or(host_part);

        if is_ipv4_or_local_host(host_no_port) || host_part.contains(':') {
            return preserve_or_sanitize_http_url(&format!("http://{trimmed}"));
        }

        if looks_like_domain(host_no_port) {
            return preserve_or_sanitize_http_url(&format!("https://{trimmed}"));
        }
    }

    let encoded_query: String = url::form_urlencoded::byte_serialize(trimmed.as_bytes()).collect();
    let template = if search_template.contains("{query}") {
        search_template.replace("{query}", &encoded_query)
    } else {
        format!("https://www.google.com/search?q={encoded_query}")
    };
    Ok(template)
}

/// Resolves a configured `homepage_url` safely for New Tab, Home button, startup, and closing
/// the last browser tab: when `homepage_url` is blank (`""`) or `about:blank`, opens `"about:blank"`.
pub fn resolve_homepage_or_blank(homepage_url: &str, search_template: &str) -> String {
    let trimmed = homepage_url.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("about:blank") {
        return "about:blank".to_string();
    }
    resolve_navigation_input(trimmed, search_template)
        .unwrap_or_else(|_| "about:blank".to_string())
}

fn is_ipv4_or_local_host(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") || host.ends_with(".local") || host.ends_with(".lan") {
        return true;
    }
    let parts: Vec<&str> = host.split('.').collect();
    if parts.len() == 4 && parts.iter().all(|p| p.parse::<u8>().is_ok()) {
        return true;
    }
    false
}

fn looks_like_domain(host: &str) -> bool {
    if !host.contains('.') || host.starts_with('.') || host.ends_with('.') {
        return false;
    }
    let tld = host.rsplit('.').next().unwrap_or("");
    tld.len() >= 2
        && tld.chars().all(|c| c.is_ascii_alphabetic())
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
}

/// Normalizes a media URL into a stable lookup key for playback positions and active-link highlighting
/// without merging distinct files (preserves path case and query parameters, strips `#fragment` and default ports).
pub fn normalize_media_key(raw_url: &str) -> String {
    if let Ok(mut u) = Url::parse(raw_url.trim()) {
        u.set_fragment(None);
        // Canonicalize percent-encoding hex digits to uppercase so `%e2%98%85` == `%E2%98%85`
        let s = u.to_string();
        canonicalize_percent_hex(&s)
    } else {
        raw_url.trim().to_string()
    }
}

fn canonicalize_percent_hex(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let h1 = bytes[i + 1] as char;
            let h2 = bytes[i + 2] as char;
            if h1.is_ascii_hexdigit() && h2.is_ascii_hexdigit() {
                out.push('%');
                out.push(h1.to_ascii_uppercase());
                out.push(h2.to_ascii_uppercase());
                i += 3;
                continue;
            }
        }
        if let Some(ch) = s[i..].chars().next() {
            out.push(ch);
            i += ch.len_utf8();
        } else {
            break;
        }
    }
    out
}

/// Fast synchronous check by URL path extension.
pub fn classify_media_extension(ext: &str) -> Option<MediaKind> {
    let lower = ext.trim_start_matches('.').to_ascii_lowercase();
    if VIDEO_EXTENSIONS.contains(&lower.as_str()) {
        Some(MediaKind::Video)
    } else if AUDIO_EXTENSIONS.contains(&lower.as_str()) {
        Some(MediaKind::Audio)
    } else {
        None
    }
}

pub fn classify_mime_type(mime: &str) -> Option<(MediaKind, Option<&'static str>)> {
    let essence = mime
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    match essence.as_str() {
        "video/x-matroska" | "application/x-matroska" => Some((MediaKind::Video, Some("mkv"))),
        "video/mp4" => Some((MediaKind::Video, Some("mp4"))),
        "video/x-msvideo" | "video/avi" => Some((MediaKind::Video, Some("avi"))),
        "video/quicktime" => Some((MediaKind::Video, Some("mov"))),
        "video/x-ms-wmv" => Some((MediaKind::Video, Some("wmv"))),
        "video/x-flv" => Some((MediaKind::Video, Some("flv"))),
        "video/webm" => Some((MediaKind::Video, Some("webm"))),
        "video/x-m4v" => Some((MediaKind::Video, Some("m4v"))),
        "video/mpeg" => Some((MediaKind::Video, Some("mpeg"))),
        "video/mp2t" => Some((MediaKind::Video, Some("ts"))),
        "video/ogg" => Some((MediaKind::Video, Some("ogv"))),
        "audio/mpeg" | "audio/mp3" => Some((MediaKind::Audio, Some("mp3"))),
        "audio/flac" | "audio/x-flac" => Some((MediaKind::Audio, Some("flac"))),
        "audio/wav" | "audio/x-wav" | "audio/wave" => Some((MediaKind::Audio, Some("wav"))),
        "audio/aac" | "audio/aacp" => Some((MediaKind::Audio, Some("aac"))),
        "audio/mp4" | "audio/x-m4a" => Some((MediaKind::Audio, Some("m4a"))),
        "audio/ogg" | "application/ogg" => Some((MediaKind::Audio, Some("ogg"))),
        "audio/opus" => Some((MediaKind::Audio, Some("opus"))),
        _ if essence.starts_with("video/") => Some((MediaKind::Video, None)),
        _ if essence.starts_with("audio/") => Some((MediaKind::Audio, None)),
        _ => None,
    }
}

/// Fast URL-only media check before or alongside HTTP header inspection.
pub fn detect_media_from_url_only(raw_url: &str) -> MediaDetectionResult {
    let clean = raw_url.trim().to_string();
    let mut ext_found: Option<String> = None;
    if let Ok(parsed) = Url::parse(&clean) {
        if matches!(parsed.scheme(), "http" | "https") {
            if let Some(seg) = parsed
                .path_segments()
                .and_then(|mut s| s.rfind(|part| !part.is_empty()))
            {
                let decoded = percent_decode_str(seg).decode_utf8_lossy();
                if let Some((_, ext)) = decoded.rsplit_once('.') {
                    if classify_media_extension(ext).is_some() {
                        ext_found = Some(ext.to_ascii_lowercase());
                    }
                }
            }
        }
    }

    let kind = ext_found.as_deref().and_then(classify_media_extension);
    let suggested_filename = extract_server_filename(&clean, None, None);

    MediaDetectionResult {
        is_media: kind.is_some(),
        media_kind: kind,
        original_url: clean,
        suggested_filename,
        extension: ext_found,
        mime_type: None,
        content_length: None,
        supports_range: false,
        detection_source: if kind.is_some() {
            "extension".to_string()
        } else {
            "none".to_string()
        },
    }
}

/// Extracts the human-readable media title from a URL (decoded filename without extension if helpful, or full filename).
pub fn extract_display_title_from_url(raw_url: &str) -> String {
    let filename = extract_server_filename(raw_url, None, None);
    if filename.is_empty() || filename == "media" {
        raw_url.trim().to_string()
    } else {
        filename
    }
}

/// Extracts the exact original server filename from `Content-Disposition` headers or the URL path,
/// preserving Unicode (`★`, `—`, `–`), spaces, parentheses, dots, and the original extension.
pub fn extract_server_filename(
    raw_url: &str,
    content_disposition: Option<&str>,
    content_type: Option<&str>,
) -> String {
    // 1. Prefer RFC 5987 / RFC 6266 Content-Disposition filename if present
    if let Some(cd) = content_disposition {
        if let Some(cd_name) = parse_content_disposition_filename(cd) {
            let sanitized = sanitize_windows_filename(&cd_name);
            if !sanitized.is_empty() {
                return ensure_extension(sanitized, content_type);
            }
        }
    }

    // 2. Extract final non-empty URL path segment and UTF-8 percent-decode it
    if let Ok(parsed) = Url::parse(raw_url.trim()) {
        if let Some(seg) = parsed
            .path_segments()
            .and_then(|mut s| s.rfind(|part| !part.is_empty()))
        {
            let decoded = percent_decode_str(seg).decode_utf8_lossy().to_string();
            let sanitized = sanitize_windows_filename(&decoded);
            if !sanitized.is_empty() {
                return ensure_extension(sanitized, content_type);
            }
        }
    }

    ensure_extension("media_stream".to_string(), content_type)
}

fn ensure_extension(filename: String, content_type: Option<&str>) -> String {
    if filename.contains('.') && !filename.ends_with('.') {
        return filename;
    }
    if let Some(ct) = content_type {
        if let Some((_, Some(ext))) = classify_mime_type(ct) {
            return format!("{}.{ext}", filename.trim_end_matches('.'));
        }
    }
    filename
}

pub fn parse_content_disposition_filename(header_val: &str) -> Option<String> {
    // Check `filename*=UTF-8''...` first
    for part in header_val.split(';') {
        let p = part.trim();
        if let Some(rest) = p
            .strip_prefix("filename*=")
            .or_else(|| p.strip_prefix("FILENAME*="))
        {
            let cleaned = rest.trim().trim_matches('"');
            let encoded = cleaned
                .split("''")
                .nth(1)
                .unwrap_or(cleaned);
            let decoded = percent_decode_str(encoded).decode_utf8_lossy().to_string();
            if !decoded.trim().is_empty() {
                return Some(decoded.trim().to_string());
            }
        }
    }

    // Check `filename="..."` or `filename=...`
    for part in header_val.split(';') {
        let p = part.trim();
        if let Some(rest) = p
            .strip_prefix("filename=")
            .or_else(|| p.strip_prefix("FILENAME="))
        {
            let unquoted = rest.trim().trim_matches('"').trim_matches('\'');
            let decoded = percent_decode_str(unquoted).decode_utf8_lossy().to_string();
            if !decoded.trim().is_empty() {
                return Some(decoded.trim().to_string());
            }
        }
    }

    None
}

/// Sanitizes ONLY characters strictly illegal on Windows NTFS/exFAT filesystems
/// (`< > : " / \ | ? *` and ASCII control characters `0x00..=0x1F`),
/// preserving Unicode characters, spaces, parentheses, brackets, dashes, and dots.
pub fn sanitize_windows_filename(name: &str) -> String {
    let mut cleaned = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_control() || matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
            cleaned.push('_');
        } else {
            cleaned.push(ch);
        }
    }

    // Trim trailing spaces and dots (disallowed by Win32 file creation APIs)
    let trimmed = cleaned.trim().trim_end_matches([' ', '.']).to_string();
    if trimmed.is_empty() {
        return "media_file".to_string();
    }

    // Guard reserved DOS/Win32 device names (CON, PRN, AUX, NUL, COM1..9, LPT1..9)
    let stem = trimmed
        .split('.')
        .next()
        .unwrap_or(&trimmed)
        .to_ascii_uppercase();
    let is_reserved = matches!(
        stem.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    );

    if is_reserved {
        format!("_{trimmed}")
    } else {
        trimmed
    }
}

/// Inspects both URL structure AND live HTTP response headers (`HEAD` with automatic fallback
/// to `GET Range: bytes=0-1`) to identify media resources, MIME types, Range support,
/// `Content-Length`, and original server filenames.
pub async fn probe_media_url(client: &reqwest::Client, raw_url: &str) -> Result<MediaDetectionResult> {
    let preserved_url = preserve_or_sanitize_http_url(raw_url)?;
    let mut base = detect_media_from_url_only(&preserved_url);

    // 1. Try lightweight HEAD request
    let mut content_type: Option<String> = None;
    let mut content_disp: Option<String> = None;
    let mut content_len: Option<u64> = None;
    let mut supports_range = false;

    if let Ok(resp) = client
        .head(&preserved_url)
        .timeout(Duration::from_secs(6))
        .send()
        .await
    {
        if resp.status().is_success() {
            content_type = resp
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            content_disp = resp
                .headers()
                .get(CONTENT_DISPOSITION)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            content_len = resp
                .headers()
                .get(CONTENT_LENGTH)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok());
            supports_range = resp
                .headers()
                .get(ACCEPT_RANGES)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_ascii_lowercase().contains("bytes"))
                .unwrap_or(false);
        }
    }

    // 2. Fallback/verify with `GET Range: bytes=0-1` (streamed, immediately dropped so 0 body bloat!)
    if !supports_range || content_len.is_none() || content_type.is_none() {
        if let Ok(resp) = client
            .get(&preserved_url)
            .header(RANGE, "bytes=0-1")
            .timeout(Duration::from_secs(8))
            .send()
            .await
        {
            if resp.status() == reqwest::StatusCode::PARTIAL_CONTENT {
                supports_range = true;
                if let Some(cr) = resp
                    .headers()
                    .get(CONTENT_RANGE)
                    .and_then(|v| v.to_str().ok())
                {
                    // e.g. "bytes 0-1/734003200"
                    if let Some(total_str) = cr.rsplit('/').next() {
                        if let Ok(total) = total_str.trim().parse::<u64>() {
                            content_len = Some(total);
                        }
                    }
                }
            } else if resp.status().is_success() && content_len.is_none() {
                content_len = resp
                    .headers()
                    .get(CONTENT_LENGTH)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.parse::<u64>().ok());
            }

            if content_type.is_none() {
                content_type = resp
                    .headers()
                    .get(CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.to_string());
            }
            if content_disp.is_none() {
                content_disp = resp
                    .headers()
                    .get(CONTENT_DISPOSITION)
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.to_string());
            }
        }
    }

    let filename = extract_server_filename(
        &preserved_url,
        content_disp.as_deref(),
        content_type.as_deref(),
    );
    base.suggested_filename = filename.clone();
    base.mime_type = content_type.clone();
    base.content_length = content_len;
    base.supports_range = supports_range;

    // Determine media kind if URL alone didn't have a media extension
    if !base.is_media {
        if let Some(ref ct) = content_type {
            if let Some((kind, inferred_ext)) = classify_mime_type(ct) {
                base.is_media = true;
                base.media_kind = Some(kind);
                base.extension = inferred_ext.map(|s| s.to_string());
                base.detection_source = "mime_header".to_string();
            }
        }
    }

    // Also check if Content-Disposition filename revealed a media extension (common for `application/octet-stream`)
    if !base.is_media {
        if let Some((_, ext)) = filename.rsplit_once('.') {
            if let Some(kind) = classify_media_extension(ext) {
                base.is_media = true;
                base.media_kind = Some(kind);
                base.extension = Some(ext.to_ascii_lowercase());
                base.detection_source = "content_disposition".to_string();
            }
        }
    }

    Ok(base)
}
