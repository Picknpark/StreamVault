use crate::logging::StructuredLogger;
use crate::portable::PortablePaths;
use anyhow::{Context, Result};
use libloading::{Library, Symbol};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use std::ffi::{c_char, c_double, c_int, c_ulong, c_void, CStr, CString};
use std::path::{Path, PathBuf};
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
pub struct MediaChapter {
    pub index: usize,
    pub title: String,
    pub time_ms: i64,
}

#[derive(Debug, Deserialize)]
struct RawMpvChapter {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    time: f64,
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
    pub chapters: Vec<MediaChapter>,
    pub audio_track_id: Option<i64>,
    pub subtitle_track_id: Option<i64>,
    pub subtitles_visible: bool,
    pub sub_delay_ms: i64,
    pub audio_delay_ms: i64,
    pub sub_scale: f64,
    pub dialogue_boost: bool,
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
            chapters: Vec::new(),
            audio_track_id: None,
            subtitle_track_id: None,
            subtitles_visible: true,
            sub_delay_ms: 0,
            audio_delay_ms: 0,
            sub_scale: 1.0,
            dialogue_boost: false,
            video_width: 0,
            video_height: 0,
            buffering: false,
            eof_reached: false,
            last_error: None,
        }
    }
}

/// Events emitted from the Win32 video surface `WndProc` (mouse single/double click, wheel, move, keys),
/// Windows System Media Transport Controls (`SMTC` / `WM_APPCOMMAND`), or `libmpv`'s event loop.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SurfaceInputEvent {
    SingleClick,
    DoubleClick,
    WheelDelta { notches: i32 },
    MouseMove,
    KeyPress { vk_code: u32 },
    MediaCommand { action: String },
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
        DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, PAINTSTRUCT,
        TRANSPARENT,
    };
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::System::SystemServices::{
        APPCOMMAND_MEDIA_FAST_FORWARD, APPCOMMAND_MEDIA_NEXTTRACK, APPCOMMAND_MEDIA_PAUSE,
        APPCOMMAND_MEDIA_PLAY, APPCOMMAND_MEDIA_PLAY_PAUSE, APPCOMMAND_MEDIA_PREVIOUSTRACK,
        APPCOMMAND_MEDIA_REWIND, APPCOMMAND_MEDIA_STOP,
    };
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    const SHOT_TIMER_ID: usize = 701;

    struct ShotRenderState {
        display_text: String,
        saved_path: std::path::PathBuf,
        fallback_dir: std::path::PathBuf,
        dpi_scale: f64,
        width: i32,
        height: i32,
        shot_hwnd: isize,
        video_hwnd: isize,
    }

    static REGISTER_ONCE: Once = Once::new();
    static SURFACE_CALLBACK: Mutex<Option<Box<dyn Fn(SurfaceInputEvent) + Send + Sync>>> =
        Mutex::new(None);
    static ORIG_PARENT_WNDPROC: AtomicIsize = AtomicIsize::new(0);
    static LAST_MOUSEMOVE_MS: AtomicU64 = AtomicU64::new(0);
    static PENDING_SINGLE_CLICK: AtomicBool = AtomicBool::new(false);
    static TIP_RENDER_STATE: Mutex<(String, f64)> = Mutex::new((String::new(), 1.0));
    static SHOT_VISIBLE: AtomicBool = AtomicBool::new(false);
    static SHOT_RENDER_STATE: Mutex<ShotRenderState> = Mutex::new(ShotRenderState {
        display_text: String::new(),
        saved_path: std::path::PathBuf::new(),
        fallback_dir: std::path::PathBuf::new(),
        dpi_scale: 1.0,
        width: 0,
        height: 0,
        shot_hwnd: 0,
        video_hwnd: 0,
    });

    pub struct Win32VideoHost {
        parent_hwnd: AtomicIsize,
        video_hwnd: AtomicIsize,
        pip_hwnd: AtomicIsize,
        tip_hwnd: AtomicIsize,
        shot_hwnd: AtomicIsize,
        pip_mode: AtomicBool,
        tip_visible: AtomicBool,
        tip_size: Mutex<(i32, i32)>,
        shot_size: Mutex<(i32, i32)>,
        applied_viewport: Mutex<(bool, i32, i32, i32, i32)>,
        smtc: Mutex<Option<windows::Media::SystemMediaTransportControls>>,
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
            let shot_class_name: Vec<u16> = OsStr::new("StreamVaultShotToast")
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

                    let shot_wc = WNDCLASSEXW {
                        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                        style: 0,
                        lpfnWndProc: Some(shot_toast_wndproc),
                        cbClsExtra: 0,
                        cbWndExtra: 0,
                        hInstance: hinstance,
                        hIcon: std::ptr::null_mut(),
                        hCursor: LoadCursorW(std::ptr::null_mut(), IDC_HAND),
                        hbrBackground: std::ptr::null_mut(),
                        lpszMenuName: std::ptr::null(),
                        lpszClassName: shot_class_name.as_ptr(),
                        hIconSm: std::ptr::null_mut(),
                    };
                    RegisterClassExW(&shot_wc);
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

                // Native interactive `WS_POPUP` owned by `parent_hwnd` (`main_hwnd`) so screenshot
                // saved notifications ALWAYS float cleanly IN FRONT OF the D3D11 video frame!
                let shot_hwnd = CreateWindowExW(
                    WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                    shot_class_name.as_ptr(),
                    std::ptr::null(),
                    WS_POPUP,
                    0,
                    0,
                    340,
                    40,
                    parent_hwnd as HWND,
                    std::ptr::null_mut(),
                    hinstance,
                    std::ptr::null(),
                );

                // Subclass main parent HWND strictly to intercept WM_APPCOMMAND hardware media keys
                // while forwarding 100% of all other messages untouched to Tauri/wry!
                if ORIG_PARENT_WNDPROC.load(Ordering::SeqCst) == 0 && parent_hwnd != 0 {
                    let prev = SetWindowLongPtrW(
                        parent_hwnd as HWND,
                        GWLP_WNDPROC,
                        parent_media_subclass_wndproc as *const () as usize as isize,
                    );
                    if prev != 0 {
                        ORIG_PARENT_WNDPROC.store(prev, Ordering::SeqCst);
                    }
                }

                // Bind Windows 10/11 System Media Transport Controls (SMTC) for global hardware
                // media keys + OS Volume/Media Flyout integration.
                let smtc = init_winrt_smtc(parent_hwnd);

                Ok(Self {
                    parent_hwnd: AtomicIsize::new(parent_hwnd),
                    video_hwnd: AtomicIsize::new(video_hwnd as isize),
                    pip_hwnd: AtomicIsize::new(pip_hwnd as isize),
                    tip_hwnd: AtomicIsize::new(tip_hwnd as isize),
                    shot_hwnd: AtomicIsize::new(shot_hwnd as isize),
                    pip_mode: AtomicBool::new(false),
                    tip_visible: AtomicBool::new(false),
                    tip_size: Mutex::new((0, 0)),
                    shot_size: Mutex::new((0, 0)),
                    applied_viewport: Mutex::new((false, 0, 0, 0, 0)),
                    smtc: Mutex::new(smtc),
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
                    if SHOT_VISIBLE.load(Ordering::SeqCst) {
                        reposition_active_shot_toast();
                    }
                } else {
                    *self.applied_viewport.lock() = (false, 0, 0, 0, 0);
                    self.hide_time_tooltip();
                    self.hide_screenshot_toast();
                    if IsWindowVisible(hwnd) != 0 {
                        ShowWindow(hwnd, SW_HIDE);
                    }
                }
            }
        }

        pub fn hide_screenshot_toast(&self) {
            let shot = self.shot_hwnd.load(Ordering::SeqCst) as HWND;
            if !shot.is_null() && SHOT_VISIBLE.swap(false, Ordering::SeqCst) {
                unsafe {
                    let _ = KillTimer(shot, SHOT_TIMER_ID);
                    ShowWindow(shot, SW_HIDE);
                }
            }
        }

        pub fn show_screenshot_toast(
            &self,
            filename: &str,
            saved_path: &std::path::Path,
            fallback_dir: &std::path::Path,
            scale: f64,
        ) -> bool {
            if self.pip_mode.load(Ordering::SeqCst) {
                return false;
            }
            let shot = self.shot_hwnd.load(Ordering::SeqCst) as HWND;
            let v_hwnd = self.video_hwnd.load(Ordering::SeqCst) as HWND;
            let main_hwnd = self.parent_hwnd.load(Ordering::SeqCst) as HWND;
            if shot.is_null() || v_hwnd.is_null() || main_hwnd.is_null() {
                return false;
            }

            unsafe {
                if IsWindowVisible(v_hwnd) == 0 {
                    return false;
                }

                let dpi_scale = scale.clamp(1.0, 4.0);
                let clean_name: String = if filename.chars().count() > 38 {
                    let head: String = filename.chars().take(22).collect();
                    let tail: String = filename
                        .chars()
                        .rev()
                        .take(14)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .collect();
                    format!("{head}…{tail}")
                } else {
                    filename.to_string()
                };
                let display_text = format!("✓ Saved {clean_name}");
                let width = (((display_text.chars().count() as f64) * 7.4 + 108.0) * dpi_scale)
                    .round()
                    .clamp(260.0 * dpi_scale, 460.0 * dpi_scale) as i32;
                let height = (40.0 * dpi_scale).round() as i32;
                let margin = (16.0 * dpi_scale).round() as i32;

                {
                    let mut st = SHOT_RENDER_STATE.lock();
                    st.display_text = display_text;
                    st.saved_path = saved_path.to_path_buf();
                    st.fallback_dir = fallback_dir.to_path_buf();
                    st.dpi_scale = dpi_scale;
                    st.width = width;
                    st.height = height;
                    st.shot_hwnd = shot as isize;
                    st.video_hwnd = v_hwnd as isize;
                }

                {
                    let mut prev_sz = self.shot_size.lock();
                    if *prev_sz != (width, height) {
                        *prev_sz = (width, height);
                        let radius = (8.0 * dpi_scale).round() as i32;
                        let rgn = CreateRoundRectRgn(0, 0, width + 1, height + 1, radius, radius);
                        if !rgn.is_null() {
                            let _ = SetWindowRgn(shot, rgn, 0);
                        }
                    }
                }

                let mut vrc: RECT = std::mem::zeroed();
                if GetWindowRect(v_hwnd, &mut vrc) == 0 {
                    return false;
                }
                let screen_x = (vrc.right - width - margin).max(vrc.left + margin);
                let screen_y = (vrc.bottom - height - margin).max(vrc.top + margin);

                let already_vis = SHOT_VISIBLE.swap(true, Ordering::SeqCst);
                let flags = if already_vis {
                    SWP_NOACTIVATE
                } else {
                    SWP_NOACTIVATE | SWP_SHOWWINDOW
                };
                SetWindowPos(shot, HWND_TOP, screen_x, screen_y, width, height, flags);
                let _ = InvalidateRect(shot, std::ptr::null(), 0);
                let _ = SetTimer(shot, SHOT_TIMER_ID, 5000, None);
                true
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
            let display_text: String = if text.chars().count() > 44 {
                let mut s: String = text.chars().take(43).collect();
                s.push('…');
                s
            } else {
                text.to_string()
            };
            let char_count = display_text.chars().count().max(5);
            let width = (((char_count as f64) * 8.1 + 18.0) * dpi_scale).round() as i32;
            let height = (24.0 * dpi_scale).round() as i32;
            let gap = (6.0 * dpi_scale).round() as i32;

            {
                let mut st = TIP_RENDER_STATE.lock();
                st.0.clear();
                st.0.push_str(&display_text);
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
                    self.hide_screenshot_toast();
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
                        if IsWindowVisible(v_hwnd) != 0 {
                            ShowWindow(v_hwnd, SW_HIDE);
                        }
                    }
                    false
                }
            }
        }

        pub fn update_smtc(
            &self,
            active: bool,
            paused: bool,
            title: Option<&str>,
            has_prev: bool,
            has_next: bool,
        ) {
            let guard = self.smtc.lock();
            let Some(ref smtc) = *guard else {
                return;
            };
            let _ = smtc.SetIsEnabled(active);
            let _ = smtc.SetIsPlayEnabled(active);
            let _ = smtc.SetIsPauseEnabled(active);
            let _ = smtc.SetIsStopEnabled(active);
            let _ = smtc.SetIsPreviousEnabled(active && has_prev);
            let _ = smtc.SetIsNextEnabled(active && has_next);
            let status = if !active {
                windows::Media::MediaPlaybackStatus::Closed
            } else if paused {
                windows::Media::MediaPlaybackStatus::Paused
            } else {
                windows::Media::MediaPlaybackStatus::Playing
            };
            let _ = smtc.SetPlaybackStatus(status);
            if active {
                if let Ok(updater) = smtc.DisplayUpdater() {
                    let _ = updater.SetType(windows::Media::MediaPlaybackType::Video);
                    if let Ok(vp) = updater.VideoProperties() {
                        let clean_title = title.unwrap_or("StreamVault Media");
                        let _ = vp.SetTitle(&windows::core::HSTRING::from(clean_title));
                        let _ = vp.SetSubtitle(&windows::core::HSTRING::from("StreamVault"));
                    }
                    let _ = updater.Update();
                }
            }
        }

        pub fn destroy(&self) {
            if let Some(ref smtc) = *self.smtc.lock() {
                let _ = smtc.SetPlaybackStatus(windows::Media::MediaPlaybackStatus::Closed);
                let _ = smtc.SetIsEnabled(false);
            }
            unsafe {
                let main_hwnd = self.parent_hwnd.load(Ordering::SeqCst) as HWND;
                let orig = ORIG_PARENT_WNDPROC.swap(0, Ordering::SeqCst);
                if !main_hwnd.is_null() && orig != 0 {
                    let _ = SetWindowLongPtrW(main_hwnd, GWLP_WNDPROC, orig);
                }
                let s = self.shot_hwnd.swap(0, Ordering::SeqCst) as HWND;
                let t = self.tip_hwnd.swap(0, Ordering::SeqCst) as HWND;
                let v = self.video_hwnd.swap(0, Ordering::SeqCst) as HWND;
                let p = self.pip_hwnd.swap(0, Ordering::SeqCst) as HWND;
                SHOT_VISIBLE.store(false, Ordering::SeqCst);
                if !s.is_null() {
                    let _ = KillTimer(s, SHOT_TIMER_ID);
                    DestroyWindow(s);
                }
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

    unsafe fn reposition_active_shot_toast() {
        let (shot_raw, video_raw, width, height, dpi_scale) = {
            let st = SHOT_RENDER_STATE.lock();
            (st.shot_hwnd, st.video_hwnd, st.width, st.height, st.dpi_scale)
        };
        let shot = shot_raw as HWND;
        let v_hwnd = video_raw as HWND;
        if shot.is_null() || v_hwnd.is_null() || width <= 0 || height <= 0 {
            return;
        }
        if IsWindowVisible(v_hwnd) == 0 {
            SHOT_VISIBLE.store(false, Ordering::SeqCst);
            let _ = KillTimer(shot, SHOT_TIMER_ID);
            ShowWindow(shot, SW_HIDE);
            return;
        }
        let mut vrc: RECT = std::mem::zeroed();
        if GetWindowRect(v_hwnd, &mut vrc) != 0 {
            let margin = (16.0 * dpi_scale).round() as i32;
            let screen_x = (vrc.right - width - margin).max(vrc.left + margin);
            let screen_y = (vrc.bottom - height - margin).max(vrc.top + margin);
            SetWindowPos(
                shot,
                HWND_TOP,
                screen_x,
                screen_y,
                width,
                height,
                SWP_NOACTIVATE,
            );
        }
    }

    fn init_winrt_smtc(parent_hwnd: isize) -> Option<windows::Media::SystemMediaTransportControls> {
        use windows::Foundation::TypedEventHandler;
        use windows::Media::{
            SystemMediaTransportControls, SystemMediaTransportControlsButton,
            SystemMediaTransportControlsButtonPressedEventArgs,
        };
        use windows::Win32::System::WinRT::ISystemMediaTransportControlsInterop;

        if parent_hwnd == 0 {
            return None;
        }
        unsafe {
            let interop: ISystemMediaTransportControlsInterop =
                windows::core::factory::<SystemMediaTransportControls, ISystemMediaTransportControlsInterop>().ok()?;
            let win_hwnd = windows::Win32::Foundation::HWND(parent_hwnd as *mut core::ffi::c_void);
            let smtc: SystemMediaTransportControls = interop.GetForWindow(win_hwnd).ok()?;
            let _ = smtc.SetIsPlayEnabled(true);
            let _ = smtc.SetIsPauseEnabled(true);
            let _ = smtc.SetIsStopEnabled(true);
            let _ = smtc.SetIsNextEnabled(true);
            let _ = smtc.SetIsPreviousEnabled(true);
            let handler = TypedEventHandler::<
                SystemMediaTransportControls,
                SystemMediaTransportControlsButtonPressedEventArgs,
            >::new(|_, args| {
                if let Some(ref a) = *args {
                    if let Ok(btn) = a.Button() {
                        let action = match btn {
                            b if b == SystemMediaTransportControlsButton::Play => Some("play"),
                            b if b == SystemMediaTransportControlsButton::Pause => Some("pause"),
                            b if b == SystemMediaTransportControlsButton::Stop => Some("stop"),
                            b if b == SystemMediaTransportControlsButton::Next => Some("next"),
                            b if b == SystemMediaTransportControlsButton::Previous => {
                                Some("previous")
                            }
                            b if b == SystemMediaTransportControlsButton::FastForward => {
                                Some("fast_forward")
                            }
                            b if b == SystemMediaTransportControlsButton::Rewind => Some("rewind"),
                            _ => None,
                        };
                        if let Some(act) = action {
                            emit_surface(SurfaceInputEvent::MediaCommand {
                                action: act.to_string(),
                            });
                        }
                    }
                }
                Ok(())
            });
            let _ = smtc.ButtonPressed(&handler);
            Some(smtc)
        }
    }

    fn map_appcommand_to_action(lparam: LPARAM) -> Option<&'static str> {
        let cmd = (((lparam >> 16) as u32) & 0x0FFF) as u32;
        match cmd {
            APPCOMMAND_MEDIA_PLAY_PAUSE => Some("play_pause"),
            APPCOMMAND_MEDIA_PLAY => Some("play"),
            APPCOMMAND_MEDIA_PAUSE => Some("pause"),
            APPCOMMAND_MEDIA_STOP => Some("stop"),
            APPCOMMAND_MEDIA_NEXTTRACK => Some("next"),
            APPCOMMAND_MEDIA_PREVIOUSTRACK => Some("previous"),
            APPCOMMAND_MEDIA_FAST_FORWARD => Some("fast_forward"),
            APPCOMMAND_MEDIA_REWIND => Some("rewind"),
            _ => None,
        }
    }

    fn map_vk_media_to_action(vk: u32) -> Option<&'static str> {
        match vk {
            0xB3 => Some("play_pause"), // VK_MEDIA_PLAY_PAUSE
            0xB2 => Some("stop"),       // VK_MEDIA_STOP
            0xB0 => Some("next"),       // VK_MEDIA_NEXT_TRACK
            0xB1 => Some("previous"),   // VK_MEDIA_PREV_TRACK
            _ => None,
        }
    }

    unsafe extern "system" fn parent_media_subclass_wndproc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if msg == WM_APPCOMMAND {
            if let Some(act) = map_appcommand_to_action(lparam) {
                emit_surface(SurfaceInputEvent::MediaCommand {
                    action: act.to_string(),
                });
                return 1;
            }
        } else if msg == WM_KEYDOWN {
            if let Some(act) = map_vk_media_to_action(wparam as u32) {
                emit_surface(SurfaceInputEvent::MediaCommand {
                    action: act.to_string(),
                });
                return 0;
            }
        }
        let orig = ORIG_PARENT_WNDPROC.load(Ordering::SeqCst);
        let res = if orig != 0 {
            let proc_fn: WNDPROC = Some(std::mem::transmute(orig));
            CallWindowProcW(proc_fn, hwnd, msg, wparam, lparam)
        } else {
            DefWindowProcW(hwnd, msg, wparam, lparam)
        };
        if (msg == WM_MOVE || msg == WM_SIZE) && SHOT_VISIBLE.load(Ordering::SeqCst) {
            reposition_active_shot_toast();
        }
        res
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
            WM_APPCOMMAND => {
                if let Some(act) = map_appcommand_to_action(lparam) {
                    emit_surface(SurfaceInputEvent::MediaCommand {
                        action: act.to_string(),
                    });
                    return 1;
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            WM_KEYDOWN => {
                let vk = wparam as u32;
                if let Some(act) = map_vk_media_to_action(vk) {
                    emit_surface(SurfaceInputEvent::MediaCommand {
                        action: act.to_string(),
                    });
                } else {
                    emit_surface(SurfaceInputEvent::KeyPress { vk_code: vk });
                }
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

    unsafe extern "system" fn shot_toast_wndproc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_ERASEBKGND => 1,
            WM_MOUSEACTIVATE => 3, // MA_NOACTIVATE — clicking Reveal never steals focus or breaks fullscreen
            WM_TIMER => {
                if wparam == SHOT_TIMER_ID {
                    SHOT_VISIBLE.store(false, Ordering::SeqCst);
                    let _ = KillTimer(hwnd, SHOT_TIMER_ID);
                    ShowWindow(hwnd, SW_HIDE);
                }
                0
            }
            WM_LBUTTONDOWN => {
                let mut rc: RECT = std::mem::zeroed();
                let _ = GetClientRect(hwnd, &mut rc);
                let click_x = (lparam & 0xFFFF) as i16 as i32;
                let (saved_path, fallback_dir, dpi_scale) = {
                    let st = SHOT_RENDER_STATE.lock();
                    (st.saved_path.clone(), st.fallback_dir.clone(), st.dpi_scale)
                };
                let reveal_zone_left = rc.right - (76.0 * dpi_scale).round() as i32;
                SHOT_VISIBLE.store(false, Ordering::SeqCst);
                let _ = KillTimer(hwnd, SHOT_TIMER_ID);
                ShowWindow(hwnd, SW_HIDE);
                if click_x >= reveal_zone_left && !saved_path.as_os_str().is_empty() {
                    std::thread::spawn(move || {
                        let _ = crate::downloader::reveal_in_file_manager(
                            &saved_path,
                            None,
                            &fallback_dir,
                        );
                    });
                }
                0
            }
            WM_PAINT => {
                let mut ps: PAINTSTRUCT = std::mem::zeroed();
                let hdc = BeginPaint(hwnd, &mut ps);
                if !hdc.is_null() {
                    let mut rc: RECT = std::mem::zeroed();
                    let _ = GetClientRect(hwnd, &mut rc);

                    let (text, dpi_scale) = {
                        let st = SHOT_RENDER_STATE.lock();
                        (st.display_text.clone(), st.dpi_scale)
                    };

                    // Dark card background (#151b28 -> COLORREF 0x00BBGGRR = 0x00281b15)
                    let bg_brush = CreateSolidBrush(0x00281b15);
                    if !bg_brush.is_null() {
                        FillRect(hdc, &rc, bg_brush);
                        let _ = DeleteObject(bg_brush as _);
                    }

                    // Emerald success accent stripe + border (#22c55e -> 0x005ec522)
                    let accent_brush = CreateSolidBrush(0x005ec522);
                    if !accent_brush.is_null() {
                        let stripe_rc = RECT {
                            left: rc.left,
                            top: rc.top,
                            right: (4.0 * dpi_scale).round() as i32,
                            bottom: rc.bottom,
                        };
                        FillRect(hdc, &stripe_rc, accent_brush);
                        FrameRect(hdc, &rc, accent_brush);
                        let _ = DeleteObject(accent_brush as _);
                    }

                    // Right-side "Reveal" button pill (#232d42 fill, #38bdf8 border)
                    let mut btn_rc = RECT {
                        left: rc.right - (70.0 * dpi_scale).round() as i32,
                        top: (7.0 * dpi_scale).round() as i32,
                        right: rc.right - (10.0 * dpi_scale).round() as i32,
                        bottom: rc.bottom - (7.0 * dpi_scale).round() as i32,
                    };
                    let btn_bg = CreateSolidBrush(0x00422d23);
                    if !btn_bg.is_null() {
                        FillRect(hdc, &btn_rc, btn_bg);
                        let _ = DeleteObject(btn_bg as _);
                    }
                    let btn_border = CreateSolidBrush(0x00f8bd38);
                    if !btn_border.is_null() {
                        FrameRect(hdc, &btn_rc, btn_border);
                        let _ = DeleteObject(btn_border as _);
                    }

                    let font_height = -((13.0 * dpi_scale).round() as i32);
                    let face: Vec<u16> = OsStr::new("Segoe UI")
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

                    if !text.is_empty() {
                        SetTextColor(hdc, 0x00fcfaf8);
                        let mut text_rc = RECT {
                            left: (14.0 * dpi_scale).round() as i32,
                            top: 0,
                            right: btn_rc.left - (10.0 * dpi_scale).round() as i32,
                            bottom: rc.bottom,
                        };
                        let wide_text: Vec<u16> = OsStr::new(&text).encode_wide().collect();
                        DrawTextW(
                            hdc,
                            wide_text.as_ptr(),
                            wide_text.len() as i32,
                            &mut text_rc,
                            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
                        );
                    }

                    // "Reveal" label
                    SetTextColor(hdc, 0x00fcfaf8);
                    let reveal_wide: Vec<u16> = OsStr::new("Reveal").encode_wide().collect();
                    DrawTextW(
                        hdc,
                        reveal_wide.as_ptr(),
                        reveal_wide.len() as i32,
                        &mut btn_rc,
                        DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
                    );

                    if !old_font.is_null() {
                        let _ = SelectObject(hdc, old_font);
                    }
                    if !font.is_null() {
                        let _ = DeleteObject(font as _);
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
                // Closing PiP window exits PiP mode cleanly and re-parents video HWND back to main window!
                ShowWindow(hwnd, SW_HIDE);
                emit_surface(SurfaceInputEvent::MediaCommand {
                    action: "exit_pip".to_string(),
                });
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
                ("sub-delay", MPV_FORMAT_DOUBLE),
                ("audio-delay", MPV_FORMAT_DOUBLE),
                ("sub-scale", MPV_FORMAT_DOUBLE),
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
                        if ef.reason == 4 || err_msg.is_some() {
                            let mut st = self.state.write();
                            if let Some(ref e) = err_msg {
                                st.last_error = Some(e.clone());
                            }
                            st.active = false;
                            st.paused = true;
                            st.buffering = false;
                        }
                        event_callback(SurfaceInputEvent::PlaybackEnded {
                            reason: ef.reason,
                            error_message: err_msg,
                        });
                        event_callback(SurfaceInputEvent::StateChanged);
                    }
                }
                MPV_EVENT_PROPERTY_CHANGE => {
                    if !ev.data.is_null() {
                        let prop = unsafe { &*(ev.data as *const MpvEventProperty) };
                        if !prop.name.is_null() {
                            let name = unsafe { CStr::from_ptr(prop.name).to_string_lossy() };
                            let mut emit_eof_ended = false;
                            {
                                let mut st = self.state.write();
                                if prop.format == 0 {
                                    match name.as_ref() {
                                        "width" => st.video_width = 0,
                                        "height" => st.video_height = 0,
                                        "aid" => st.audio_track_id = None,
                                        "sid" => st.subtitle_track_id = None,
                                        _ => {}
                                    }
                                } else if !prop.data.is_null() {
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
                                                st.volume =
                                                    (*(prop.data as *const c_double)).round() as i64;
                                            }
                                            ("mute", MPV_FORMAT_FLAG) => {
                                                st.muted = *(prop.data as *const c_int) != 0;
                                            }
                                            ("speed", MPV_FORMAT_DOUBLE) => {
                                                st.speed = *(prop.data as *const c_double);
                                            }
                                            ("eof-reached", MPV_FORMAT_FLAG) => {
                                                let now_eof = *(prop.data as *const c_int) != 0;
                                                let was_eof = st.eof_reached;
                                                st.eof_reached = now_eof;
                                                if !was_eof && now_eof && st.active {
                                                    if st.duration_ms > 0 {
                                                        st.position_ms = st.duration_ms;
                                                    }
                                                    emit_eof_ended = true;
                                                }
                                            }
                                            ("paused-for-cache", MPV_FORMAT_FLAG) => {
                                                st.buffering = *(prop.data as *const c_int) != 0;
                                            }
                                            ("aid", MPV_FORMAT_INT64) => {
                                                st.audio_track_id =
                                                    Some(*(prop.data as *const i64));
                                            }
                                            ("sid", MPV_FORMAT_INT64) => {
                                                st.subtitle_track_id =
                                                    Some(*(prop.data as *const i64));
                                            }
                                            ("sub-visibility", MPV_FORMAT_FLAG) => {
                                                st.subtitles_visible =
                                                    *(prop.data as *const c_int) != 0;
                                            }
                                            ("sub-delay", MPV_FORMAT_DOUBLE) => {
                                                let secs = *(prop.data as *const c_double);
                                                if secs.is_finite() {
                                                    st.sub_delay_ms =
                                                        (secs * 1000.0).round() as i64;
                                                }
                                            }
                                            ("audio-delay", MPV_FORMAT_DOUBLE) => {
                                                let secs = *(prop.data as *const c_double);
                                                if secs.is_finite() {
                                                    st.audio_delay_ms =
                                                        (secs * 1000.0).round() as i64;
                                                }
                                            }
                                            ("sub-scale", MPV_FORMAT_DOUBLE) => {
                                                let scale = *(prop.data as *const c_double);
                                                if scale.is_finite() && scale > 0.0 {
                                                    st.sub_scale = scale;
                                                }
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
                            }
                            event_callback(SurfaceInputEvent::StateChanged);
                            if emit_eof_ended {
                                event_callback(SurfaceInputEvent::PlaybackEnded {
                                    reason: 0,
                                    error_message: None,
                                });
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn refresh_tracks_internal(&self, vt: &MpvVtable, handle: *mut mpv_handle) {
        if let Ok(cprop) = CString::new("track-list") {
            unsafe {
                let raw_ptr = (vt.get_property_string)(handle, cprop.as_ptr());
                if !raw_ptr.is_null() {
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
        }

        if let Ok(cchap) = CString::new("chapter-list") {
            unsafe {
                let raw_ptr = (vt.get_property_string)(handle, cchap.as_ptr());
                if !raw_ptr.is_null() {
                    let json_str = CStr::from_ptr(raw_ptr).to_string_lossy().to_string();
                    (vt.free_fn)(raw_ptr as *mut c_void);

                    if let Ok(parsed) = serde_json::from_str::<Vec<RawMpvChapter>>(&json_str) {
                        let chapters: Vec<MediaChapter> = parsed
                            .into_iter()
                            .enumerate()
                            .filter(|(_, c)| c.time.is_finite() && c.time >= 0.0)
                            .map(|(idx, c)| {
                                let title = c
                                    .title
                                    .map(|t| t.trim().to_string())
                                    .filter(|t| !t.is_empty())
                                    .unwrap_or_else(|| format!("Chapter {}", idx + 1));
                                MediaChapter {
                                    index: idx,
                                    title,
                                    time_ms: (c.time * 1000.0).round() as i64,
                                }
                            })
                            .collect();
                        self.state.write().chapters = chapters;
                    }
                }
            }
        }
    }

    fn active_mpv(&self) -> Option<(&Arc<MpvVtable>, *mut mpv_handle)> {
        let vt = self.vtable.as_ref()?;
        let RawMpvHandle(handle) = (*self.ctx.lock())?;
        Some((vt, handle))
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
            st.tracks.clear();
            st.chapters.clear();
            st.audio_track_id = None;
            st.subtitle_track_id = None;
            st.video_width = 0;
            st.video_height = 0;
            st.buffering = false;
            st.sub_delay_ms = 0;
            st.audio_delay_ms = 0;
            st.eof_reached = false;
            st.last_error = None;
            if let Some(spd) = initial_speed {
                st.speed = spd;
            }
        }

        if let Some((vt, handle)) = self.active_mpv() {
            let _ = self.mpv_set_prop_double(vt, handle, "sub-delay", 0.0);
            let _ = self.mpv_set_prop_double(vt, handle, "audio-delay", 0.0);
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

        self.sync_smtc(false, false);
        Ok(self.snapshot())
    }

    /// Loads a local video/audio file directly into `libmpv` (bypassing any HTTP proxy/cache)
    /// and automatically attaches stem-matching sidecar `.srt`/`.ass`/`.ssa`/`.vtt` subtitles
    /// found in the same local directory when `auto_attach_sidecar_subs` is enabled.
    pub fn load_local_file(
        &self,
        file_path: &Path,
        title: &str,
        start_position_ms: Option<i64>,
        initial_speed: Option<f64>,
        auto_attach_sidecar_subs: bool,
    ) -> Result<PlayerStateSnapshot> {
        if !file_path.is_file() {
            anyhow::bail!("Local media file not found: {}", file_path.display());
        }
        let path_str = file_path.to_string_lossy().to_string();
        let file_uri_id = format!("local://{}", path_str.replace('\\', "/"));

        {
            let mut st = self.state.write();
            st.active = true;
            st.paused = false;
            st.original_url = Some(file_uri_id);
            st.stream_url = Some(path_str.clone());
            st.title = Some(title.to_string());
            st.position_ms = start_position_ms.unwrap_or(0).max(0);
            st.duration_ms = 0;
            st.tracks.clear();
            st.chapters.clear();
            st.audio_track_id = None;
            st.subtitle_track_id = None;
            st.video_width = 0;
            st.video_height = 0;
            st.buffering = false;
            st.sub_delay_ms = 0;
            st.audio_delay_ms = 0;
            st.eof_reached = false;
            st.last_error = None;
            if let Some(spd) = initial_speed {
                st.speed = spd;
            }
        }

        if let Some((vt, handle)) = self.active_mpv() {
            let _ = self.mpv_set_prop_double(vt, handle, "sub-delay", 0.0);
            let _ = self.mpv_set_prop_double(vt, handle, "audio-delay", 0.0);
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
            let cmd_path = CString::new(path_str.as_bytes())?;
            let cmd_mode = CString::new("replace")?;
            let args = [
                cmd_load.as_ptr(),
                cmd_path.as_ptr(),
                cmd_mode.as_ptr(),
                std::ptr::null(),
            ];
            let rc = unsafe { (vt.command)(handle, args.as_ptr()) };
            if rc < 0 {
                let err_msg = vt.err_text(rc);
                self.state.write().last_error = Some(err_msg.clone());
                anyhow::bail!("libmpv local loadfile failed: {err_msg}");
            }
            let _ = self.mpv_set_prop_flag(vt, handle, "pause", false);

            if auto_attach_sidecar_subs {
                for sub_path in discover_local_sidecar_subtitles(file_path) {
                    if let (Ok(c_cmd), Ok(c_sub), Ok(c_mode)) = (
                        CString::new("sub-add"),
                        CString::new(sub_path.to_string_lossy().as_bytes()),
                        CString::new("auto"),
                    ) {
                        let sub_args = [
                            c_cmd.as_ptr(),
                            c_sub.as_ptr(),
                            c_mode.as_ptr(),
                            std::ptr::null(),
                        ];
                        unsafe {
                            let _ = (vt.command)(handle, sub_args.as_ptr());
                        }
                    }
                }
            }
        }

        self.sync_smtc(false, false);
        Ok(self.snapshot())
    }

    pub fn set_pause(&self, paused: bool) -> Result<PlayerStateSnapshot> {
        let should_rewind = {
            let mut st = self.state.write();
            st.paused = paused;
            if !paused && st.eof_reached {
                st.eof_reached = false;
                st.position_ms = 0;
                true
            } else {
                false
            }
        };
        if let Some((vt, handle)) = self.active_mpv() {
            if should_rewind {
                let _ = self.seek_absolute_ms(0);
            }
            self.mpv_set_prop_flag(vt, handle, "pause", paused)?;
        }
        self.sync_smtc(false, false);
        Ok(self.snapshot())
    }

    pub fn toggle_pause(&self) -> Result<PlayerStateSnapshot> {
        let next = !self.state.read().paused;
        self.set_pause(next)
    }

    pub fn stop(&self) -> Result<PlayerStateSnapshot> {
        if let Some((vt, handle)) = self.active_mpv() {
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
            st.chapters.clear();
            st.audio_track_id = None;
            st.subtitle_track_id = None;
            st.video_width = 0;
            st.video_height = 0;
            st.buffering = false;
            st.eof_reached = false;
            st.sub_delay_ms = 0;
            st.audio_delay_ms = 0;
        }
        self.set_video_viewport(false, 0, 0, 0, 0);
        self.sync_smtc(false, false);
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
            if delta_secs < 0.0 {
                st.eof_reached = false;
            }
        }
        if let Some((vt, handle)) = self.active_mpv() {
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
        {
            let mut st = self.state.write();
            st.position_ms = clamped;
            if st.duration_ms == 0 || clamped < st.duration_ms {
                st.eof_reached = false;
            }
        }
        if let Some((vt, handle)) = self.active_mpv() {
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
        if let Some((vt, handle)) = self.active_mpv() {
            self.mpv_set_prop_double(vt, handle, "volume", v as f64)?;
        }
        Ok(self.snapshot())
    }

    pub fn set_mute(&self, muted: bool) -> Result<PlayerStateSnapshot> {
        self.state.write().muted = muted;
        if let Some((vt, handle)) = self.active_mpv() {
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
        if let Some((vt, handle)) = self.active_mpv() {
            self.mpv_set_prop_double(vt, handle, "speed", s)?;
        }
        Ok(self.snapshot())
    }

    pub fn set_audio_track(&self, track_id: i64) -> Result<PlayerStateSnapshot> {
        self.state.write().audio_track_id = Some(track_id);
        if let Some((vt, handle)) = self.active_mpv() {
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
        if let Some((vt, handle)) = self.active_mpv() {
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
        if let Some((vt, handle)) = self.active_mpv() {
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

    pub fn add_external_subtitle_url(
        &self,
        subtitle_url: &str,
        select: bool,
    ) -> Result<PlayerStateSnapshot> {
        let trimmed = subtitle_url.trim();
        if trimmed.is_empty() {
            anyhow::bail!("Subtitle URL cannot be empty");
        }
        if let Some((vt, handle)) = self.active_mpv() {
            let c_cmd = CString::new("sub-add")?;
            let c_url = CString::new(trimmed.as_bytes())?;
            let c_mode = CString::new(if select { "select" } else { "auto" })?;
            let args = [
                c_cmd.as_ptr(),
                c_url.as_ptr(),
                c_mode.as_ptr(),
                std::ptr::null(),
            ];
            unsafe {
                let rc = (vt.command)(handle, args.as_ptr());
                if rc < 0 {
                    anyhow::bail!("Failed attaching remote subtitle URL: {}", vt.err_text(rc));
                }
            }
            self.refresh_tracks_internal(vt, handle);
        }
        Ok(self.snapshot())
    }

    pub fn set_sub_delay_ms(&self, delay_ms: i64) -> Result<PlayerStateSnapshot> {
        let clamped = delay_ms.clamp(-60_000, 60_000);
        self.state.write().sub_delay_ms = clamped;
        if let Some((vt, handle)) = self.active_mpv() {
            self.mpv_set_prop_double(vt, handle, "sub-delay", (clamped as f64) / 1000.0)?;
        }
        Ok(self.snapshot())
    }

    pub fn set_audio_delay_ms(&self, delay_ms: i64) -> Result<PlayerStateSnapshot> {
        let clamped = delay_ms.clamp(-60_000, 60_000);
        self.state.write().audio_delay_ms = clamped;
        if let Some((vt, handle)) = self.active_mpv() {
            self.mpv_set_prop_double(vt, handle, "audio-delay", (clamped as f64) / 1000.0)?;
        }
        Ok(self.snapshot())
    }

    pub fn set_sub_scale(&self, scale: f64) -> Result<PlayerStateSnapshot> {
        let clamped = scale.clamp(0.4, 3.0);
        self.state.write().sub_scale = clamped;
        if let Some((vt, handle)) = self.active_mpv() {
            self.mpv_set_prop_double(vt, handle, "sub-scale", clamped)?;
        }
        Ok(self.snapshot())
    }

    /// Toggles Night / Dialogue Boost dynamic audio normalization (`lavfi=[dynaudnorm=f=150:g=15:p=0.92]`)
    /// so quiet speech is boosted while loud explosions/action peaks are tamed cleanly.
    pub fn set_dialogue_boost(&self, enabled: bool) -> Result<PlayerStateSnapshot> {
        self.state.write().dialogue_boost = enabled;
        if let Some((vt, handle)) = self.active_mpv() {
            let filter_spec = if enabled {
                "lavfi=[dynaudnorm=f=150:g=15:p=0.92]"
            } else {
                ""
            };
            self.mpv_set_prop_str(vt, handle, "af", filter_spec)?;
        }
        Ok(self.snapshot())
    }

    /// Captures the current video frame via `libmpv` `screenshot-to-file` into `screenshots_dir`.
    pub fn take_screenshot_to_dir(
        &self,
        screenshots_dir: &Path,
        include_subtitles: bool,
    ) -> Result<PathBuf> {
        std::fs::create_dir_all(screenshots_dir).with_context(|| {
            format!(
                "Cannot create screenshot folder: {}",
                screenshots_dir.display()
            )
        })?;

        let (active, title, pos_ms) = {
            let st = self.state.read();
            (st.active, st.title.clone(), st.position_ms)
        };
        if !active {
            anyhow::bail!("No active video playback to screenshot");
        }

        let raw_stem = title
            .as_deref()
            .and_then(|t| Path::new(t).file_stem().map(|s| s.to_string_lossy().to_string()))
            .unwrap_or_else(|| "StreamVault".to_string());
        let clean_stem: String = raw_stem
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                    c
                } else {
                    '_'
                }
            })
            .take(48)
            .collect();
        let total_secs = (pos_ms.max(0) / 1000) as u64;
        let hh = total_secs / 3600;
        let mm = (total_secs % 3600) / 60;
        let ss = total_secs % 60;
        let ts_stamp = chrono::Utc::now().format("%Y%m%d_%H%M%S_%3f");
        let file_name = format!(
            "{}_{:02}h{:02}m{:02}s_{}.png",
            if clean_stem.is_empty() {
                "StreamVault"
            } else {
                &clean_stem
            },
            hh,
            mm,
            ss,
            ts_stamp
        );
        let out_path = screenshots_dir.join(file_name);

        if let Some((vt, handle)) = self.active_mpv() {
            let c_cmd = CString::new("screenshot-to-file")?;
            let c_path = CString::new(out_path.to_string_lossy().as_bytes())?;
            let c_mode = CString::new(if include_subtitles {
                "subtitles"
            } else {
                "video"
            })?;
            let args = [
                c_cmd.as_ptr(),
                c_path.as_ptr(),
                c_mode.as_ptr(),
                std::ptr::null(),
            ];
            let rc = unsafe { (vt.command)(handle, args.as_ptr()) };
            if rc < 0 {
                anyhow::bail!("libmpv screenshot-to-file failed: {}", vt.err_text(rc));
            }
        } else {
            anyhow::bail!("libmpv engine is not initialized");
        }

        Ok(out_path)
    }

    pub fn sync_smtc(&self, has_prev: bool, has_next: bool) {
        #[cfg(windows)]
        {
            let (active, paused, title) = {
                let st = self.state.read();
                (st.active, st.paused, st.title.clone())
            };
            if let Some(ref host) = *self.win32_host.lock() {
                host.update_smtc(active, paused, title.as_deref(), has_prev, has_next);
            }
        }
        #[cfg(not(windows))]
        {
            let _ = (has_prev, has_next);
        }
    }

    /// Sets video aspect ratio: `"auto"`, `"16:9"`, `"4:3"`, `"21:9"`, `"2.35:1"`, `"1:1"`, `"stretch"`
    pub fn set_aspect_ratio(&self, ratio: &str) -> Result<PlayerStateSnapshot> {
        self.state.write().aspect_ratio = ratio.to_string();
        if let Some((vt, handle)) = self.active_mpv() {
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
                let (vis, x, y, w, h) = *self.last_viewport.lock();
                if vis && w > 2 && h > 2 {
                    Some((x, y, w, h))
                } else {
                    None
                }
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

    pub fn show_screenshot_toast(
        &self,
        filename: &str,
        saved_path: &Path,
        fallback_dir: &Path,
        scale: f64,
    ) -> bool {
        #[cfg(windows)]
        {
            if let Some(ref host) = *self.win32_host.lock() {
                return host.show_screenshot_toast(filename, saved_path, fallback_dir, scale);
            }
            false
        }
        #[cfg(not(windows))]
        {
            let _ = (filename, saved_path, fallback_dir, scale);
            false
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

/// Discovers stem-matching external subtitle files (`.srt`, `.ass`, `.ssa`, `.vtt`, `.sub`)
/// sitting in the same local folder as `video_path` (including language suffixes such as `Movie.en.srt`).
pub fn discover_local_sidecar_subtitles(video_path: &Path) -> Vec<PathBuf> {
    let Some(parent) = video_path.parent() else {
        return Vec::new();
    };
    let Some(video_stem) = video_path
        .file_stem()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
    else {
        return Vec::new();
    };

    let Ok(entries) = std::fs::read_dir(parent) else {
        return Vec::new();
    };

    let mut matches = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() || path == video_path {
            continue;
        }
        let Some(ext) = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
        else {
            continue;
        };
        if !matches!(ext.as_str(), "srt" | "ass" | "ssa" | "vtt" | "sub") {
            continue;
        }
        let Some(sub_stem) = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_ascii_lowercase())
        else {
            continue;
        };
        if sub_stem == video_stem
            || sub_stem
                .strip_prefix(&video_stem)
                .is_some_and(|rest| rest.starts_with('.') || rest.starts_with('_') || rest.starts_with('-'))
        {
            matches.push(path);
        }
    }
    matches.sort();
    matches
}
