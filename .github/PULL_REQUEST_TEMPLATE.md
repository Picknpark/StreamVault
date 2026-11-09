## Summary

<!-- Briefly describe what this PR changes and why. -->

## Related Issue

<!-- e.g., Closes #12 -->

## Checklist

- [ ] `cargo test -p streamvault-core` passes with 0 failures
- [ ] All Vanilla JS modules pass `node --check ui/js/*.js`
- [ ] Portable path invariant preserved (no `%APPDATA%` / `%LOCALAPPDATA%` writes introduced)
- [ ] Temporary stream cache (`data/cache/media`) remains isolated from permanent downloads
- [ ] If new Tauri IPC commands were added: updated `src-tauri/build.rs` and `src-tauri/capabilities/shell-local.json` (`main` webview only, never `tab-*`)
- [ ] No binary `.exe`, `.dll`, `.db`, or `data/webview2/` files committed
