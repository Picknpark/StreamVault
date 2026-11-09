use crate::logging::StructuredLogger;
use crate::portable::PortablePaths;
use anyhow::{Context, Result};
use libloading::{Library, Symbol};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use std::ffi::{c_char, c_double, c_int, c_ulong, c_void, CStr, CString};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[allow(non_camel_case_types)]
type mpv_handle = c_void;

#[allow(dead_code)]
const MPV_FORMAT_STRING: c_int = 1;
const MPV_FORMAT_FLAG: c_int = 3;
const MPV_FORMAT_INT64: c_int = 4;
const MPV_FORMAT_DOUBLE: c_int = 5;

const MPV_EVENT_NONE: c_int = 0;
const MPV_EVENT_SHUTDOWN: c_int = 1;
const MPV_EVENT_END_FILE: c_int = 7;
const MPV_EVENT_FILE_LOADED: c_int = 8;
const MPV_EVENT_PROPERTY_CHANGE: c_int = 22;

#[repr(C)]
struct MpvEvent {
    event_id: c_int,
    error: c_int,
    reply_userdata: u64,
    data: *mut c_void,
}

#[repr(C)]
struct MpvEventProperty {
    name: *const c_char,
    format: c_int,
    data: *mut c_void,
}

#[repr(C)]
struct MpvEventEndFile {
    reason: c_int,
    error: c_int,
    playlist_entry_id: i64,
}

#[allow(dead_code)]
struct MpvVtable {
    _lib: Library,
    client_api_version: unsafe extern "C" fn() -> c_ulong,
    error_string: unsafe extern "C" fn(c_int) -> *const c_char,
    free_fn: unsafe extern "C" fn(*mut c_void),
    create: unsafe extern "C" fn() -> *mut mpv_handle,
    initialize: unsafe extern "C" fn(*mut mpv_handle) -> c_int,
    terminate_destroy: unsafe extern "C" fn(*mut mpv_handle),
    set_option: unsafe extern "C" fn(*mut mpv_handle, *const c_char, c_int, *mut c_void) -> c_int,
    set_option_string: unsafe extern "C" fn(*mut mpv_handle, *const c_char, *const c_char) -> c_int,
    command: unsafe extern "C" fn(*mut mpv_handle, *const *const c_char) -> c_int,
    set_property: unsafe extern "C" fn(*mut mpv_handle, *const c_char, c_int, *mut c_void) -> c_int,
    set_property_string:
        unsafe extern "C" fn(*mut mpv_handle, *const c_char, *const c_char) -> c_int,
    get_property: unsafe extern "C" fn(*mut mpv_handle, *const c_char, c_int, *mut c_void) -> c_int,
    get_property_string: unsafe extern "C" fn(*mut mpv_handle, *const c_char) -> *mut c_char,
    observe_property: unsafe extern "C" fn(*mut mpv_handle, u64, *const c_char, c_int) -> c_int,
    wait_event: unsafe extern "C" fn(*mut mpv_handle, c_double) -> *mut MpvEvent,
}

unsafe impl Send for MpvVtable {}
unsafe impl Sync for MpvVtable {}

#[cfg(windows)]
fn apply_media_engine_dll_search_dir(dll_path: &Path) {
    if let Some(dir) = dll_path.parent() {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::System::LibraryLoader::SetDllDirectoryW;
        let wide: Vec<u16> = dir
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        unsafe {
            let _ = SetDllDirectoryW(wide.as_ptr());
        }
    }
}

impl MpvVtable {
    fn load(dll_path: &Path) -> Result<Self> {
        unsafe {
            // On Windows, add `runtime/media-engine` after System32 in the DLL search path
            // so real GPU drivers in System32 win first while GPU-less VMs can resolve fallback stubs.
            #[cfg(windows)]
            apply_media_engine_dll_search_dir(dll_path);

            let lib = Library::new(dll_path).with_context(|| {
                format!("Failed loading libmpv shared library from {}", dll_path.display())
            })?;

            let client_api_version = *lib.get::<unsafe extern "C" fn() -> c_ulong>(b"mpv_client_api_version\0")?;
            let error_string = *lib.get::<unsafe extern "C" fn(c_int) -> *const c_char>(b"mpv_error_string\0")?;
            let free_fn = *lib.get::<unsafe extern "C" fn(*mut c_void)>(b"mpv_free\0")?;
            let create = *lib.get::<unsafe extern "C" fn() -> *mut mpv_handle>(b"mpv_create\0")?;
            let initialize = *lib.get::<unsafe extern "C" fn(*mut mpv_handle) -> c_int>(b"mpv_initialize\0")?;
            let terminate_destroy =
                *lib.get::<unsafe extern "C" fn(*mut mpv_handle)>(b"mpv_terminate_destroy\0")?;
            let set_option = *lib.get::<unsafe extern "C" fn(
                *mut mpv_handle,
                *const c_char,
                c_int,
                *mut c_void,
            ) -> c_int>(b"mpv_set_option\0")?;
            let set_option_string = *lib.get::<unsafe extern "C" fn(
                *mut mpv_handle,
                *const c_char,
                *const c_char,
            ) -> c_int>(b"mpv_set_option_string\0")?;
            let command =
                *lib.get::<unsafe extern "C" fn(*mut mpv_handle, *const *const c_char) -> c_int>(
                    b"mpv_command\0",
                )?;
            let set_property = *lib.get::<unsafe extern "C" fn(
                *mut mpv_handle,
                *const c_char,
                c_int,
                *mut c_void,
            ) -> c_int>(b"mpv_set_property\0")?;
            let set_property_string = *lib.get::<unsafe extern "C" fn(
                *mut mpv_handle,
                *const c_char,
                *const c_char,
            ) -> c_int>(b"mpv_set_property_string\0")?;
            let get_property = *lib.get::<unsafe extern "C" fn(
                *mut mpv_handle,
                *const c_char,
                c_int,
                *mut c_void,
            ) -> c_int>(b"mpv_get_property\0")?;
            let get_property_string = *lib
                .get::<unsafe extern "C" fn(*mut mpv_handle, *const c_char) -> *mut c_char>(
                    b"mpv_get_property_string\0",
                )?;
            let observe_property = *lib.get::<unsafe extern "C" fn(
                *mut mpv_handle,
                u64,
                *const c_char,
                c_int,
            ) -> c_int>(b"mpv_observe_property\0")?;
            let wait_event = *lib
                .get::<unsafe extern "C" fn(*mut mpv_handle, c_double) -> *mut MpvEvent>(
                    b"mpv_wait_event\0",
                )?;

            Ok(Self {
                _lib: lib,
                client_api_version,
                error_string,
                free_fn,
                create,
                initialize,
                terminate_destroy,
                set_option,
                set_option_string,
                command,
                set_property,
                set_property_string,
                get_property,
                get_property_string,
                observe_property,
                wait_event,
            })
        }
    }

    fn err_text(&self, code: c_int) -> String {
        unsafe {
            let ptr = (self.error_string)(code);
            if ptr.is_null() {
                format!("mpv error {code}")
            } else {
                CStr::from_ptr(ptr).to_string_lossy().to_string()
            }
        }
    }
}

/// Probes `libmpv-2.dll` client API version when running on the target OS.
pub fn probe_libmpv_version(dll_path: &Path) -> Result<String> {
    #[cfg(windows)]
    apply_media_engine_dll_search_dir(dll_path);
    let lib = unsafe { Library::new(dll_path)? };
    let sym: Symbol<unsafe extern "C" fn() -> c_ulong> =
        unsafe { lib.get(b"mpv_client_api_version\0")? };
    let ver = unsafe { sym() };
    let major = ver >> 16;
    let minor = ver & 0xFFFF;
    Ok(format!("libmpv C API v{major}.{minor}"))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaTrack {
    pub id: i64,
    #[serde(rename = "type")]
    pub track_type: String, // "video" | "audio" | "sub"
    pub title: Option<String>,
    pub lang: Option<String>,
    pub codec: Option<String>,
    pub selected: bool,
    #[serde(default)]
    pub external: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerStateSnapshot {
    pub engine_ready: bool,
    pub engine_backend: String, // "libmpv" | "unavailable"
    pub active: bool,
    pub paused: bool,
    pub original_url: Option<String>,
    pub stream_url: Option<String>,
    pub title: Option<String>,
    pub position_ms: i64,
    pub duration_ms: i64,
    pub volume: i64,
    pub muted: bool,
    pub speed: f64,
    pub aspect_ratio: String,
    pub fullscreen: bool,
    pub pip_active: bool,
    pub tracks: Vec<MediaTrack>,
    pub audio_track_id: Option<i64>,
    pub subtitle_track_id: Option<i64>,
    pub subtitles_visible: bool,
    pub video_width: i64,
    pub video_height: i64,
    pub buffering: bool,
    pub eof_reached: bool,
    pub last_error: Option<String>,
}

impl Default for PlayerStateSnapshot {
    fn default() -> Self {
        Self {
            engine_ready: false,
            engine_backend: "unavailable".to_string(),
            active: false,
            paused: true,
            original_url: None,
            stream_url: None,
            title: None,
            position_ms: 0,
            duration_ms: 0,
            volume: 100,
            muted: false,
            speed: 1.0,
            aspect_ratio: "auto".to_string(),
            fullscreen: false,
            pip_active: false,
            tracks: Vec::new(),
            audio_track_id: None,
            subtitle_track_id: None,
            subtitles_visible: true,
            video_width: 0,
            video_height: 0,
            buffering: false,
            eof_reached: false,
            last_error: None,
        }
    }
}

/// Events emitted from the Win32 video surface `WndProc` (mouse single/double click, wheel, move, keys)
/// or from `libmpv`'s asynchronous event loop.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SurfaceInputEvent {
    SingleClick,
    DoubleClick,
    WheelDelta { notches: i32 },
    MouseMove,
    KeyPress { vk_code: u32 },
    StateChanged,
    PlaybackEnded { reason: i32, error_message: Option<String> },
}

#[derive(Clone, Copy)]
struct RawMpvHandle(*mut mpv_handle);
unsafe impl Send for RawMpvHandle {}
unsafe impl Sync for RawMpvHandle {}

#[cfg(windows)]
mod win32_surface {
    use super::SurfaceInputEvent;
    use parking_lot::Mutex;
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, Ordering};
    use std::sync::Once;
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
    use windows_sys::Win32::Graphics::Gdi::{
        BeginPaint, ClientToScreen, CreateFontW, CreateRoundRectRgn, CreateSolidBrush,
        DeleteObject, DrawTextW, EndPaint, FillRect, FrameRect, GetStockObject, InvalidateRect,
        SelectObject, SetBkMode, SetTextColor, SetWindowRgn, BLACK_BRUSH, DT_CENTER,
        DT_SINGLELINE, DT_VCENTER, PAINTSTRUCT, TRANSPARENT,
    };
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    static REGISTER_ONCE: Once = Once::new();
    static SURFACE_CALLBACK: Mutex<Option<Box<dyn Fn(SurfaceInputEvent) + Send + Sync>>> =
        Mutex::new(None);
    static LAST_MOUSEMOVE_MS: AtomicU64 = AtomicU64::new(0);
    static PENDING_SINGLE_CLICK: AtomicBool = AtomicBool::new(false);
    static TIP_RENDER_STATE: Mutex<(String, f64)> = Mutex::new((String::new(), 1.0));

    pub struct Win32VideoHost {
        parent_hwnd: AtomicIsize,
        video_hwnd: AtomicIsize,
        pip_hwnd: AtomicIsize,
        tip_hwnd: AtomicIsize,
        pip_mode: AtomicBool,
        tip_visible: AtomicBool,
        tip_size: Mutex<(i32, i32)>,
        applied_viewport: Mutex<(bool, i32, i32, i32, i32)>,
    }

    impl Win32VideoHost {
        pub fn new(
            parent_hwnd: isize,
            callback: Box<dyn Fn(SurfaceInputEvent) + Send + Sync>,
        ) -> anyhow::Result<Self> {
            *SURFACE_CALLBACK.lock() = Some(callback);

            let class_name: Vec<u16> = OsStr::new("StreamVaultMpvSurface")
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();
            let pip_class_name: Vec<u16> = OsStr::new("StreamVaultPipHost")
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();
            let tip_class_name: Vec<u16> = OsStr::new("StreamVaultTimeTip")
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();

            unsafe {
                let hinstance = GetModuleHandleW(std::ptr::null());
                REGISTER_ONCE.call_once(|| {
                    // Omit CS_HREDRAW | CS_VREDRAW and set null hbrBackground so Windows GDI
                    // never erases/flashes the D3D11 libmpv video surface during fullscreen resize!
                    let wc = WNDCLASSEXW {
                        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                        style: CS_DBLCLKS,
                        lpfnWndProc: Some(video_surface_wndproc),
                        cbClsExtra: 0,
                        cbWndExtra: 0,
                        hInstance: hinstance,
                        hIcon: std::ptr::null_mut(),
                        hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
                        hbrBackground: std::ptr::null_mut(),
                        lpszMenuName: std::ptr::null(),
                        lpszClassName: class_name.as_ptr(),
                        hIconSm: std::ptr::null_mut(),
                    };
                    RegisterClassExW(&wc);

                    let pip_wc = WNDCLASSEXW {
                        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                        style: 0,
                        lpfnWndProc: Some(pip_host_wndproc),
                        cbClsExtra: 0,
                        cbWndExtra: 0,
                        hInstance: hinstance,
                        hIcon: std::ptr::null_mut(),
                        hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
                        hbrBackground: GetStockObject(BLACK_BRUSH),
                        lpszMenuName: std::ptr::null(),
                        lpszClassName: pip_class_name.as_ptr(),
                        hIconSm: std::ptr::null_mut(),
                    };
                    RegisterClassExW(&pip_wc);

                    let tip_wc = WNDCLASSEXW {
                        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                        style: 0,
                        lpfnWndProc: Some(time_tip_wndproc),
                        cbClsExtra: 0,
                        cbWndExtra: 0,
                        hInstance: hinstance,
                        hIcon: std::ptr::null_mut(),
                        hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
                        hbrBackground: std::ptr::null_mut(),
                        lpszMenuName: std::ptr::null(),
                        lpszClassName: tip_class_name.as_ptr(),
                        hIconSm: std::ptr::null_mut(),
                    };
                    RegisterClassExW(&tip_wc);
                });

                let video_hwnd = CreateWindowExW(
                    0,
                    class_name.as_ptr(),
                    std::ptr::null(),
                    WS_CHILD | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
                    0,
                    0,
                    16,
                    16,
                    parent_hwnd as HWND,
                    std::ptr::null_mut(),
                    hinstance,
                    std::ptr::null(),
                );
                if video_hwnd.is_null() {
                    anyhow::bail!("Failed creating Win32 child HWND for libmpv video surface");
                }

                let pip_title: Vec<u16> = OsStr::new("StreamVault — Picture in Picture")
                    .encode_wide()
                    .chain(std::iter::once(0))
                    .collect();
                let pip_hwnd = CreateWindowExW(
                    WS_EX_TOPMOST | WS_EX_APPWINDOW,
                    pip_class_name.as_ptr(),
                    pip_title.as_ptr(),
                    WS_OVERLAPPEDWINDOW & !WS_MAXIMIZEBOX,
                    80,
                    80,
                    512,
                    310,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    hinstance,
                    std::ptr::null(),
                );

                // Native click-through popup owned by `parent_hwnd` (`main_hwnd`) so the timeline
                // hover time tooltip ALWAYS floats cleanly OVER the libmpv D3D11 video frame!
                let tip_hwnd = CreateWindowExW(
                    WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TRANSPARENT,
                    tip_class_name.as_ptr(),
                    std::ptr::null(),
                    WS_POPUP | WS_DISABLED,
                    0,
                    0,
                    64,
                    24,
                    parent_hwnd as HWND,
                    std::ptr::null_mut(),
                    hinstance,
                    std::ptr::null(),
                );

                Ok(Self {
                    parent_hwnd: AtomicIsize::new(parent_hwnd),
                    video_hwnd: AtomicIsize::new(video_hwnd as isize),
                    pip_hwnd: AtomicIsize::new(pip_hwnd as isize),
                    tip_hwnd: AtomicIsize::new(tip_hwnd as isize),
                    pip_mode: AtomicBool::new(false),
                    tip_visible: AtomicBool::new(false),
                    tip_size: Mutex::new((0, 0)),
                    applied_viewport: Mutex::new((false, 0, 0, 0, 0)),
                })
            }
        }

        pub fn video_hwnd_i64(&self) -> i64 {
            self.video_hwnd.load(Ordering::SeqCst) as i64
        }

        pub fn set_viewport(&self, visible: bool, x: i32, y: i32, width: i32, height: i32) {
            if self.pip_mode.load(Ordering::SeqCst) {
                return;
            }
            let hwnd = self.video_hwnd.load(Ordering::SeqCst) as HWND;
            let main_hwnd = self.parent_hwnd.load(Ordering::SeqCst) as HWND;
            if hwnd.is_null() {
                return;
            }
            unsafe {
                let should_show = visible && width > 2 && height > 2;
                if should_show {
                    // Prevent Windows 10/11 DWM from promoting `video_hwnd`'s d3d11-flip swapchain
                    // into full-monitor DirectFlip / Independent Flip when fullscreen HUD hides
                    // (which flashes the screen alongside WebView2's DirectComposition tree):
                    // when `video_hwnd` fills the full client rect at (0, 0), inset height by 1px
                    // over the #000000 black background.
                    let mut eff_h = height;
                    if x == 0 && y == 0 && !main_hwnd.is_null() {
                        let mut crc: RECT = std::mem::zeroed();
                        if GetClientRect(main_hwnd, &mut crc) != 0 {
                            let cw = crc.right - crc.left;
                            let ch = crc.bottom - crc.top;
                            if width >= cw && height >= ch && height > 16 {
                                eff_h = height - 1;
                            }
                        }
                    }

                    let target = (true, x, y, width, eff_h);
                    let is_already_top_and_visible = !main_hwnd.is_null()
                        && IsWindowVisible(hwnd) != 0
                        && GetTopWindow(main_hwnd) == hwnd;

                    {
                        let mut applied = self.applied_viewport.lock();
                        if *applied == target && is_already_top_and_visible {
                            return;
                        }
                        *applied = target;
                    }

                    SetWindowPos(
                        hwnd,
                        HWND_TOP,
                        x,
                        y,
                        width,
                        eff_h,
                        SWP_NOACTIVATE | SWP_SHOWWINDOW,
                    );
                } else {
                    *self.applied_viewport.lock() = (false, 0, 0, 0, 0);
                    self.hide_time_tooltip();
                    if IsWindowVisible(hwnd) != 0 {
                        ShowWindow(hwnd, SW_HIDE);
                    }
                }
            }
        }

        pub fn hide_time_tooltip(&self) {
            let tip = self.tip_hwnd.load(Ordering::SeqCst) as HWND;
            if !tip.is_null() && self.tip_visible.swap(false, Ordering::SeqCst) {
                unsafe {
                    ShowWindow(tip, SW_HIDE);
                }
            }
        }

        pub fn set_time_tooltip(
            &self,
            visible: bool,
            client_x: i32,
            client_y: i32,
            scale: f64,
            text: &str,
        ) {
            if !visible || text.is_empty() || self.pip_mode.load(Ordering::SeqCst) {
                self.hide_time_tooltip();
                return;
            }
            let tip = self.tip_hwnd.load(Ordering::SeqCst) as HWND;
            let main_hwnd = self.parent_hwnd.load(Ordering::SeqCst) as HWND;
            if tip.is_null() || main_hwnd.is_null() {
                return;
            }

            let dpi_scale = scale.clamp(1.0, 4.0);
            let char_count = text.chars().count().max(5);
            let width = (((char_count as f64) * 8.2 + 18.0) * dpi_scale).round() as i32;
            let height = (24.0 * dpi_scale).round() as i32;
            let gap = (6.0 * dpi_scale).round() as i32;

            {
                let mut st = TIP_RENDER_STATE.lock();
                st.0.clear();
                st.0.push_str(text);
                st.1 = dpi_scale;
            }

            unsafe {
                let mut pt = POINT {
                    x: client_x,
                    y: client_y,
                };
                let _ = ClientToScreen(main_hwnd, &mut pt);

                let mut client_rc: RECT = std::mem::zeroed();
                let _ = GetClientRect(main_hwnd, &mut client_rc);
                let mut top_left = POINT { x: 0, y: 0 };
                let _ = ClientToScreen(main_hwnd, &mut top_left);
                let min_x = top_left.x + (6.0 * dpi_scale).round() as i32;
                let max_x = (top_left.x + (client_rc.right - client_rc.left) - width
                    - (6.0 * dpi_scale).round() as i32)
                    .max(min_x);

                let screen_x = (pt.x - width / 2).clamp(min_x, max_x);
                let screen_y = pt.y - height - gap;

                {
                    let mut prev_sz = self.tip_size.lock();
                    if *prev_sz != (width, height) {
                        *prev_sz = (width, height);
                        let radius = (6.0 * dpi_scale).round() as i32;
                        let rgn = CreateRoundRectRgn(0, 0, width + 1, height + 1, radius, radius);
                        if !rgn.is_null() {
                            let _ = SetWindowRgn(tip, rgn, 0);
                        }
                    }
                }

                let already_vis = self.tip_visible.swap(true, Ordering::SeqCst);
                let flags = if already_vis {
                    SWP_NOACTIVATE
                } else {
                    SWP_NOACTIVATE | SWP_SHOWWINDOW
                };
                SetWindowPos(tip, HWND_TOP, screen_x, screen_y, width, height, flags);
                let _ = InvalidateRect(tip, std::ptr::null(), 0);
            }
        }

        pub fn set_pip_mode(&self, enable: bool, restore_rect: Option<(i32, i32, i32, i32)>) -> bool {
            let v_hwnd = self.video_hwnd.load(Ordering::SeqCst) as HWND;
            let p_hwnd = self.pip_hwnd.load(Ordering::SeqCst) as HWND;
            let main_hwnd = self.parent_hwnd.load(Ordering::SeqCst) as HWND;
            if v_hwnd.is_null() || p_hwnd.is_null() || main_hwnd.is_null() {
                return false;
            }

            unsafe {
                if enable {
                    self.hide_time_tooltip();
                    self.pip_mode.store(true, Ordering::SeqCst);
                    ShowWindow(p_hwnd, SW_SHOW);
                    let mut rc: RECT = std::mem::zeroed();
                    GetClientRect(p_hwnd, &mut rc);
                    let w = (rc.right - rc.left).max(320);
                    let h = (rc.bottom - rc.top).max(180);
                    SetParent(v_hwnd, p_hwnd);
                    SetWindowPos(v_hwnd, HWND_TOP, 0, 0, w, h, SWP_SHOWWINDOW);
                    true
                } else {
                    self.pip_mode.store(false, Ordering::SeqCst);
                    SetParent(v_hwnd, main_hwnd);
                    ShowWindow(p_hwnd, SW_HIDE);
                    if let Some((x, y, w, h)) = restore_rect {
                        *self.applied_viewport.lock() = (true, x, y, w, h);
                        SetWindowPos(
                            v_hwnd,
                            HWND_TOP,
                            x,
                            y,
                            w,
                            h,
                            SWP_NOACTIVATE | SWP_SHOWWINDOW,
                        );
                    } else {
                        *self.applied_viewport.lock() = (false, 0, 0, 0, 0);
                    }
                    false
                }
            }
        }

        pub fn destroy(&self) {
            unsafe {
                let t = self.tip_hwnd.swap(0, Ordering::SeqCst) as HWND;
                let v = self.video_hwnd.swap(0, Ordering::SeqCst) as HWND;
                let p = self.pip_hwnd.swap(0, Ordering::SeqCst) as HWND;
                if !t.is_null() {
                    DestroyWindow(t);
                }
                if !v.is_null() {
                    DestroyWindow(v);
                }
                if !p.is_null() {
                    DestroyWindow(p);
                }
            }
        }
    }

    fn emit_surface(ev: SurfaceInputEvent) {
        if let Some(ref cb) = *SURFACE_CALLBACK.lock() {
            cb(ev);
        }
    }

    unsafe extern "system" fn video_surface_wndproc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            // Prevent Windows GDI from clearing/flashing the D3D11 video surface in fullscreen!
            WM_ERASEBKGND => 1,
            WM_LBUTTONDOWN => {
                SetFocus(hwnd);
                PENDING_SINGLE_CLICK.store(true, Ordering::SeqCst);
                std::thread::spawn(|| {
                    std::thread::sleep(Duration::from_millis(220));
                    if PENDING_SINGLE_CLICK.swap(false, Ordering::SeqCst) {
                        emit_surface(SurfaceInputEvent::SingleClick);
                    }
                });
                0
            }
            WM_LBUTTONDBLCLK => {
                PENDING_SINGLE_CLICK.store(false, Ordering::SeqCst);
                emit_surface(SurfaceInputEvent::DoubleClick);
                0
            }
            WM_MOUSEWHEEL => {
                let delta = ((wparam >> 16) as i16) as i32;
                let notches = delta / 120;
                if notches != 0 {
                    emit_surface(SurfaceInputEvent::WheelDelta { notches });
                }
                0
            }
            WM_MOUSEMOVE => {
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                let prev = LAST_MOUSEMOVE_MS.load(Ordering::Relaxed);
                if now_ms.saturating_sub(prev) >= 150 {
                    LAST_MOUSEMOVE_MS.store(now_ms, Ordering::Relaxed);
                    emit_surface(SurfaceInputEvent::MouseMove);
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            WM_KEYDOWN => {
                emit_surface(SurfaceInputEvent::KeyPress {
                    vk_code: wparam as u32,
                });
                0
            }
            _ => {
                let _ = Instant::now();
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
        }
    }

    unsafe extern "system" fn time_tip_wndproc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_ERASEBKGND => 1,
            WM_NCHITTEST => -1, // HTTRANSPARENT — clicks/hovers pass straight through to timeline
            WM_PAINT => {
                let mut ps: PAINTSTRUCT = std::mem::zeroed();
                let hdc = BeginPaint(hwnd, &mut ps);
                if !hdc.is_null() {
                    let mut rc: RECT = std::mem::zeroed();
                    let _ = GetClientRect(hwnd, &mut rc);

                    // Dark pill background (#1e2433 -> COLORREF 0x00BBGGRR = 0x0033241e)
                    let bg_brush = CreateSolidBrush(0x0033241e);
                    if !bg_brush.is_null() {
                        FillRect(hdc, &rc, bg_brush);
                        let _ = DeleteObject(bg_brush as _);
                    }

                    // Crisp 1px accent/slate border (#38bdf8 / #475569 -> 0x00695547)
                    let border_brush = CreateSolidBrush(0x00695547);
                    if !border_brush.is_null() {
                        FrameRect(hdc, &rc, border_brush);
                        let _ = DeleteObject(border_brush as _);
                    }

                    let (text, dpi_scale) = TIP_RENDER_STATE.lock().clone();
                    if !text.is_empty() {
                        let font_height = -((12.0 * dpi_scale).round() as i32);
                        let face: Vec<u16> = OsStr::new("Consolas")
                            .encode_wide()
                            .chain(std::iter::once(0))
                            .collect();
                        let font = CreateFontW(
                            font_height,
                            0,
                            0,
                            0,
                            600, // FW_SEMIBOLD
                            0,
                            0,
                            0,
                            1, // DEFAULT_CHARSET
                            0,
                            0,
                            5, // CLEARTYPE_QUALITY
                            0,
                            face.as_ptr(),
                        );
                        let old_font = if !font.is_null() {
                            SelectObject(hdc, font as _)
                        } else {
                            std::ptr::null_mut()
                        };

                        SetBkMode(hdc, TRANSPARENT as i32);
                        // Bright crisp white text (#f8fafc -> COLORREF 0x00fcfaf8)
                        SetTextColor(hdc, 0x00fcfaf8);

                        let wide_text: Vec<u16> = OsStr::new(&text).encode_wide().collect();
                        DrawTextW(
                            hdc,
                            wide_text.as_ptr(),
                            wide_text.len() as i32,
                            &mut rc,
                            DT_CENTER | DT_VCENTER | DT_SINGLELINE,
                        );

                        if !old_font.is_null() {
                            let _ = SelectObject(hdc, old_font);
                        }
                        if !font.is_null() {
                            let _ = DeleteObject(font as _);
                        }
                    }
                }
                let _ = EndPaint(hwnd, &ps);
                0
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }

    unsafe extern "system" fn pip_host_wndproc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_SIZE => {
                let width = (lparam & 0xFFFF) as i32;
                let height = ((lparam >> 16) & 0xFFFF) as i32;
                let child = GetWindow(hwnd, GW_CHILD);
                if !child.is_null() {
                    SetWindowPos(child, HWND_TOP, 0, 0, width, height, SWP_NOACTIVATE);
                }
                0
            }
            WM_CLOSE => {
                // Closing PiP window hides it and notifies controller instead of destroying video HWND!
                ShowWindow(hwnd, SW_HIDE);
                emit_surface(SurfaceInputEvent::KeyPress { vk_code: 0x1B });
                0
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

pub struct MpvPlayerController {
    paths: PortablePaths,
    logger: StructuredLogger,
    vtable: Option<Arc<MpvVtable>>,
    ctx: Mutex<Option<RawMpvHandle>>,
    state: Arc<RwLock<PlayerStateSnapshot>>,
    shutdown_flag: Arc<AtomicBool>,
    last_viewport: Mutex<(bool, i32, i32, i32, i32)>,
    #[cfg(windows)]
    win32_host: Mutex<Option<Arc<win32_surface::Win32VideoHost>>>,
}

impl MpvPlayerController {
    pub fn new(paths: PortablePaths, logger: StructuredLogger) -> Arc<Self> {
        let mut snap = PlayerStateSnapshot::default();
        let vtable = match paths.find_libmpv_library() {
            Some(ref dll_path) => match MpvVtable::load(dll_path) {
                Ok(vt) => {
                    let api_ver = unsafe { (vt.client_api_version)() };
                    logger.info_ctx(
                        "mpv_engine",
                        "Loaded libmpv media engine library",
                        serde_json::json!({
                            "path": dll_path.display().to_string(),
                            "api_version": format!("{}.{}", api_ver >> 16, api_ver & 0xFFFF)
                        }),
                    );
                    snap.engine_ready = true;
                    snap.engine_backend = "libmpv".to_string();
                    Some(Arc::new(vt))
                }
                Err(err) => {
                    logger.warn_ctx(
                        "mpv_engine",
                        "libmpv library found on disk (cross-host or load error)",
                        serde_json::json!({
                            "path": dll_path.display().to_string(),
                            "detail": err.to_string()
                        }),
                    );
                    None
                }
            },
            None => {
                logger.warn(
                    "mpv_engine",
                    "libmpv-2.dll not found in runtime/media-engine; place libmpv-2.dll in StreamVault/runtime/media-engine/",
                );
                None
            }
        };

        Arc::new(Self {
            paths,
            logger,
            vtable,
            ctx: Mutex::new(None),
            state: Arc::new(RwLock::new(snap)),
            shutdown_flag: Arc::new(AtomicBool::new(false)),
            last_viewport: Mutex::new((false, 0, 0, 0, 0)),
            #[cfg(windows)]
            win32_host: Mutex::new(None),
        })
    }

    /// Attaches the embedded Win32 video rendering surface to StreamVault's main window `HWND`
    /// and initializes the `libmpv` playback context with hardware-accelerated D3D11 output.
    pub fn attach_to_parent_window(
        self: &Arc<Self>,
        parent_hwnd: isize,
        event_callback: Arc<dyn Fn(SurfaceInputEvent) + Send + Sync + 'static>,
    ) -> Result<()> {
        let Some(ref vt) = self.vtable else {
            return Ok(());
        };

        if self.ctx.lock().is_some() {
            return Ok(());
        }

        #[cfg(windows)]
        let wid_opt: Option<i64> = {
            let cb_clone = event_callback.clone();
            let host = Arc::new(win32_surface::Win32VideoHost::new(
                parent_hwnd,
                Box::new(move |ev| cb_clone(ev)),
            )?);
            let wid = host.video_hwnd_i64();
            *self.win32_host.lock() = Some(host);
            Some(wid)
        };
        #[cfg(not(windows))]
        let wid_opt: Option<i64> = {
            let _ = parent_hwnd;
            None
        };

        unsafe {
            let handle = (vt.create)();
            if handle.is_null() {
                anyhow::bail!("mpv_create() returned null");
            }

            if let Some(mut wid) = wid_opt {
                let wid_name = CString::new("wid")?;
                (vt.set_option)(
                    handle,
                    wid_name.as_ptr(),
                    MPV_FORMAT_INT64,
                    &mut wid as *mut i64 as *mut c_void,
                );
            }

            // Configure professional desktop mpv engine options for Windows 10/11 + MKV/ASS/D3D11.
            // IMPORTANT: Keep `d3d11-flip=yes` (DXGI flip-model swapchain) so `libmpv` gets its own
            // DWM composition surface above WebView2's DirectComposition tree (`d3d11-flip=no` BitBlt
            // is occluded as a black screen by DirectComposition!). Fullscreen DirectFlip flicker is
            // prevented in `Win32VideoHost::set_viewport` + `d3d11-exclusive-fs=no`.
            for (k, v) in [
                ("vo", "gpu"),
                ("gpu-context", "d3d11"),
                ("d3d11-exclusive-fs", "no"),
                ("d3d11-flip", "yes"),
                ("hwdec", "auto-safe"),
                ("keep-open", "yes"),
                ("idle", "yes"),
                ("osc", "no"),
                ("osd-level", "1"),
                ("osd-bar", "no"),
                ("input-default-bindings", "no"),
                ("input-vo-keyboard", "no"),
                ("cache", "yes"),
                ("demuxer-max-bytes", "64MiB"),
                ("demuxer-max-back-bytes", "32MiB"),
                ("network-timeout", "20"),
                ("sub-auto", "fuzzy"),
                ("volume-max", "150"),
            ] {
                let ck = CString::new(k)?;
                let cv = CString::new(v)?;
                let _ = (vt.set_option_string)(handle, ck.as_ptr(), cv.as_ptr());
            }

            let init_rc = (vt.initialize)(handle);
            if init_rc < 0 {
                let msg = vt.err_text(init_rc);
                (vt.terminate_destroy)(handle);
                anyhow::bail!("mpv_initialize() failed: {msg}");
            }

            // Observe real-time playback properties
            for (idx, (prop, fmt)) in [
                ("time-pos", MPV_FORMAT_DOUBLE),
                ("duration", MPV_FORMAT_DOUBLE),
                ("pause", MPV_FORMAT_FLAG),
                ("volume", MPV_FORMAT_DOUBLE),
                ("mute", MPV_FORMAT_FLAG),
                ("speed", MPV_FORMAT_DOUBLE),
                ("eof-reached", MPV_FORMAT_FLAG),
                ("paused-for-cache", MPV_FORMAT_FLAG),
                ("aid", MPV_FORMAT_INT64),
                ("sid", MPV_FORMAT_INT64),
                ("sub-visibility", MPV_FORMAT_FLAG),
                ("width", MPV_FORMAT_INT64),
                ("height", MPV_FORMAT_INT64),
            ]
            .into_iter()
            .enumerate()
            {
                let cprop = CString::new(prop)?;
                let _ = (vt.observe_property)(handle, (idx + 1) as u64, cprop.as_ptr(), fmt);
            }

            *self.ctx.lock() = Some(RawMpvHandle(handle));
        }

        // Spawn dedicated background event loop thread pumping `mpv_wait_event`
        let controller = Arc::clone(self);
        let vt_clone = Arc::clone(vt);
        std::thread::Builder::new()
            .name("streamvault-mpv-events".to_string())
            .spawn(move || {
                controller.run_event_loop(vt_clone, event_callback);
            })?;

        Ok(())
    }

    fn run_event_loop(
        &self,
        vt: Arc<MpvVtable>,
        event_callback: Arc<dyn Fn(SurfaceInputEvent) + Send + Sync + 'static>,
    ) {
        while !self.shutdown_flag.load(Ordering::Relaxed) {
            let handle_ptr = match *self.ctx.lock() {
                Some(RawMpvHandle(p)) => p,
                None => break,
            };

            let ev_ptr = unsafe { (vt.wait_event)(handle_ptr, 0.25) };
            if ev_ptr.is_null() {
                continue;
            }
            let ev = unsafe { &*ev_ptr };
            match ev.event_id {
                MPV_EVENT_NONE => continue,
                MPV_EVENT_SHUTDOWN => break,
                MPV_EVENT_FILE_LOADED => {
                    self.refresh_tracks_internal(&vt, handle_ptr);
                    event_callback(SurfaceInputEvent::StateChanged);
                }
                MPV_EVENT_END_FILE => {
                    if !ev.data.is_null() {
                        let ef = unsafe { &*(ev.data as *const MpvEventEndFile) };
                        let err_msg = if ef.error < 0 {
                            Some(vt.err_text(ef.error))
                        } else {
                            None
                        };
                        if let Some(ref e) = err_msg {
                            self.state.write().last_error = Some(e.clone());
                        }
                        event_callback(SurfaceInputEvent::PlaybackEnded {
                            reason: ef.reason,
                            error_message: err_msg,
                        });
                    }
                }
                MPV_EVENT_PROPERTY_CHANGE => {
                    if !ev.data.is_null() {
                        let prop = unsafe { &*(ev.data as *const MpvEventProperty) };
                        if !prop.name.is_null() && !prop.data.is_null() {
                            let name = unsafe { CStr::from_ptr(prop.name).to_string_lossy() };
                            let mut st = self.state.write();
                            unsafe {
                                match (name.as_ref(), prop.format) {
                                    ("time-pos", MPV_FORMAT_DOUBLE) => {
                                        let secs = *(prop.data as *const c_double);
                                        if secs.is_finite() && secs >= 0.0 {
                                            st.position_ms = (secs * 1000.0) as i64;
                                        }
                                    }
                                    ("duration", MPV_FORMAT_DOUBLE) => {
                                        let secs = *(prop.data as *const c_double);
                                        if secs.is_finite() && secs > 0.0 {
                                            st.duration_ms = (secs * 1000.0) as i64;
                                        }
                                    }
                                    ("pause", MPV_FORMAT_FLAG) => {
                                        st.paused = *(prop.data as *const c_int) != 0;
                                    }
                                    ("volume", MPV_FORMAT_DOUBLE) => {
                                        st.volume = (*(prop.data as *const c_double)).round() as i64;
                                    }
                                    ("mute", MPV_FORMAT_FLAG) => {
                                        st.muted = *(prop.data as *const c_int) != 0;
                                    }
                                    ("speed", MPV_FORMAT_DOUBLE) => {
                                        st.speed = *(prop.data as *const c_double);
                                    }
                                    ("eof-reached", MPV_FORMAT_FLAG) => {
                                        st.eof_reached = *(prop.data as *const c_int) != 0;
                                    }
                                    ("paused-for-cache", MPV_FORMAT_FLAG) => {
                                        st.buffering = *(prop.data as *const c_int) != 0;
                                    }
                                    ("aid", MPV_FORMAT_INT64) => {
                                        st.audio_track_id = Some(*(prop.data as *const i64));
                                    }
                                    ("sid", MPV_FORMAT_INT64) => {
                                        st.subtitle_track_id = Some(*(prop.data as *const i64));
                                    }
                                    ("sub-visibility", MPV_FORMAT_FLAG) => {
                                        st.subtitles_visible = *(prop.data as *const c_int) != 0;
                                    }
                                    ("width", MPV_FORMAT_INT64) => {
                                        st.video_width = *(prop.data as *const i64);
                                    }
                                    ("height", MPV_FORMAT_INT64) => {
                                        st.video_height = *(prop.data as *const i64);
                                    }
                                    _ => {}
                                }
                            }
                        }
                        event_callback(SurfaceInputEvent::StateChanged);
                    }
                }
                _ => {}
            }
        }
    }

    fn refresh_tracks_internal(&self, vt: &MpvVtable, handle: *mut mpv_handle) {
        let Ok(cprop) = CString::new("track-list") else {
            return;
        };
        unsafe {
            let raw_ptr = (vt.get_property_string)(handle, cprop.as_ptr());
            if raw_ptr.is_null() {
                return;
            }
            let json_str = CStr::from_ptr(raw_ptr).to_string_lossy().to_string();
            (vt.free_fn)(raw_ptr as *mut c_void);

            if let Ok(parsed) = serde_json::from_str::<Vec<MediaTrack>>(&json_str) {
                let mut st = self.state.write();
                st.audio_track_id = parsed
                    .iter()
                    .find(|t| t.track_type == "audio" && t.selected)
                    .map(|t| t.id);
                st.subtitle_track_id = parsed
                    .iter()
                    .find(|t| t.track_type == "sub" && t.selected)
                    .map(|t| t.id);
                st.tracks = parsed;
            }
        }
    }

    pub fn load_stream(
        &self,
        original_url: &str,
        local_stream_url: &str,
        title: &str,
        start_position_ms: Option<i64>,
        initial_speed: Option<f64>,
    ) -> Result<PlayerStateSnapshot> {
        {
            let mut st = self.state.write();
            st.active = true;
            st.paused = false;
            st.original_url = Some(original_url.to_string());
            st.stream_url = Some(local_stream_url.to_string());
            st.title = Some(title.to_string());
            st.position_ms = start_position_ms.unwrap_or(0).max(0);
            st.duration_ms = 0;
            st.eof_reached = false;
            st.last_error = None;
            if let Some(spd) = initial_speed {
                st.speed = spd;
            }
        }

        if let (Some(ref vt), Some(RawMpvHandle(handle))) = (&self.vtable, *self.ctx.lock()) {
            if let Some(pos_ms) = start_position_ms.filter(|&p| p > 0) {
                let secs_str = format!("{:.3}", pos_ms as f64 / 1000.0);
                self.mpv_set_prop_str(vt, handle, "start", &secs_str)?;
            } else {
                let _ = self.mpv_set_prop_str(vt, handle, "start", "none");
            }

            if let Some(spd) = initial_speed {
                let _ = self.mpv_set_prop_double(vt, handle, "speed", spd);
            }

            let cmd_load = CString::new("loadfile")?;
            let cmd_url = CString::new(local_stream_url)?;
            let cmd_mode = CString::new("replace")?;
            let args = [
                cmd_load.as_ptr(),
                cmd_url.as_ptr(),
                cmd_mode.as_ptr(),
                std::ptr::null(),
            ];
            let rc = unsafe { (vt.command)(handle, args.as_ptr()) };
            if rc < 0 {
                let err_msg = vt.err_text(rc);
                self.state.write().last_error = Some(err_msg.clone());
                anyhow::bail!("libmpv loadfile failed: {err_msg}");
            }
            let _ = self.mpv_set_prop_flag(vt, handle, "pause", false);
        }

        Ok(self.snapshot())
    }

    pub fn set_pause(&self, paused: bool) -> Result<PlayerStateSnapshot> {
        self.state.write().paused = paused;
        if let (Some(ref vt), Some(RawMpvHandle(handle))) = (&self.vtable, *self.ctx.lock()) {
            // If playback reached EOF and user presses Play, seek back to 0 automatically
            if !paused && self.state.read().eof_reached {
                let _ = self.seek_absolute_ms(0);
            }
            self.mpv_set_prop_flag(vt, handle, "pause", paused)?;
        }
        Ok(self.snapshot())
    }

    pub fn toggle_pause(&self) -> Result<PlayerStateSnapshot> {
        let next = !self.state.read().paused;
        self.set_pause(next)
    }

    pub fn stop(&self) -> Result<PlayerStateSnapshot> {
        if let (Some(ref vt), Some(RawMpvHandle(handle))) = (&self.vtable, *self.ctx.lock()) {
            let c_stop = CString::new("stop")?;
            let args = [c_stop.as_ptr(), std::ptr::null()];
            unsafe {
                let _ = (vt.command)(handle, args.as_ptr());
            }
        }
        {
            let mut st = self.state.write();
            st.active = false;
            st.paused = true;
            st.position_ms = 0;
            st.duration_ms = 0;
            st.tracks.clear();
        }
        self.set_video_viewport(false, 0, 0, 0, 0);
        Ok(self.snapshot())
    }

    pub fn seek_relative_seconds(&self, delta_secs: f64) -> Result<PlayerStateSnapshot> {
        {
            let mut st = self.state.write();
            let next = (st.position_ms + (delta_secs * 1000.0) as i64).max(0);
            st.position_ms = if st.duration_ms > 0 {
                next.min(st.duration_ms)
            } else {
                next
            };
        }
        if let (Some(ref vt), Some(RawMpvHandle(handle))) = (&self.vtable, *self.ctx.lock()) {
            let c_seek = CString::new("seek")?;
            let c_val = CString::new(format!("{delta_secs:.3}"))?;
            let c_mode = CString::new("relative")?;
            let args = [
                c_seek.as_ptr(),
                c_val.as_ptr(),
                c_mode.as_ptr(),
                std::ptr::null(),
            ];
            unsafe {
                let _ = (vt.command)(handle, args.as_ptr());
            }
        }
        Ok(self.snapshot())
    }

    pub fn seek_absolute_ms(&self, position_ms: i64) -> Result<PlayerStateSnapshot> {
        let clamped = position_ms.max(0);
        self.state.write().position_ms = clamped;
        if let (Some(ref vt), Some(RawMpvHandle(handle))) = (&self.vtable, *self.ctx.lock()) {
            let c_seek = CString::new("seek")?;
            let c_val = CString::new(format!("{:.3}", clamped as f64 / 1000.0))?;
            let c_mode = CString::new("absolute")?;
            let args = [
                c_seek.as_ptr(),
                c_val.as_ptr(),
                c_mode.as_ptr(),
                std::ptr::null(),
            ];
            unsafe {
                let _ = (vt.command)(handle, args.as_ptr());
            }
        }
        Ok(self.snapshot())
    }

    pub fn set_volume(&self, volume: i64) -> Result<PlayerStateSnapshot> {
        let v = volume.clamp(0, 150);
        self.state.write().volume = v;
        if let (Some(ref vt), Some(RawMpvHandle(handle))) = (&self.vtable, *self.ctx.lock()) {
            self.mpv_set_prop_double(vt, handle, "volume", v as f64)?;
        }
        Ok(self.snapshot())
    }

    pub fn set_mute(&self, muted: bool) -> Result<PlayerStateSnapshot> {
        self.state.write().muted = muted;
        if let (Some(ref vt), Some(RawMpvHandle(handle))) = (&self.vtable, *self.ctx.lock()) {
            self.mpv_set_prop_flag(vt, handle, "mute", muted)?;
        }
        Ok(self.snapshot())
    }

    pub fn toggle_mute(&self) -> Result<PlayerStateSnapshot> {
        let next = !self.state.read().muted;
        self.set_mute(next)
    }

    pub fn set_speed(&self, speed: f64) -> Result<PlayerStateSnapshot> {
        let s = speed.clamp(0.25, 4.0);
        self.state.write().speed = s;
        if let (Some(ref vt), Some(RawMpvHandle(handle))) = (&self.vtable, *self.ctx.lock()) {
            self.mpv_set_prop_double(vt, handle, "speed", s)?;
        }
        Ok(self.snapshot())
    }

    pub fn set_audio_track(&self, track_id: i64) -> Result<PlayerStateSnapshot> {
        self.state.write().audio_track_id = Some(track_id);
        if let (Some(ref vt), Some(RawMpvHandle(handle))) = (&self.vtable, *self.ctx.lock()) {
            self.mpv_set_prop_str(vt, handle, "aid", &track_id.to_string())?;
            self.refresh_tracks_internal(vt, handle);
        }
        Ok(self.snapshot())
    }

    pub fn set_subtitle_track(&self, track_id: Option<i64>) -> Result<PlayerStateSnapshot> {
        {
            let mut st = self.state.write();
            st.subtitle_track_id = track_id;
            st.subtitles_visible = track_id.is_some();
        }
        if let (Some(ref vt), Some(RawMpvHandle(handle))) = (&self.vtable, *self.ctx.lock()) {
            match track_id {
                Some(id) if id > 0 => {
                    self.mpv_set_prop_flag(vt, handle, "sub-visibility", true)?;
                    self.mpv_set_prop_str(vt, handle, "sid", &id.to_string())?;
                }
                _ => {
                    self.mpv_set_prop_str(vt, handle, "sid", "no")?;
                }
            }
            self.refresh_tracks_internal(vt, handle);
        }
        Ok(self.snapshot())
    }

    pub fn add_external_subtitle_file(&self, file_path: &Path) -> Result<PlayerStateSnapshot> {
        if !file_path.is_file() {
            anyhow::bail!("Subtitle file not found: {}", file_path.display());
        }
        if let (Some(ref vt), Some(RawMpvHandle(handle))) = (&self.vtable, *self.ctx.lock()) {
            let c_cmd = CString::new("sub-add")?;
            let c_path = CString::new(file_path.to_string_lossy().as_bytes())?;
            let c_sel = CString::new("select")?;
            let args = [c_cmd.as_ptr(), c_path.as_ptr(), c_sel.as_ptr(), std::ptr::null()];
            unsafe {
                let rc = (vt.command)(handle, args.as_ptr());
                if rc < 0 {
                    anyhow::bail!("Failed loading subtitle file: {}", vt.err_text(rc));
                }
            }
            self.refresh_tracks_internal(vt, handle);
        }
        Ok(self.snapshot())
    }

    /// Sets video aspect ratio: `"auto"`, `"16:9"`, `"4:3"`, `"21:9"`, `"2.35:1"`, `"1:1"`, `"stretch"`
    pub fn set_aspect_ratio(&self, ratio: &str) -> Result<PlayerStateSnapshot> {
        self.state.write().aspect_ratio = ratio.to_string();
        if let (Some(ref vt), Some(RawMpvHandle(handle))) = (&self.vtable, *self.ctx.lock()) {
            match ratio {
                "auto" => {
                    let _ = self.mpv_set_prop_str(vt, handle, "keepaspect", "yes");
                    let _ = self.mpv_set_prop_str(vt, handle, "video-aspect-override", "-1");
                }
                "stretch" => {
                    let _ = self.mpv_set_prop_str(vt, handle, "keepaspect", "no");
                }
                custom => {
                    let _ = self.mpv_set_prop_str(vt, handle, "keepaspect", "yes");
                    let _ = self.mpv_set_prop_str(vt, handle, "video-aspect-override", custom);
                }
            }
        }
        Ok(self.snapshot())
    }

    pub fn set_pip_mode(&self, enable: bool) -> Result<PlayerStateSnapshot> {
        #[cfg(windows)]
        {
            let restore = {
                let (_, x, y, w, h) = *self.last_viewport.lock();
                Some((x, y, w, h))
            };
            if let Some(ref host) = *self.win32_host.lock() {
                let active = host.set_pip_mode(enable, restore);
                self.state.write().pip_active = active;
            }
        }
        #[cfg(not(windows))]
        {
            self.state.write().pip_active = enable;
        }
        Ok(self.snapshot())
    }

    pub fn set_video_viewport(&self, visible: bool, x: i32, y: i32, width: i32, height: i32) {
        *self.last_viewport.lock() = (visible, x, y, width, height);
        #[cfg(windows)]
        if let Some(ref host) = *self.win32_host.lock() {
            host.set_viewport(visible, x, y, width, height);
        }
    }

    pub fn set_time_tooltip(
        &self,
        visible: bool,
        client_x: i32,
        client_y: i32,
        scale: f64,
        text: &str,
    ) {
        #[cfg(windows)]
        if let Some(ref host) = *self.win32_host.lock() {
            host.set_time_tooltip(visible, client_x, client_y, scale, text);
        }
        #[cfg(not(windows))]
        {
            let _ = (visible, client_x, client_y, scale, text);
        }
    }

    pub fn snapshot(&self) -> PlayerStateSnapshot {
        self.state.read().clone()
    }

    pub fn shutdown(&self) {
        self.shutdown_flag.store(true, Ordering::SeqCst);
        if let Some(ref vt) = self.vtable {
            if let Some(RawMpvHandle(handle)) = self.ctx.lock().take() {
                unsafe {
                    (vt.terminate_destroy)(handle);
                }
            }
        }
        #[cfg(windows)]
        if let Some(host) = self.win32_host.lock().take() {
            host.destroy();
        }
        let _ = &self.paths;
        self.logger.info("mpv_engine", "Shut down libmpv media engine cleanly");
    }

    fn mpv_set_prop_flag(&self, vt: &MpvVtable, handle: *mut mpv_handle, name: &str, val: bool) -> Result<()> {
        let cname = CString::new(name)?;
        let mut cval: c_int = if val { 1 } else { 0 };
        let rc = unsafe {
            (vt.set_property)(
                handle,
                cname.as_ptr(),
                MPV_FORMAT_FLAG,
                &mut cval as *mut c_int as *mut c_void,
            )
        };
        if rc < 0 {
            anyhow::bail!("mpv_set_property({name}) failed: {}", vt.err_text(rc));
        }
        Ok(())
    }

    fn mpv_set_prop_double(&self, vt: &MpvVtable, handle: *mut mpv_handle, name: &str, val: f64) -> Result<()> {
        let cname = CString::new(name)?;
        let mut cval: c_double = val;
        let rc = unsafe {
            (vt.set_property)(
                handle,
                cname.as_ptr(),
                MPV_FORMAT_DOUBLE,
                &mut cval as *mut c_double as *mut c_void,
            )
        };
        if rc < 0 {
            anyhow::bail!("mpv_set_property({name}) failed: {}", vt.err_text(rc));
        }
        Ok(())
    }

    fn mpv_set_prop_str(&self, vt: &MpvVtable, handle: *mut mpv_handle, name: &str, val: &str) -> Result<()> {
        let cname = CString::new(name)?;
        let cval = CString::new(val)?;
        let rc = unsafe { (vt.set_property_string)(handle, cname.as_ptr(), cval.as_ptr()) };
        if rc < 0 {
            anyhow::bail!("mpv_set_property_string({name}) failed: {}", vt.err_text(rc));
        }
        Ok(())
    }
}
