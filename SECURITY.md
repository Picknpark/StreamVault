# Security Policy

## Supported Versions

| Version | Supported |
|---|---|
| `1.0.x` | :white_check_mark: Yes |
| `< 1.0` | :x: No |

---

## StreamVault Security Architecture

Because StreamVault embeds a general-purpose multi-tab browser (`WebView2`) alongside native Rust capabilities (file downloads, external media player execution, and SQLite storage), strict security boundaries are enforced by design:

1. **Strict Capability Separation (`src-tauri/capabilities/`)**:
   - Privileged IPC commands (`shell-local.json`) are bound **exclusively** to the local `"main"` application shell webview (`local: true`).
   - External untrusted websites rendered inside `"tab-*"` child webviews receive **only** `tab-media-context.json`, which exposes a single per-session cryptographic-nonce-guarded context menu bridge (`tab_context_bridge`).
2. **WebView2 Profile Segregation**:
   - The privileged local UI shell (`data/webview2/shell`) and external browsing tabs (`data/webview2/tabs`) run in separate WebView2 user data directories.
3. **Zero Shell Interpolation on External Player Launch**:
   - External media players (`external_player.rs`) and Windows Explorer folder reveals (`commands.rs`) are invoked via native Win32 APIs (`SHOpenFolderAndSelectItems` / `ShellExecuteW`) and OS `Command::new(exe).arg(raw_url)` argument vectors—never via `cmd.exe /c`, preventing command injection from maliciously crafted media URLs.
4. **Loopback-Only Streaming Proxy**:
   - The progressive HTTP Range cache server (`stream_cache.rs`) binds strictly to `127.0.0.1:0` (localhost loopback only, never `0.0.0.0`) with randomized session UUID paths.
5. **Path Traversal & Overwrite Protection on Downloads**:
   - Server-supplied filenames (`Content-Disposition` and URL path segments) are sanitized against Windows reserved device names (`CON`, `NUL`, `COM1`, etc.), control characters, and `..` path traversal segments before pre-populating Save As dialogs or auto-naming downloads.

---

## Reporting a Vulnerability

**Please do NOT report security vulnerabilities through public GitHub Issues.**

Instead, please report suspected vulnerabilities privately using **GitHub Private Vulnerability Reporting**:

1. Navigate to the **Security** tab of this GitHub repository.
2. Click **Report a vulnerability** (`Security → Advisories → New draft security advisory`).
3. Include:
   - Affected StreamVault version and Windows build,
   - Clear reproduction steps or proof-of-concept,
   - Impact assessment (e.g., capability bypass, path traversal, arbitrary execution).

We aim to acknowledge receipt within **72 hours** and coordinate a fix and credit before public disclosure.
