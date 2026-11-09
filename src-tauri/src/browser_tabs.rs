use anyhow::{Context, Result};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use streamvault_core::db::session::SavedSessionTab;
use streamvault_core::media_detect::{
    detect_media_from_url_only, probe_media_url, resolve_homepage_or_blank,
    resolve_navigation_input, MediaDetectionResult,
};
use streamvault_core::StreamVaultCore;
use tauri::webview::{DownloadEvent, NewWindowResponse, PageLoadEvent, WebviewBuilder};
use tauri::{AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, Rect, Webview, WebviewUrl};
use url::Url;
use uuid::Uuid;

const TAB_SCRIPT_TEMPLATE: &str = include_str!("tab_content_script.js");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserTabInfo {
    pub id: String,
    pub title: String,
    pub url: String,
    pub is_active: bool,
    pub is_loading: bool,
    pub can_go_back: bool,
    pub can_go_forward: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ViewportBounds {
    pub visible: bool,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Default for ViewportBounds {
    fn default() -> Self {
        Self {
            visible: false,
            x: 220.0,
            y: 118.0,
            width: 1020.0,
            height: 640.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaDetectedPayload {
    pub tab_id: String,
    pub detection: MediaDetectionResult,
    pub preferred_method: String,
}

#[derive(Clone)]
pub struct BrowserTabManager {
    core: StreamVaultCore,
    secret_nonce: String,
    startup_lock: Arc<Mutex<bool>>,
    next_num: Arc<AtomicU64>,
    tab_order: Arc<RwLock<Vec<String>>>,
    tabs: Arc<RwLock<HashMap<String, BrowserTabInfo>>>,
    active_tab_id: Arc<RwLock<Option<String>>>,
    viewport: Arc<RwLock<ViewportBounds>>,
    allow_media_in_tab: Arc<RwLock<HashMap<String, String>>>,
}

impl BrowserTabManager {
    pub fn new(core: StreamVaultCore) -> Self {
        let secret_nonce = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        Self {
            core,
            secret_nonce,
            startup_lock: Arc::new(Mutex::new(false)),
            next_num: Arc::new(AtomicU64::new(1)),
            tab_order: Arc::new(RwLock::new(Vec::new())),
            tabs: Arc::new(RwLock::new(HashMap::new())),
            active_tab_id: Arc::new(RwLock::new(None)),
            viewport: Arc::new(RwLock::new(ViewportBounds::default())),
            allow_media_in_tab: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn verify_nonce(&self, candidate: &str) -> bool {
        !candidate.is_empty() && candidate == self.secret_nonce
    }

    pub fn list_tabs(&self) -> Vec<BrowserTabInfo> {
        let order = self.tab_order.read();
        let map = self.tabs.read();
        let active = self.active_tab_id.read().clone();
        order
            .iter()
            .filter_map(|id| {
                let mut info = map.get(id)?.clone();
                info.is_active = active.as_deref() == Some(id.as_str());
                Some(info)
            })
            .collect()
    }

    /// Restores previous session tabs if enabled in Settings, or initializes a default first tab.
    /// Idempotent and thread-safe — safe to call from background startup or on first Browser switch.
    pub fn initialize_startup_tabs(&self, app: &AppHandle) -> Result<Vec<BrowserTabInfo>> {
        let mut initialized = self.startup_lock.lock();
        if *initialized || !self.tab_order.read().is_empty() {
            *initialized = true;
            return Ok(self.list_tabs());
        }

        let settings = self.core.db.get_settings()?;
        if settings.restore_previous_session || settings.startup_behavior == "restore_session" {
            let saved = self.core.db.load_browser_session().unwrap_or_default();
            if !saved.is_empty() {
                let mut active_to_focus: Option<String> = None;
                for st in saved {
                    let restored = if st.url.trim().is_empty()
                        || st.url.trim().eq_ignore_ascii_case("about:blank")
                    {
                        self.create_tab_internal_opts(app, "about:blank", Some("New Tab"), false, false)
                    } else if detect_media_from_url_only(&st.url).is_media {
                        self.create_tab_allowing_media(app, &st.url, Some(&st.title), false)
                    } else {
                        self.create_tab(app, Some(&st.url), false)
                    };
                    if let Ok(tab) = restored {
                        if st.is_active {
                            active_to_focus = Some(tab.id);
                        }
                    }
                }
                if !self.tab_order.read().is_empty() {
                    *initialized = true;
                    if let Some(act_id) = active_to_focus {
                        let _ = self.switch_tab(app, &act_id);
                    } else if let Some(first_id) = self.tab_order.read().first().cloned() {
                        let _ = self.switch_tab(app, &first_id);
                    }
                    return Ok(self.list_tabs());
                }
            }
        }

        let initial_url =
            resolve_homepage_or_blank(&settings.homepage_url, &settings.search_engine_template);
        let _ = self.create_tab_internal_opts(app, &initial_url, None, true, false)?;
        *initialized = true;
        Ok(self.list_tabs())
    }

    pub fn create_tab(
        &self,
        app: &AppHandle,
        raw_input: Option<&str>,
        make_active: bool,
    ) -> Result<BrowserTabInfo> {
        let settings = self.core.db.get_settings()?;
        let target_str = match raw_input.map(|s| s.trim()).filter(|s| !s.is_empty()) {
            Some(inp) => resolve_navigation_input(inp, &settings.search_engine_template)?,
            None => resolve_homepage_or_blank(
                &settings.homepage_url,
                &settings.search_engine_template,
            ),
        };

        if target_str == "about:blank" {
            return self.create_tab_internal_opts(
                app,
                "about:blank",
                Some("New Tab"),
                make_active,
                false,
            );
        }

        // Check if user directly pasted/opened a media URL into a new tab!
        if settings.auto_detect_media {
            let quick = detect_media_from_url_only(&target_str);
            if quick.is_media {
                self.dispatch_media_detected(app.clone(), "direct".to_string(), target_str.clone());
                // Ensure at least one browser tab exists even if user opened a direct media URL first
                if self.tab_order.read().is_empty() {
                    let fallback_home = resolve_homepage_or_blank(
                        &settings.homepage_url,
                        &settings.search_engine_template,
                    );
                    return self.create_tab_internal_opts(
                        app,
                        &fallback_home,
                        None,
                        make_active,
                        false,
                    );
                }
                if let Some(curr_active) = self.active_tab_id.read().clone() {
                    if let Some(existing) = self.tabs.read().get(&curr_active).cloned() {
                        return Ok(existing);
                    }
                }
            }
        }

        self.create_tab_internal_opts(app, &target_str, None, make_active, false)
    }

    /// Explicitly opens a URL (including direct media links like `.mkv`, `.mp4`, `.mp3`)
    /// inside a brand-new browser tab when the user chooses `+ Open in New Tab`.
    pub fn create_tab_allowing_media(
        &self,
        app: &AppHandle,
        raw_input: &str,
        title_hint: Option<&str>,
        make_active: bool,
    ) -> Result<BrowserTabInfo> {
        let settings = self.core.db.get_settings()?;
        let trimmed = raw_input.trim();
        if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("about:blank") {
            return self.create_tab_internal_opts(
                app,
                "about:blank",
                Some("New Tab"),
                make_active,
                false,
            );
        }

        let target_str = resolve_navigation_input(trimmed, &settings.search_engine_template)?;
        let quick = detect_media_from_url_only(&target_str);
        let initial_title = title_hint
            .map(|t| t.trim())
            .filter(|t| !t.is_empty())
            .map(|t| t.to_string())
            .or_else(|| {
                if quick.is_media && !quick.suggested_filename.trim().is_empty() {
                    Some(quick.suggested_filename.clone())
                } else {
                    None
                }
            });

        let created = self.create_tab_internal_opts(
            app,
            &target_str,
            initial_title.as_deref(),
            make_active,
            quick.is_media,
        )?;

        if quick.is_media {
            // Surface the non-blocking media bar ("ask") on the newly created tab so the user
            // can also one-click Play in StreamVault / External Player / Download if desired.
            self.dispatch_media_detected_ask_only(app.clone(), created.id.clone(), target_str);
        }

        Ok(created)
    }

    fn is_media_allowed_for_tab(&self, tab_id: &str, candidate_url: &str) -> bool {
        let map = self.allow_media_in_tab.read();
        let Some(allowed_url) = map.get(tab_id) else {
            return false;
        };
        allowed_url == candidate_url
            || streamvault_core::media_detect::normalize_media_key(allowed_url)
                == streamvault_core::media_detect::normalize_media_key(candidate_url)
    }

    fn create_tab_internal_opts(
        &self,
        app: &AppHandle,
        resolved_url: &str,
        title_override: Option<&str>,
        make_active: bool,
        allow_direct_media: bool,
    ) -> Result<BrowserTabInfo> {
        let parsed_url = Url::parse(resolved_url).context("Invalid resolved tab URL")?;
        let num = self.next_num.fetch_add(1, Ordering::SeqCst);
        let tab_id = format!("tab-{num}");

        if allow_direct_media {
            self.allow_media_in_tab
                .write()
                .insert(tab_id.clone(), parsed_url.to_string());
        }

        let main_window = app
            .get_window("main")
            .context("Main StreamVault window not found")?;

        let injected_script = TAB_SCRIPT_TEMPLATE
            .replace("__SV_SECRET_TOKEN__", &self.secret_nonce)
            .replace("__SV_TAB_ID__", &tab_id);

        let mgr_nav = self.clone();
        let app_nav = app.clone();
        let tab_nav_id = tab_id.clone();

        let mgr_new_win = self.clone();
        let app_new_win = app.clone();

        let mgr_title = self.clone();
        let app_title = app.clone();
        let tab_title_id = tab_id.clone();

        let mgr_load = self.clone();
        let app_load = app.clone();
        let tab_load_id = tab_id.clone();

        let mgr_dl = self.clone();
        let app_dl = app.clone();
        let tab_dl_id = tab_id.clone();

        let mut builder = WebviewBuilder::new(&tab_id, WebviewUrl::External(parsed_url.clone()))
            .data_directory(self.core.paths.webview2_tabs_dir.clone())
            .initialization_script(injected_script)
            .zoom_hotkeys_enabled(true)
            .on_navigation(move |nav_url| {
                let scheme = nav_url.scheme();
                if scheme != "http" && scheme != "https" && scheme != "about" {
                    return false;
                }
                if scheme == "http" || scheme == "https" {
                    let url_str = nav_url.as_str().to_string();
                    if mgr_nav.is_media_allowed_for_tab(&tab_nav_id, &url_str) {
                        mgr_nav.update_tab_url(&app_nav, &tab_nav_id, &url_str);
                        return true;
                    }
                    let auto_detect = mgr_nav
                        .core
                        .db
                        .get_settings()
                        .map(|s| s.auto_detect_media)
                        .unwrap_or(true);
                    if auto_detect {
                        let quick = detect_media_from_url_only(&url_str);
                        if quick.is_media {
                            mgr_nav.dispatch_media_detected(
                                app_nav.clone(),
                                tab_nav_id.clone(),
                                url_str,
                            );
                            return false;
                        }
                    }
                    mgr_nav.update_tab_url(&app_nav, &tab_nav_id, &url_str);
                }
                true
            })
            .on_new_window(move |popup_url, _features| {
                let url_str = popup_url.as_str().to_string();
                if url_str.starts_with("http://") || url_str.starts_with("https://") {
                    let mgr_c = mgr_new_win.clone();
                    let app_c = app_new_win.clone();
                    std::thread::spawn(move || {
                        let _ = mgr_c.create_tab_allowing_media(&app_c, &url_str, None, true);
                    });
                }
                NewWindowResponse::Deny
            })
            .on_document_title_changed(move |wv, new_title| {
                mgr_title.on_tab_title_changed(&app_title, &tab_title_id, &wv, new_title);
            })
            .on_page_load(move |wv, payload| {
                let loading = matches!(payload.event(), PageLoadEvent::Started);
                mgr_load.on_tab_page_load(
                    &app_load,
                    &tab_load_id,
                    &wv,
                    payload.url().as_str(),
                    loading,
                );
            })
            .on_download(move |wv, event| {
                if let DownloadEvent::Requested { url, .. } = event {
                    let url_str = url.as_str().to_string();
                    if url_str.starts_with("http://") || url_str.starts_with("https://") {
                        if mgr_dl.is_media_allowed_for_tab(&tab_dl_id, &url_str) {
                            // WebView2 cannot render this media container inline (e.g. .mkv / .avi / .ts)
                            // and triggered DownloadEvent::Requested on the newly opened tab.
                            // Keep the tab open at the media URL and render our interactive Direct Media Card!
                            mgr_dl.render_direct_media_tab_fallback(&app_dl, &tab_dl_id, &wv, &url_str);
                            return false;
                        }
                        // Intercept WebView2 downloads (such as extensionless MIME media streams or MKV attachments!)
                        mgr_dl.dispatch_media_or_download_intercept(
                            app_dl.clone(),
                            tab_dl_id.clone(),
                            url_str,
                        );
                        return false;
                    }
                }
                false
            });

        builder = builder.user_agent(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
        );

        let vp = *self.viewport.read();
        let show_immediately = make_active && vp.visible && vp.width > 10.0 && vp.height > 10.0;
        let pos = if show_immediately {
            LogicalPosition::new(vp.x.max(0.0), vp.y.max(0.0))
        } else {
            LogicalPosition::new(-20000.0, -20000.0)
        };
        let size = LogicalSize::new(vp.width.max(100.0), vp.height.max(100.0));

        let webview = main_window
            .add_child(builder, pos, size)
            .context("Failed creating WebView2 browser tab")?;

        let is_blank = parsed_url.as_str() == "about:blank";
        let initial_title = title_override
            .map(|t| t.trim())
            .filter(|t| !t.is_empty())
            .map(|t| t.to_string())
            .unwrap_or_else(|| {
                if is_blank {
                    "New Tab".to_string()
                } else {
                    parsed_url.host_str().unwrap_or("New Tab").to_string()
                }
            });

        let info = BrowserTabInfo {
            id: tab_id.clone(),
            title: initial_title,
            url: parsed_url.to_string(),
            is_active: make_active,
            is_loading: !is_blank,
            can_go_back: false,
            can_go_forward: false,
        };

        self.tabs.write().insert(tab_id.clone(), info.clone());
        self.tab_order.write().push(tab_id.clone());

        if make_active || self.active_tab_id.read().is_none() {
            *self.active_tab_id.write() = Some(tab_id.clone());
        } else {
            let _ = webview.hide();
        }

        self.apply_viewport_to_all(app);
        self.persist_session_and_emit(app);
        Ok(self.tabs.read().get(&tab_id).cloned().unwrap_or(info))
    }

    pub fn switch_tab(&self, app: &AppHandle, tab_id: &str) -> Result<Vec<BrowserTabInfo>> {
        if !self.tabs.read().contains_key(tab_id) {
            anyhow::bail!("Browser tab '{tab_id}' not found");
        }
        *self.active_tab_id.write() = Some(tab_id.to_string());
        self.apply_viewport_to_all(app);
        self.push_active_media_to_tab(app, tab_id);
        self.persist_session_and_emit(app);
        Ok(self.list_tabs())
    }

    pub fn close_tab(&self, app: &AppHandle, tab_id: &str) -> Result<Vec<BrowserTabInfo>> {
        if let Some(wv) = app.get_webview(tab_id) {
            let _ = wv.hide();
            let _ = wv.close();
        }
        self.allow_media_in_tab.write().remove(tab_id);
        self.tabs.write().remove(tab_id);
        {
            let mut order = self.tab_order.write();
            if let Some(pos) = order.iter().position(|x| x == tab_id) {
                order.remove(pos);
                let was_active = self.active_tab_id.read().as_deref() == Some(tab_id);
                if was_active {
                    let next_active = if order.is_empty() {
                        None
                    } else {
                        let pick_idx = pos.min(order.len() - 1);
                        Some(order[pick_idx].clone())
                    };
                    *self.active_tab_id.write() = next_active;
                }
            }
        }

        if self.tab_order.read().is_empty() {
            let settings = self.core.db.get_settings()?;
            let fallback_home = resolve_homepage_or_blank(
                &settings.homepage_url,
                &settings.search_engine_template,
            );
            let _ = self.create_tab_internal_opts(app, &fallback_home, None, true, false)?;
        } else {
            self.apply_viewport_to_all(app);
            self.persist_session_and_emit(app);
        }

        Ok(self.list_tabs())
    }

    pub fn navigate_tab(
        &self,
        app: &AppHandle,
        tab_id: Option<&str>,
        raw_input: &str,
    ) -> Result<BrowserTabInfo> {
        let settings = self.core.db.get_settings()?;
        let target_url = resolve_navigation_input(raw_input, &settings.search_engine_template)?;

        let resolved_id = match tab_id {
            Some(id) if self.tabs.read().contains_key(id) => id.to_string(),
            _ => match self.active_tab_id.read().clone() {
                Some(act) => act,
                None => return self.create_tab(app, Some(&target_url), true),
            },
        };

        // Clear any prior direct-media tab override when the user explicitly navigates away
        self.allow_media_in_tab.write().remove(&resolved_id);

        // Direct media URL detection before navigating WebView2!
        if settings.auto_detect_media && target_url != "about:blank" {
            let quick = detect_media_from_url_only(&target_url);
            if quick.is_media {
                self.dispatch_media_detected(app.clone(), resolved_id.clone(), target_url);
                return self
                    .tabs
                    .read()
                    .get(&resolved_id)
                    .cloned()
                    .context("Tab missing");
            }

            // Optional lightweight header probe if URL has no extension or ambiguous path
            if settings.probe_http_headers_for_media && ! looks_like_html_route(&target_url) {
                let mgr = self.clone();
                let app_c = app.clone();
                let tid_c = resolved_id.clone();
                let u_c = target_url.clone();
                tauri::async_runtime::spawn(async move {
                    if let Ok(probed) = probe_media_url(&mgr.core.http_client, &u_c).await {
                        if probed.is_media {
                            mgr.emit_probed_media(&app_c, &tid_c, probed);
                        }
                    }
                });
            }
        }

        let is_blank = target_url == "about:blank";
        let parsed = Url::parse(&target_url)?;
        if let Some(wv) = app.get_webview(&resolved_id) {
            wv.navigate(parsed)?;
        }
        {
            if let Some(t) = self.tabs.write().get_mut(&resolved_id) {
                t.url = target_url;
                if is_blank {
                    t.title = "New Tab".to_string();
                    t.is_loading = false;
                } else {
                    t.is_loading = true;
                }
            }
        }
        self.persist_session_and_emit(app);
        self.tabs
            .read()
            .get(&resolved_id)
            .cloned()
            .context("Tab missing after navigate")
    }

    pub fn go_back(&self, app: &AppHandle, tab_id: Option<&str>) -> Result<()> {
        let Some(id) = self.resolve_target_tab_id(tab_id) else {
            return Ok(());
        };
        if let Some(wv) = app.get_webview(&id) {
            #[cfg(windows)]
            {
                let _ = wv.with_webview(|platform_wv| unsafe {
                    if let Ok(core) = platform_wv.controller().CoreWebView2() {
                        let _ = core.GoBack();
                    }
                });
            }
            #[cfg(not(windows))]
            {
                let _ = wv.eval("window.history.back();");
            }
        }
        Ok(())
    }

    pub fn go_forward(&self, app: &AppHandle, tab_id: Option<&str>) -> Result<()> {
        let Some(id) = self.resolve_target_tab_id(tab_id) else {
            return Ok(());
        };
        if let Some(wv) = app.get_webview(&id) {
            #[cfg(windows)]
            {
                let _ = wv.with_webview(|platform_wv| unsafe {
                    if let Ok(core) = platform_wv.controller().CoreWebView2() {
                        let _ = core.GoForward();
                    }
                });
            }
            #[cfg(not(windows))]
            {
                let _ = wv.eval("window.history.forward();");
            }
        }
        Ok(())
    }

    pub fn reload(&self, app: &AppHandle, tab_id: Option<&str>) -> Result<()> {
        let Some(id) = self.resolve_target_tab_id(tab_id) else {
            return Ok(());
        };
        if let Some(wv) = app.get_webview(&id) {
            wv.reload()?;
        }
        Ok(())
    }

    pub fn set_viewport(&self, app: &AppHandle, bounds: ViewportBounds) {
        *self.viewport.write() = bounds;
        self.apply_viewport_to_all(app);
    }

    pub fn apply_viewport_to_all(&self, app: &AppHandle) {
        let vp = *self.viewport.read();
        let active_id = self.active_tab_id.read().clone();
        let ids = self.tab_order.read().clone();

        for id in ids {
            if let Some(wv) = app.get_webview(&id) {
                let is_active = active_id.as_deref() == Some(id.as_str());
                if vp.visible && is_active && vp.width > 10.0 && vp.height > 10.0 {
                    let _ = wv.set_bounds(Rect {
                        position: LogicalPosition::new(vp.x, vp.y).into(),
                        size: LogicalSize::new(vp.width, vp.height).into(),
                    });
                    let _ = wv.show();
                    #[cfg(windows)]
                    {
                        let _ = wv.with_webview(|platform_wv| unsafe {
                            #[link(name = "user32")]
                            unsafe extern "system" {
                                fn SetWindowPos(
                                    hwnd: *mut core::ffi::c_void,
                                    hwnd_insert_after: *mut core::ffi::c_void,
                                    x: i32,
                                    y: i32,
                                    cx: i32,
                                    cy: i32,
                                    u_flags: u32,
                                ) -> i32;
                            }
                            const SWP_NOSIZE: u32 = 0x0001;
                            const SWP_NOMOVE: u32 = 0x0002;
                            const SWP_NOACTIVATE: u32 = 0x0010;
                            let controller = platform_wv.controller();
                            let mut child_hwnd = Default::default();
                            if controller.ParentWindow(&mut child_hwnd).is_ok()
                                && !child_hwnd.0.is_null()
                            {
                                // HWND_TOP == 0 (null_mut): guarantees active tab-* child HWND
                                // sits above the "main" shell webview child HWND in Win32 Z-order.
                                let _ = SetWindowPos(
                                    child_hwnd.0,
                                    core::ptr::null_mut(),
                                    0,
                                    0,
                                    0,
                                    0,
                                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                                );
                            }
                        });
                    }
                } else {
                    let _ = wv.hide();
                }
            }
        }
    }

    /// Broadcasts the currently playing media URL & playback mode (`in_app` | `external`)
    /// to ALL open browser tabs so matching links are highlighted live on webpages!
    pub fn broadcast_active_media_highlight(&self, app: &AppHandle) {
        let ids = self.tab_order.read().clone();
        for id in ids {
            self.push_active_media_to_tab(app, &id);
        }
        let hl = self.core.active_highlight.read().clone();
        let _ = app.emit("sv://active-media-changed", hl);
    }

    fn push_active_media_to_tab(&self, app: &AppHandle, tab_id: &str) {
        let Some(wv) = app.get_webview(tab_id) else {
            return;
        };
        let hl = self.core.active_highlight.read().clone();
        let norm_json = serde_json::to_string(&hl.normalized_key).unwrap_or_else(|_| "null".to_string());
        let mode_json = serde_json::to_string(&hl.mode).unwrap_or_else(|_| "null".to_string());
        let js = format!(
            "if (typeof window.__SV_SET_ACTIVE_MEDIA__ === 'function') {{ window.__SV_SET_ACTIVE_MEDIA__({norm_json}, {mode_json}); }}"
        );
        let _ = wv.eval(&js);
    }

    fn resolve_target_tab_id(&self, tab_id: Option<&str>) -> Option<String> {
        match tab_id {
            Some(id) if self.tabs.read().contains_key(id) => Some(id.to_string()),
            _ => self.active_tab_id.read().clone(),
        }
    }

    fn update_tab_url(&self, app: &AppHandle, tab_id: &str, new_url: &str) {
        if let Some(t) = self.tabs.write().get_mut(tab_id) {
            t.url = new_url.to_string();
        }
        let _ = app.emit("sv://tabs-updated", self.list_tabs());
    }

    fn on_tab_title_changed(&self, app: &AppHandle, tab_id: &str, wv: &Webview, new_title: String) {
        let current_url = wv
            .url()
            .map(|u| u.to_string())
            .unwrap_or_default();
        let allowed_media_url = self.allow_media_in_tab.read().get(tab_id).cloned();

        if current_url == "about:blank" {
            if allowed_media_url.is_some() {
                // Keep direct-media tab URL & filename title intact when WebView2 falls back on .mkv
                return;
            }
            if let Some(t) = self.tabs.write().get_mut(tab_id) {
                t.title = "New Tab".to_string();
                t.url = "about:blank".to_string();
                t.is_loading = false;
            }
            self.persist_session_and_emit(app);
            return;
        }

        {
            if let Some(t) = self.tabs.write().get_mut(tab_id) {
                if !new_title.trim().is_empty() && new_title.trim() != "about:blank" {
                    t.title = new_title.clone();
                }
                if !current_url.is_empty() {
                    t.url = current_url.clone();
                }
            }
        }
        if !current_url.is_empty()
            && current_url != "about:blank"
            && !new_title.trim().is_empty()
            && new_title.trim() != "about:blank"
        {
            let _ = self.core.db.record_history_visit(&new_title, &current_url);
        }
        self.persist_session_and_emit(app);
    }

    fn on_tab_page_load(
        &self,
        app: &AppHandle,
        tab_id: &str,
        wv: &Webview,
        url_str: &str,
        is_loading: bool,
    ) {
        let allowed_media_url = self.allow_media_in_tab.read().get(tab_id).cloned();
        let mut title_snapshot = String::new();
        {
            if let Some(t) = self.tabs.write().get_mut(tab_id) {
                t.is_loading = is_loading;
                if url_str == "about:blank" {
                    if let Some(ref m_url) = allowed_media_url {
                        t.url = m_url.clone();
                    } else {
                        t.url = "about:blank".to_string();
                        t.title = "New Tab".to_string();
                    }
                } else if !url_str.is_empty() {
                    t.url = url_str.to_string();
                }
                title_snapshot = t.title.clone();
            }
        }

        // Refresh native WebView2 back/forward capability state on Windows
        #[cfg(windows)]
        {
            let tabs_arc = self.tabs.clone();
            let tid = tab_id.to_string();
            let app_c = app.clone();
            let mgr_c = self.clone();
            let _ = wv.with_webview(move |platform_wv| unsafe {
                if let Ok(core) = platform_wv.controller().CoreWebView2() {
                    let mut back = windows_sys::Win32::Foundation::FALSE;
                    let mut fwd = windows_sys::Win32::Foundation::FALSE;
                    let _ = core.CanGoBack(&mut back as *mut _ as *mut _);
                    let _ = core.CanGoForward(&mut fwd as *mut _ as *mut _);
                    if let Some(t) = tabs_arc.write().get_mut(&tid) {
                        t.can_go_back = back != 0;
                        t.can_go_forward = fwd != 0;
                    }
                    let _ = app_c.emit("sv://tabs-updated", mgr_c.list_tabs());
                }
            });
        }

        if !is_loading {
            if !url_str.is_empty() && url_str != "about:blank" {
                let _ = self.core.db.record_history_visit(&title_snapshot, url_str);
            }
            self.push_active_media_to_tab(app, tab_id);
        }
        self.persist_session_and_emit(app);
    }

    fn render_direct_media_tab_fallback(
        &self,
        app: &AppHandle,
        tab_id: &str,
        wv: &Webview,
        media_url: &str,
    ) {
        let quick = detect_media_from_url_only(media_url);
        let mut display_title = if !quick.suggested_filename.trim().is_empty() {
            quick.suggested_filename.clone()
        } else {
            media_url.to_string()
        };

        {
            if let Some(t) = self.tabs.write().get_mut(tab_id) {
                t.url = media_url.to_string();
                t.is_loading = false;
                if !t.title.trim().is_empty()
                    && t.title != "New Tab"
                    && !media_url.contains(&format!("://{}/", t.title))
                {
                    display_title = t.title.clone();
                } else {
                    t.title = display_title.clone();
                }
            }
        }

        let url_json = serde_json::to_string(media_url).unwrap_or_else(|_| "\"\"".to_string());
        let title_json =
            serde_json::to_string(&display_title).unwrap_or_else(|_| "\"Media Stream\"".to_string());
        let js = format!(
            "if (typeof window.__SV_RENDER_DIRECT_MEDIA_TAB__ === 'function') {{ window.__SV_RENDER_DIRECT_MEDIA_TAB__({url_json}, {title_json}); }}"
        );
        let _ = wv.eval(&js);
        self.persist_session_and_emit(app);
    }

    pub fn dispatch_media_detected(&self, app: AppHandle, tab_id: String, media_url: String) {
        let mgr = self.clone();
        tauri::async_runtime::spawn(async move {
            let det = probe_media_url(&mgr.core.http_client, &media_url)
                .await
                .unwrap_or_else(|_| detect_media_from_url_only(&media_url));
            mgr.emit_probed_media(&app, &tab_id, det);
        });
    }

    fn dispatch_media_detected_ask_only(&self, app: AppHandle, tab_id: String, media_url: String) {
        let mgr = self.clone();
        tauri::async_runtime::spawn(async move {
            let det = probe_media_url(&mgr.core.http_client, &media_url)
                .await
                .unwrap_or_else(|_| detect_media_from_url_only(&media_url));
            let payload = MediaDetectedPayload {
                tab_id,
                detection: det,
                preferred_method: "ask".to_string(),
            };
            let _ = app.emit("sv://media-detected", payload);
        });
    }

    fn dispatch_media_or_download_intercept(&self, app: AppHandle, tab_id: String, url_str: String) {
        let mgr = self.clone();
        tauri::async_runtime::spawn(async move {
            let det = probe_media_url(&mgr.core.http_client, &url_str)
                .await
                .unwrap_or_else(|_| detect_media_from_url_only(&url_str));
            if det.is_media {
                mgr.emit_probed_media(&app, &tab_id, det);
            } else {
                // Non-media file download initiated from webpage -> route to StreamVault Save As Download Manager
                let _ = app.emit(
                    "sv://non-media-download-requested",
                    serde_json::json!({ "url": url_str, "suggested_filename": det.suggested_filename }),
                );
            }
        });
    }

    fn emit_probed_media(&self, app: &AppHandle, tab_id: &str, detection: MediaDetectionResult) {
        let preferred = self
            .core
            .db
            .get_settings()
            .map(|s| s.default_playback_method)
            .unwrap_or_else(|_| "ask".to_string());
        let payload = MediaDetectedPayload {
            tab_id: tab_id.to_string(),
            detection,
            preferred_method: preferred,
        };
        let _ = app.emit("sv://media-detected", payload);
    }

    fn persist_session_and_emit(&self, app: &AppHandle) {
        let list = self.list_tabs();
        let saved: Vec<SavedSessionTab> = list
            .iter()
            .enumerate()
            .map(|(idx, t)| SavedSessionTab {
                tab_order: idx as i64,
                title: t.title.clone(),
                url: t.url.clone(),
                is_active: t.is_active,
            })
            .collect();
        let _ = self.core.db.save_browser_session(&saved);
        let _ = app.emit("sv://tabs-updated", list);
    }
}

fn looks_like_html_route(url: &str) -> bool {
    if let Ok(u) = Url::parse(url) {
        let p = u.path().to_ascii_lowercase();
        if p.is_empty() || p == "/" || p.ends_with('/') || p.ends_with(".html") || p.ends_with(".php") {
            return true;
        }
    }
    false
}
