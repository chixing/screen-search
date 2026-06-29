#![windows_subsystem = "windows"]
#![allow(unsafe_op_in_unsafe_fn)]
#![allow(unused_must_use)]

mod matcher;

use std::backtrace::Backtrace;
use std::env;
use std::ffi::c_void;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::mem::{size_of, zeroed};
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use matcher::{
    Candidate, HintContext, Word, build_text_candidates, norm, resolve_selector_matches,
};
use once_cell::sync::OnceCell;
use parking_lot::Mutex;
use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::DataWriter;
use windows::Win32::Foundation::{
    BOOL, COLORREF, CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HINSTANCE, HWND, LPARAM,
    LRESULT, POINT, RECT, SIZE, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, BeginPaint,
    BitBlt, CLIP_DEFAULT_PRECIS, CreateBitmap, CreateCompatibleBitmap, CreateCompatibleDC,
    CreateDIBSection, CreateFontW, CreateSolidBrush, DEFAULT_CHARSET, DEFAULT_QUALITY,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, EndPaint, EnumDisplayMonitors, FW_BOLD, FillRect,
    GetDC, GetDIBits, GetMonitorInfoW, HBITMAP, HBRUSH, HDC, HFONT, HMONITOR, MONITORINFO,
    OUT_DEFAULT_PRECIS, ReleaseDC, SRCCOPY, SelectObject, SetBkColor, SetBkMode, SetTextColor,
    TRANSPARENT, TextOutW,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{
    AttachThreadInput, CreateEventW, CreateMutexW, EVENT_MODIFY_STATE, GetCurrentThreadId,
    INFINITE, OpenEventW, SetEvent, WaitForSingleObject,
};
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_RIGHTDOWN,
    MOUSEEVENTF_RIGHTUP, MOUSEINPUT, SendInput, SetFocus, VIRTUAL_KEY, VK_CONTROL, VK_ESCAPE,
    VK_F5, VK_RETURN, VK_SHIFT, VK_TAB,
};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, BringWindowToTop, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CreateIconIndirect,
    CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow, DispatchMessageW,
    EN_CHANGE, FindWindowW, GA_ROOT, GetAncestor, GetClientRect, GetCursorPos, GetForegroundWindow,
    GetMessageW, GetSystemMetrics, GetWindowRect, GetWindowTextLengthW, GetWindowTextW,
    GetWindowThreadProcessId, HICON, HMENU, ICONINFO, IDC_ARROW, IsWindow, LoadCursorW, MF_CHECKED,
    MF_SEPARATOR, MF_STRING, MF_UNCHECKED, MSG, MoveWindow, PostMessageW, PostQuitMessage,
    RegisterClassW, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
    SW_HIDE, SW_SHOW, SWP_NOZORDER, SendMessageW, SetCursorPos, SetForegroundWindow, SetTimer,
    SetWindowPos, ShowWindow, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, TranslateMessage,
    ULW_ALPHA, UpdateLayeredWindow, WINDOW_EX_STYLE, WINDOW_STYLE, WM_ACTIVATE, WM_APP, WM_COMMAND,
    WM_CONTEXTMENU, WM_CREATE, WM_CTLCOLOREDIT, WM_CTLCOLORSTATIC, WM_DESTROY, WM_KEYDOWN,
    WM_LBUTTONDBLCLK, WM_LBUTTONUP, WM_PAINT, WM_RBUTTONUP, WM_SETFONT, WM_TIMER, WNDCLASSW,
    WS_BORDER, WS_CHILD, WS_CLIPSIBLINGS, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP, WS_VISIBLE, WindowFromPoint,
};
use windows::core::{Error, PCWSTR, Result, w};

const EVENT_NAME: &str = "ScreenSearchRustToggleEvent";
const EVENT_ALL_NAME: &str = "ScreenSearchRustToggleAllEvent";
const EVENT_QUIT_NAME: &str = "ScreenSearchRustQuitEvent";
const MUTEX_NAME: &str = "ScreenSearchRustSingletonMutex";

const ALL_MONITOR_OCR_PASSES: &[(f32, &str)] = &[(1.0, "fast"), (2.0, "medium"), (3.0, "high")];
const HIGH_CONTRAST_OCR_MIN_SCALE: f32 = 2.0;
const DEFAULT_OVERLAY_ENABLED: bool = true;
const OVERLAY_TEST_SIZE: (i32, i32) = (360, 180);

const THEME_BG: (u8, u8, u8) = (17, 24, 39);
const THEME_PANEL: (u8, u8, u8) = (55, 65, 81);
const THEME_TEXT: (u8, u8, u8) = (249, 250, 251);
const THEME_PRIMARY: (u8, u8, u8) = (34, 197, 94);
const THEME_ACCENT: (u8, u8, u8) = (251, 146, 60);
const THEME_STATUS: (u8, u8, u8) = (253, 186, 116);
const THEME_SELECTED: (u8, u8, u8) = (34, 197, 94);
const THEME_INK: (u8, u8, u8) = (15, 23, 42);
const THEME_LABEL_BG: (u8, u8, u8) = (15, 23, 42);
const THEME_LABEL_BORDER: (u8, u8, u8) = (0, 0, 0);
const POPUP_W: i32 = 360;
const POPUP_H: i32 = 72;
const POPUP_PAD: i32 = 10;
const EDIT_H: i32 = 26;
const STATUS_Y: i32 = 42;
const STATUS_H: i32 = 18;
const VK_A_CODE: u16 = 0x41;
const EM_SETSEL: u32 = 0x00B1;

#[derive(Clone, Copy)]
enum ConfirmAction {
    Move,
    LeftClick,
    RightClick,
}

#[derive(Clone, Copy)]
enum MouseButton {
    Left,
    Right,
}

#[derive(Clone, Copy)]
enum OcrVariant {
    Raw,
    HighContrast,
}

const WM_TOGGLE: u32 = WM_APP + 1;
const WM_TOGGLE_ALL: u32 = WM_APP + 2;
const WM_SNAPSHOT: u32 = WM_APP + 3;
const WM_CAPTURE_DONE: u32 = WM_APP + 4;
const WM_CAPTURE_FAILED: u32 = WM_APP + 5;
const WM_QUIT_APP: u32 = WM_APP + 6;
const WM_TRAY: u32 = WM_APP + 7;
const TIMER_FILTER: usize = 1;
const TIMER_REFOCUS: usize = 2;
const TRAY_UID: u32 = 1;
const MENU_OPEN: u32 = 100;
const MENU_SCAN_ALL: u32 = 101;
const MENU_UPSCALE: u32 = 102;
const MENU_OVERLAY: u32 = 103;
const MENU_QUIT: u32 = 199;
const APP_DIR_NAME: &str = "ScreenSearch";
const CONFIG_FILE_NAME: &str = "config.ini";
const CRASH_LOG_FILE_NAME: &str = "screen-search-rs-crash.log";
const TRACE_LOG_FILE_NAME: &str = "screen-search-rs-trace.log";
const DIAGNOSTICS_FILE_NAME: &str = "screen-search-rs-diagnostics.txt";

static APP: OnceCell<Arc<Mutex<App>>> = OnceCell::new();
static BG_BRUSH: AtomicIsize = AtomicIsize::new(0);
static EDIT_BRUSH: AtomicIsize = AtomicIsize::new(0);
static TRACE_ENABLED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, Debug)]
struct Settings {
    scan_all: bool,
    upscale: bool,
    overlay_enabled: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            scan_all: true,
            upscale: true,
            overlay_enabled: DEFAULT_OVERLAY_ENABLED,
        }
    }
}

fn appdata_dir() -> PathBuf {
    env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(env::temp_dir)
        .join(APP_DIR_NAME)
}

fn local_appdata_dir() -> PathBuf {
    env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(env::temp_dir)
        .join(APP_DIR_NAME)
}

fn config_path() -> PathBuf {
    appdata_dir().join(CONFIG_FILE_NAME)
}

fn local_data_path(file_name: &str) -> PathBuf {
    local_appdata_dir().join(file_name)
}

fn read_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn load_settings() -> Settings {
    let mut settings = Settings::default();
    let Ok(text) = fs::read_to_string(config_path()) else {
        return settings;
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let Some(value) = read_bool(value) else {
            continue;
        };
        match key.trim() {
            "scan_all" => settings.scan_all = value,
            "upscale" => settings.upscale = value,
            "overlay_enabled" => settings.overlay_enabled = value,
            _ => {}
        }
    }
    settings
}

fn save_settings(settings: Settings) {
    let path = config_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let text = format!(
        "scan_all={}\nupscale={}\noverlay_enabled={}\n",
        settings.scan_all, settings.upscale, settings.overlay_enabled
    );
    let _ = fs::write(path, text);
}

fn append_crash_log(context: &str, details: &str) {
    let path = local_data_path(CRASH_LOG_FILE_NAME);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "\n=== {context} ===");
        let _ = writeln!(file, "{details}");
    }
}

fn trace_log(message: impl AsRef<str>) {
    if !TRACE_ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let path = local_data_path(TRACE_LOG_FILE_NAME);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or_default();
        let _ = writeln!(
            file,
            "{} pid={} {:?} {}",
            millis,
            std::process::id(),
            thread::current().id(),
            message.as_ref()
        );
    }
}

fn install_panic_log() {
    panic::set_hook(Box::new(|info| {
        append_crash_log(
            "panic",
            &format!("{info}\nBacktrace:\n{}", Backtrace::force_capture()),
        );
    }));
}

unsafe fn guarded_wndproc<F>(context: &str, fallback: F) -> LRESULT
where
    F: FnOnce() -> LRESULT,
{
    match panic::catch_unwind(AssertUnwindSafe(fallback)) {
        Ok(result) => result,
        Err(payload) => {
            let message = if let Some(s) = payload.downcast_ref::<&str>() {
                (*s).to_string()
            } else if let Some(s) = payload.downcast_ref::<String>() {
                s.clone()
            } else {
                "non-string panic payload".to_string()
            };
            append_crash_log(context, &message);
            LRESULT(0)
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Region {
    left: i32,
    top: i32,
    width: i32,
    height: i32,
}

#[derive(Clone, Debug)]
struct Monitor {
    region: Region,
}

#[derive(Clone)]
struct Shot {
    width: i32,
    height: i32,
    bgra: Vec<u8>,
}

#[derive(Clone)]
struct Snapshot {
    words: Vec<Word>,
    candidates: Vec<Candidate>,
    region: Region,
    complete: bool,
    quality: &'static str,
}

#[derive(Default)]
struct App {
    hinstance: HINSTANCE,
    hwnd: HWND,
    edit: HWND,
    status: HWND,
    overlay: HWND,
    popup_visible: bool,
    matches: Vec<Candidate>,
    selected: usize,
    snap: Option<Snapshot>,
    capturing: bool,
    capture_seq: u64,
    hint_context: Option<HintContext>,
    last_query: String,
    region: Region,
    all_words: Vec<Word>,
    scan_all: bool,
    exact: bool,
    upscale: bool,
    debug_all: bool,
    overlay_enabled: bool,
    cold_show: bool,
    listen_events: bool,
    tray_added: bool,
    bg_brush: HBRUSH,
    edit_brush: HBRUSH,
    popup_font: HFONT,
}

unsafe impl Send for App {}

fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF((r as u32) | ((g as u32) << 8) | ((b as u32) << 16))
}

fn rgb_tuple(color: (u8, u8, u8)) -> COLORREF {
    rgb(color.0, color.1, color.2)
}

fn store_theme_brushes(bg: HBRUSH, edit: HBRUSH) {
    BG_BRUSH.store(bg.0 as isize, Ordering::Relaxed);
    EDIT_BRUSH.store(edit.0 as isize, Ordering::Relaxed);
}

fn theme_bg_brush() -> HBRUSH {
    HBRUSH(BG_BRUSH.load(Ordering::Relaxed) as *mut c_void)
}

fn theme_edit_brush() -> HBRUSH {
    HBRUSH(EDIT_BRUSH.load(Ordering::Relaxed) as *mut c_void)
}

unsafe fn create_tray_icon() -> HICON {
    let size = 16_i32;
    let mut xor = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let idx = ((y * size + x) * 4) as usize;
            let in_circle = {
                let dx = x - 7;
                let dy = y - 7;
                dx * dx + dy * dy <= 49
            };
            if in_circle {
                xor[idx] = THEME_PRIMARY.2;
                xor[idx + 1] = THEME_PRIMARY.1;
                xor[idx + 2] = THEME_PRIMARY.0;
                xor[idx + 3] = 255;
            }
            if (4..=11).contains(&x) && (6..=8).contains(&y) {
                xor[idx] = THEME_ACCENT.2;
                xor[idx + 1] = THEME_ACCENT.1;
                xor[idx + 2] = THEME_ACCENT.0;
                xor[idx + 3] = 255;
            }
            if (10..=12).contains(&x) && (9..=12).contains(&y) {
                xor[idx] = THEME_INK.2;
                xor[idx + 1] = THEME_INK.1;
                xor[idx + 2] = THEME_INK.0;
                xor[idx + 3] = 255;
            }
        }
    }
    let color = CreateBitmap(size, size, 1, 32, Some(xor.as_ptr() as *const c_void));
    let and_mask = vec![0u8; 32];
    let mask = CreateBitmap(size, size, 1, 1, Some(and_mask.as_ptr() as *const c_void));
    CreateIconIndirect(&ICONINFO {
        fIcon: BOOL(1),
        xHotspot: 0,
        yHotspot: 0,
        hbmMask: mask,
        hbmColor: color,
    })
    .unwrap_or_default()
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn fill_wide<const N: usize>(dst: &mut [u16; N], text: &str) {
    for (slot, ch) in dst
        .iter_mut()
        .zip(text.encode_utf16().chain(std::iter::once(0)))
    {
        *slot = ch;
    }
}

fn set_text(hwnd: HWND, text: &str) {
    let ws = wide(text);
    unsafe {
        windows::Win32::UI::WindowsAndMessaging::SetWindowTextW(hwnd, PCWSTR(ws.as_ptr()));
    }
}

fn get_text(hwnd: HWND) -> String {
    unsafe {
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return String::new();
        }
        let mut buf = vec![0u16; len as usize + 1];
        let got = GetWindowTextW(hwnd, &mut buf);
        String::from_utf16_lossy(&buf[..got as usize])
    }
}

fn signal_event(event_name: &str) -> bool {
    let name = wide(event_name);
    unsafe {
        let Ok(h) = OpenEventW(EVENT_MODIFY_STATE, false, PCWSTR(name.as_ptr())) else {
            return false;
        };
        let ok = SetEvent(h).is_ok();
        let _ = CloseHandle(h);
        ok
    }
}

fn post_quit_window_message() -> bool {
    unsafe {
        let Ok(hwnd) = FindWindowW(w!("ScreenSearchRustPopup"), PCWSTR::null()) else {
            return false;
        };
        if hwnd.0.is_null() {
            return false;
        }
        PostMessageW(hwnd, WM_QUIT_APP, WPARAM(0), LPARAM(0)).is_ok()
    }
}

fn create_resident_events(hwnd: HWND) -> Result<()> {
    for (name, msg) in [
        (EVENT_NAME, WM_TOGGLE),
        (EVENT_ALL_NAME, WM_TOGGLE_ALL),
        (EVENT_QUIT_NAME, WM_QUIT_APP),
    ] {
        let name_w = wide(name);
        let handle = unsafe { CreateEventW(None, false, false, PCWSTR(name_w.as_ptr()))? };
        let handle_raw = handle.0 as isize;
        let hwnd_raw = hwnd.0 as isize;
        thread::spawn(move || unsafe {
            let handle = windows::Win32::Foundation::HANDLE(handle_raw as *mut c_void);
            let hwnd = HWND(hwnd_raw as *mut c_void);
            loop {
                if WaitForSingleObject(handle, INFINITE).0 == 0 {
                    let _ = PostMessageW(hwnd, msg, WPARAM(0), LPARAM(0));
                }
            }
        });
    }
    Ok(())
}

unsafe extern "system" fn popup_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    guarded_wndproc("popup_proc", || popup_proc_inner(hwnd, msg, wparam, lparam))
}

unsafe fn popup_proc_inner(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_CREATE => LRESULT(0),
        WM_PAINT => {
            let mut ps = zeroed();
            let hdc = BeginPaint(hwnd, &mut ps);
            let brush = theme_bg_brush();
            if !brush.0.is_null() {
                let mut rect = RECT::default();
                if GetClientRect(hwnd, &mut rect).is_ok() {
                    FillRect(hdc, &rect, brush);
                }
            }
            EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_CTLCOLOREDIT => {
            let brush = theme_edit_brush();
            if !brush.0.is_null() {
                let hdc = HDC(wparam.0 as *mut c_void);
                SetTextColor(hdc, rgb_tuple(THEME_TEXT));
                SetBkColor(hdc, rgb_tuple(THEME_PANEL));
                return LRESULT(brush.0 as isize);
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_CTLCOLORSTATIC => {
            let brush = theme_bg_brush();
            if !brush.0.is_null() {
                let hdc = HDC(wparam.0 as *mut c_void);
                SetTextColor(hdc, rgb_tuple(THEME_STATUS));
                SetBkColor(hdc, rgb_tuple(THEME_BG));
                return LRESULT(brush.0 as isize);
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_COMMAND => {
            let code = ((wparam.0 >> 16) & 0xffff) as u16;
            if code == EN_CHANGE as u16 {
                let _ = SetTimer(hwnd, TIMER_FILTER, 50, None);
            }
            LRESULT(0)
        }
        WM_TIMER => {
            if wparam.0 == TIMER_FILTER {
                windows::Win32::UI::WindowsAndMessaging::KillTimer(hwnd, TIMER_FILTER);
                if let Some(app) = APP.get() {
                    app.lock().do_type();
                }
            } else if wparam.0 == TIMER_REFOCUS {
                windows::Win32::UI::WindowsAndMessaging::KillTimer(hwnd, TIMER_REFOCUS);
                if let Some(app) = APP.get() {
                    app.lock().refocus_popup();
                }
            }
            LRESULT(0)
        }
        WM_KEYDOWN => {
            let vk = VIRTUAL_KEY(wparam.0 as u16);
            if let Some(app) = APP.get() {
                let mut app = app.lock();
                if is_ctrl_a(vk) {
                    app.select_all_query();
                    return LRESULT(0);
                }
                match vk {
                    VK_RETURN => {
                        app.confirm(current_confirm_action());
                        return LRESULT(0);
                    }
                    VK_ESCAPE => {
                        app.hide_popup();
                        return LRESULT(0);
                    }
                    VK_F5 => {
                        app.recapture();
                        return LRESULT(0);
                    }
                    VK_TAB => {
                        if (windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState(
                            windows::Win32::UI::Input::KeyboardAndMouse::VK_SHIFT.0 as i32,
                        ) as u16
                            & 0x8000)
                            != 0
                        {
                            app.select_prev();
                        } else {
                            app.select_next();
                        }
                        return LRESULT(0);
                    }
                    _ => {}
                }
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_ACTIVATE => {
            let _ = wparam;
            LRESULT(0)
        }
        WM_TOGGLE => {
            if let Some(app) = APP.get() {
                app.lock().toggle_popup(false);
            }
            LRESULT(0)
        }
        WM_TOGGLE_ALL => {
            if let Some(app) = APP.get() {
                app.lock().toggle_popup(true);
            }
            LRESULT(0)
        }
        WM_SNAPSHOT => {
            let boxed = Box::from_raw(lparam.0 as *mut Snapshot);
            if let Some(app) = APP.get() {
                app.lock().accept_snapshot(wparam.0 as u64, *boxed);
            }
            LRESULT(0)
        }
        WM_CAPTURE_DONE => {
            if let Some(app) = APP.get() {
                app.lock().capture_done(wparam.0 as u64);
            }
            LRESULT(0)
        }
        WM_CAPTURE_FAILED => {
            let boxed = Box::from_raw(lparam.0 as *mut String);
            if let Some(app) = APP.get() {
                app.lock().capture_failed(wparam.0 as u64, &boxed);
            }
            LRESULT(0)
        }
        WM_QUIT_APP => {
            if let Some(app) = APP.get() {
                app.lock().hide_popup();
            }
            DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_TRAY => {
            let event = lparam.0 as u32;
            if event == WM_RBUTTONUP
                || event == WM_LBUTTONUP
                || event == WM_CONTEXTMENU
                || event == WM_LBUTTONDBLCLK
            {
                if let Some(app) = APP.get() {
                    let mut app = app.lock();
                    if event == WM_LBUTTONDBLCLK {
                        app.show_popup(false);
                    } else {
                        app.show_tray_menu();
                    }
                }
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            if let Some(app) = APP.get() {
                let mut app = app.lock();
                app.remove_tray_icon();
                app.destroy_theme_resources();
            }
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe extern "system" fn overlay_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    guarded_wndproc("overlay_proc", || {
        overlay_proc_inner(hwnd, msg, wparam, lparam)
    })
}

unsafe fn overlay_proc_inner(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            let mut ps = zeroed();
            let hdc = BeginPaint(hwnd, &mut ps);
            EndPaint(hwnd, &ps);
            let _ = hdc;
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

impl App {
    fn new(hinstance: HINSTANCE, cold_show: bool, settings: Settings, listen_events: bool) -> Self {
        Self {
            hinstance,
            scan_all: settings.scan_all,
            exact: false,
            upscale: settings.upscale,
            debug_all: false,
            overlay_enabled: settings.overlay_enabled,
            cold_show,
            listen_events,
            ..Default::default()
        }
    }

    fn current_settings(&self) -> Settings {
        Settings {
            scan_all: self.scan_all,
            upscale: self.upscale,
            overlay_enabled: self.overlay_enabled,
        }
    }

    fn save_current_settings(&self) {
        save_settings(self.current_settings());
    }

    fn create_windows(app: Arc<Mutex<App>>) -> Result<()> {
        let hinstance = app.lock().hinstance;
        unsafe {
            let popup_class = w!("ScreenSearchRustPopup");
            let overlay_class = w!("ScreenSearchRustOverlay");
            let cursor = LoadCursorW(None, IDC_ARROW)?;
            let bg_brush = CreateSolidBrush(rgb_tuple(THEME_BG));
            let edit_brush = CreateSolidBrush(rgb_tuple(THEME_PANEL));
            store_theme_brushes(bg_brush, edit_brush);
            let popup_font = CreateFontW(
                -14,
                0,
                0,
                0,
                500,
                0,
                0,
                0,
                DEFAULT_CHARSET.0 as u32,
                OUT_DEFAULT_PRECIS.0 as u32,
                CLIP_DEFAULT_PRECIS.0 as u32,
                DEFAULT_QUALITY.0 as u32,
                0,
                w!("Segoe UI"),
            );
            RegisterClassW(&WNDCLASSW {
                hCursor: cursor,
                hInstance: hinstance,
                lpszClassName: popup_class,
                style: CS_HREDRAW | CS_VREDRAW,
                hbrBackground: bg_brush,
                lpfnWndProc: Some(popup_proc),
                ..Default::default()
            });
            RegisterClassW(&WNDCLASSW {
                hCursor: cursor,
                hInstance: hinstance,
                lpszClassName: overlay_class,
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(overlay_proc),
                ..Default::default()
            });

            let hwnd = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
                popup_class,
                w!("Screen Search"),
                WS_POPUP | WS_BORDER,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                POPUP_W,
                POPUP_H,
                None,
                None,
                hinstance,
                None,
            )?;

            let edit = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("EDIT"),
                w!(""),
                WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS | WINDOW_STYLE(0x0080), // ES_AUTOHSCROLL
                POPUP_PAD,
                POPUP_PAD,
                POPUP_W - POPUP_PAD * 2,
                EDIT_H,
                hwnd,
                HMENU(1001usize as *mut c_void),
                hinstance,
                None,
            )?;
            SendMessageW(edit, WM_SETFONT, WPARAM(popup_font.0 as usize), LPARAM(1));
            let status = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("Type to search."),
                WS_CHILD | WS_VISIBLE,
                POPUP_PAD,
                STATUS_Y,
                POPUP_W - POPUP_PAD * 2,
                STATUS_H,
                hwnd,
                HMENU(1002usize as *mut c_void),
                hinstance,
                None,
            )?;
            SendMessageW(status, WM_SETFONT, WPARAM(popup_font.0 as usize), LPARAM(1));

            {
                let mut a = app.lock();
                a.hwnd = hwnd;
                a.edit = edit;
                a.status = status;
                a.bg_brush = bg_brush;
                a.edit_brush = edit_brush;
                a.popup_font = popup_font;
                if a.listen_events {
                    a.add_tray_icon();
                }
            }
            if app.lock().listen_events {
                create_resident_events(hwnd)?;
            }
            if app.lock().cold_show {
                PostMessageW(hwnd, WM_TOGGLE, WPARAM(0), LPARAM(0))?;
            }
        }
        Ok(())
    }

    fn message_loop() {
        unsafe {
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).into() {
                if let Some(app) = APP.get() {
                    let mut app = app.lock();
                    if msg.hwnd == app.edit && msg.message == WM_KEYDOWN {
                        if app.handle_key(VIRTUAL_KEY(msg.wParam.0 as u16)) {
                            continue;
                        }
                    }
                }
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }

    fn handle_key(&mut self, vk: VIRTUAL_KEY) -> bool {
        if is_ctrl_a(vk) {
            self.select_all_query();
            return true;
        }
        match vk {
            VK_RETURN => {
                self.confirm(current_confirm_action());
                true
            }
            VK_ESCAPE => {
                self.hide_popup();
                true
            }
            VK_F5 => {
                self.recapture();
                true
            }
            VK_TAB => unsafe {
                if (windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState(
                    windows::Win32::UI::Input::KeyboardAndMouse::VK_SHIFT.0 as i32,
                ) as u16
                    & 0x8000)
                    != 0
                {
                    self.select_prev();
                } else {
                    self.select_next();
                }
                true
            },
            _ => false,
        }
    }

    fn select_all_query(&self) {
        unsafe {
            if !self.edit.0.is_null() {
                SetFocus(self.edit);
                SendMessageW(self.edit, EM_SETSEL, WPARAM(0), LPARAM(-1));
            }
        }
    }

    fn tray_data(&self) -> NOTIFYICONDATAW {
        let mut data = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: TRAY_UID,
            ..Default::default()
        };
        fill_wide(&mut data.szTip, "Screen Search");
        data
    }

    fn add_tray_icon(&mut self) {
        if self.tray_added || self.hwnd.0.is_null() {
            return;
        }
        unsafe {
            let mut data = self.tray_data();
            data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
            data.uCallbackMessage = WM_TRAY;
            data.hIcon = create_tray_icon();
            if Shell_NotifyIconW(NIM_ADD, &data).as_bool() {
                self.tray_added = true;
            }
        }
    }

    fn remove_tray_icon(&mut self) {
        if !self.tray_added {
            return;
        }
        unsafe {
            let data = self.tray_data();
            let _ = Shell_NotifyIconW(NIM_DELETE, &data);
        }
        self.tray_added = false;
    }

    fn destroy_theme_resources(&mut self) {
        unsafe {
            if !self.bg_brush.0.is_null() {
                DeleteObject(self.bg_brush);
                self.bg_brush = HBRUSH(std::ptr::null_mut());
            }
            if !self.edit_brush.0.is_null() {
                DeleteObject(self.edit_brush);
                self.edit_brush = HBRUSH(std::ptr::null_mut());
            }
            store_theme_brushes(self.bg_brush, self.edit_brush);
            if !self.popup_font.0.is_null() {
                DeleteObject(self.popup_font);
                self.popup_font = HFONT(std::ptr::null_mut());
            }
        }
    }

    fn menu_flags(checked: bool) -> windows::Win32::UI::WindowsAndMessaging::MENU_ITEM_FLAGS {
        if checked {
            MF_STRING | MF_CHECKED
        } else {
            MF_STRING | MF_UNCHECKED
        }
    }

    unsafe fn append_menu_item(
        menu: HMENU,
        id: u32,
        label: &str,
        flags: windows::Win32::UI::WindowsAndMessaging::MENU_ITEM_FLAGS,
    ) {
        let text = wide(label);
        let _ = AppendMenuW(menu, flags, id as usize, PCWSTR(text.as_ptr()));
    }

    fn show_tray_menu(&mut self) {
        unsafe {
            let Ok(menu) = CreatePopupMenu() else {
                return;
            };
            Self::append_menu_item(menu, MENU_OPEN, "Open Search", MF_STRING);
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            Self::append_menu_item(
                menu,
                MENU_SCAN_ALL,
                "Scan all monitors",
                Self::menu_flags(self.scan_all),
            );
            Self::append_menu_item(
                menu,
                MENU_UPSCALE,
                "Upscale OCR",
                Self::menu_flags(self.upscale),
            );
            Self::append_menu_item(
                menu,
                MENU_OVERLAY,
                "Show overlay",
                Self::menu_flags(self.overlay_enabled),
            );
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            Self::append_menu_item(menu, MENU_QUIT, "Quit", MF_STRING);

            let mut pt = POINT::default();
            if GetCursorPos(&mut pt).is_err() {
                DestroyMenu(menu);
                return;
            }
            let _ = SetForegroundWindow(self.hwnd);
            let command = TrackPopupMenu(
                menu,
                TPM_RIGHTBUTTON | TPM_RETURNCMD,
                pt.x,
                pt.y,
                0,
                self.hwnd,
                None,
            )
            .0 as u32;
            DestroyMenu(menu);

            match command {
                MENU_OPEN => self.show_popup(false),
                MENU_SCAN_ALL => {
                    self.scan_all = !self.scan_all;
                    self.save_current_settings();
                    self.recapture_if_open();
                }
                MENU_UPSCALE => {
                    self.upscale = !self.upscale;
                    self.save_current_settings();
                    self.recapture_if_open();
                }
                MENU_OVERLAY => {
                    self.overlay_enabled = !self.overlay_enabled;
                    self.save_current_settings();
                    if self.overlay_enabled {
                        if self.popup_visible && self.snap.is_some() {
                            self.live_filter();
                        }
                    } else {
                        self.close_overlay();
                    }
                }
                MENU_QUIT => {
                    self.hide_popup();
                    DestroyWindow(self.hwnd);
                }
                _ => {}
            }
        }
    }

    fn recapture_if_open(&mut self) {
        if self.popup_visible {
            self.recapture();
        }
    }

    fn toggle_popup(&mut self, force_all: bool) {
        if self.popup_visible {
            self.hide_popup();
        } else {
            self.show_popup(force_all);
        }
    }

    fn show_popup(&mut self, force_all: bool) {
        self.selected = 0;
        self.last_query.clear();
        self.hint_context = None;
        set_text(self.edit, "");
        set_text(self.status, "Type to search. Refreshing screen...");
        self.position_popup();
        unsafe {
            ShowWindow(self.hwnd, SW_SHOW);
            BringWindowToTop(self.hwnd);
            self.force_foreground(self.hwnd);
            SetFocus(self.edit);
            let _ = SetTimer(self.hwnd, TIMER_REFOCUS, 125, None);
        }
        self.popup_visible = true;
        self.start_capture(true, force_all);
    }

    fn refocus_popup(&mut self) {
        if !self.popup_visible {
            return;
        }
        unsafe {
            BringWindowToTop(self.hwnd);
            self.force_foreground(self.hwnd);
            SetFocus(self.edit);
        }
        trace_log("refocus_popup: edit focused");
    }

    fn hide_popup(&mut self) {
        self.capture_seq = self.capture_seq.wrapping_add(1);
        self.capturing = false;
        self.hint_context = None;
        self.matches.clear();
        self.close_overlay();
        unsafe {
            ShowWindow(self.hwnd, SW_HIDE);
        }
        self.popup_visible = false;
    }

    fn position_popup(&self) {
        let mon = cursor_monitor().unwrap_or(Monitor {
            region: virtual_region(),
        });
        let w = POPUP_W;
        let h = POPUP_H;
        let x = mon.region.left + (mon.region.width - w) / 2;
        let y = mon.region.top + mon.region.height - h - 72;
        unsafe {
            SetWindowPos(self.hwnd, None, x, y, w, h, SWP_NOZORDER);
        }
    }

    unsafe fn force_foreground(&self, hwnd: HWND) {
        let fg = GetForegroundWindow();
        let mut pid = 0;
        let target_tid = if !fg.0.is_null() {
            GetWindowThreadProcessId(fg, Some(&mut pid))
        } else {
            0
        };
        let our_tid = GetCurrentThreadId();
        let attached = target_tid != 0 && target_tid != our_tid;
        if attached {
            let _ = AttachThreadInput(our_tid, target_tid, true);
        }
        let _ = BringWindowToTop(hwnd);
        let _ = SetForegroundWindow(hwnd);
        if attached {
            let _ = AttachThreadInput(our_tid, target_tid, false);
        }
    }

    fn do_type(&mut self) {
        trace_log("do_type: enter");
        if !self.popup_visible {
            trace_log("do_type: popup not visible");
            return;
        }
        let text = get_text(self.edit).trim().to_string();
        trace_log(format!("do_type: text_len={} text={:?}", text.len(), text));
        if text == self.last_query {
            trace_log("do_type: unchanged");
            return;
        }
        self.last_query = text.clone();
        if text.is_empty() {
            self.close_overlay();
            set_text(self.status, "Type to search.");
            trace_log("do_type: empty");
            return;
        }
        if self.snap.is_none() {
            if !self.capturing {
                trace_log("do_type: starting capture");
                self.start_capture(false, false);
            } else {
                set_text(self.status, "Reading screen...");
                trace_log("do_type: capture already running");
            }
            return;
        }
        trace_log("do_type: live_filter");
        self.live_filter();
        trace_log("do_type: exit");
    }

    fn start_capture(&mut self, force: bool, force_all: bool) {
        if self.capturing && !force {
            return;
        }
        self.capture_seq = self.capture_seq.wrapping_add(1);
        let seq = self.capture_seq;
        self.capturing = true;
        let hwnd_raw = self.hwnd.0 as isize;
        let all_mon = self.scan_all || force_all;
        let active_scale = if self.upscale { 2.0 } else { 1.0 };
        thread::spawn(move || {
            let hwnd = HWND(hwnd_raw as *mut c_void);
            if let Err(err) = unsafe { RoInitialize(RO_INIT_MULTITHREADED) } {
                post_capture_failed(hwnd, seq, format!("WinRT init failed: {err:?}"));
                return;
            }
            match capture_snapshots(all_mon, active_scale, hwnd, seq) {
                Ok(()) => post_capture_done(hwnd, seq),
                Err(err) => post_capture_failed(hwnd, seq, err),
            }
        });
    }

    fn accept_snapshot(&mut self, seq: u64, snap: Snapshot) {
        trace_log(format!(
            "accept_snapshot: seq={} words={} candidates={} complete={} quality={}",
            seq,
            snap.words.len(),
            snap.candidates.len(),
            snap.complete,
            snap.quality
        ));
        if seq != self.capture_seq {
            trace_log("accept_snapshot: stale");
            return;
        }
        self.snap = Some(snap.clone());
        self.capturing =
            !snap.complete || (snap.quality != "high" && self.scan_all && self.upscale);
        if !self.popup_visible {
            return;
        }
        if !get_text(self.edit).trim().is_empty() {
            trace_log("accept_snapshot: live_filter");
            self.live_filter();
            trace_log("accept_snapshot: live_filter done");
            return;
        }
        if snap.quality == "high" {
            set_text(self.status, "Type to search.");
        } else if snap.complete {
            set_text(self.status, "Type to search.");
        } else if snap.quality == "fast" {
            set_text(
                self.status,
                "Type to search. 1x scan ready; refining at 2x...",
            );
        } else {
            set_text(
                self.status,
                "Type to search. 2x scan ready; refining at 3x...",
            );
        }
    }

    fn capture_done(&mut self, seq: u64) {
        if seq == self.capture_seq {
            self.capturing = false;
        }
    }

    fn capture_failed(&mut self, seq: u64, err: &str) {
        if seq == self.capture_seq && self.popup_visible {
            self.capturing = false;
            set_text(self.status, &format!("OCR error: {err}"));
        }
    }

    fn live_filter(&mut self) {
        trace_log("live_filter: enter");
        let Some(snap) = &self.snap else {
            trace_log("live_filter: no snap");
            return;
        };
        let text = get_text(self.edit).trim().to_string();
        let q = norm(&text);
        trace_log(format!(
            "live_filter: q={:?} words={} candidates={}",
            q,
            snap.words.len(),
            snap.candidates.len()
        ));
        if q.is_empty() {
            self.close_overlay();
            self.hint_context = None;
            trace_log("live_filter: empty q");
            return;
        }
        let (mut matches, ctx, hint_suffix) =
            resolve_selector_matches(&q, &snap.candidates, self.hint_context.as_ref(), self.exact);
        trace_log(format!(
            "live_filter: resolved matches={} suffix={:?}",
            matches.len(),
            hint_suffix
        ));
        self.hint_context = ctx;

        let popup_rect = window_rect(self.hwnd);
        let mut filtered = Vec::new();
        for mut m in matches.drain(..) {
            let sx = (m.x + m.w / 2.0 + snap.region.left as f32).round() as i32;
            let sy = (m.y + m.h / 2.0 + snap.region.top as f32).round() as i32;
            if point_in_rect(sx, sy, popup_rect) {
                continue;
            }
            m.sx = sx;
            m.sy = sy;
            m.hint_typed = hint_suffix.clone();
            filtered.push(m);
        }

        self.matches = filtered;
        self.region = snap.region;
        self.all_words = snap.words.clone();
        if self.selected >= self.matches.len() {
            self.selected = 0;
        }
        if self.matches.is_empty() && !self.debug_all {
            self.close_overlay();
            set_text(
                self.status,
                &format!(
                    "No match for '{text}' ({} words read).",
                    self.all_words.len()
                ),
            );
            trace_log("live_filter: no matches");
            return;
        }
        trace_log("live_filter: refresh_overlay");
        self.refresh_overlay();
        trace_log("live_filter: refresh_overlay done");
        if self.matches.is_empty() {
            set_text(
                self.status,
                &format!(
                    "No match -- showing all {} OCR words.",
                    self.all_words.len()
                ),
            );
        } else {
            let mode = if hint_suffix.is_empty() {
                "text"
            } else {
                "selector"
            };
            set_text(
                self.status,
                &format!(
                    "{} {mode} match(es) for '{text}'. Enter move; Ctrl+Enter click; Ctrl+Shift+Enter right.",
                    self.matches.len()
                ),
            );
        }
        trace_log("live_filter: exit");
    }

    fn select_next(&mut self) {
        if !self.matches.is_empty() {
            self.selected = (self.selected + 1) % self.matches.len();
            if !self.overlay.0.is_null() {
                self.refresh_overlay();
            }
        }
    }

    fn select_prev(&mut self) {
        if !self.matches.is_empty() {
            self.selected = if self.selected == 0 {
                self.matches.len() - 1
            } else {
                self.selected - 1
            };
            if !self.overlay.0.is_null() {
                self.refresh_overlay();
            }
        }
    }

    fn confirm(&mut self, action: ConfirmAction) {
        if self.matches.is_empty() {
            return;
        }
        let m = self.matches[self.selected].clone();
        self.snap = None;
        self.hide_popup();
        thread::spawn(move || unsafe {
            match action {
                ConfirmAction::Move => move_cursor_to(m.sx, m.sy),
                ConfirmAction::LeftClick => click_at(m.sx, m.sy, MouseButton::Left),
                ConfirmAction::RightClick => click_at(m.sx, m.sy, MouseButton::Right),
            }
        });
    }

    fn recapture(&mut self) {
        self.snap = None;
        self.hint_context = None;
        self.close_overlay();
        self.start_capture(true, false);
    }

    fn ensure_overlay(&mut self) {
        if !self.overlay_enabled {
            self.close_overlay();
            return;
        }
        unsafe {
            if !self.overlay.0.is_null() && IsWindow(self.overlay).as_bool() {
                MoveWindow(
                    self.overlay,
                    self.region.left,
                    self.region.top,
                    self.region.width,
                    self.region.height,
                    true,
                );
                return;
            }
            let Ok(hwnd) = CreateWindowExW(
                WS_EX_TOPMOST
                    | WS_EX_TOOLWINDOW
                    | WS_EX_LAYERED
                    | WS_EX_TRANSPARENT
                    | WS_EX_NOACTIVATE,
                w!("ScreenSearchRustOverlay"),
                w!("Screen Search Overlay"),
                WS_POPUP | WS_VISIBLE,
                self.region.left,
                self.region.top,
                self.region.width,
                self.region.height,
                None,
                None,
                self.hinstance,
                None,
            ) else {
                return;
            };
            self.overlay = hwnd;
        }
    }

    fn refresh_overlay(&mut self) {
        trace_log(format!(
            "refresh_overlay: enter enabled={} region={}x{} matches={}",
            self.overlay_enabled,
            self.region.width,
            self.region.height,
            self.matches.len()
        ));
        if !self.overlay_enabled {
            self.close_overlay();
            trace_log("refresh_overlay: disabled");
            return;
        }
        if self.region.width <= 0 || self.region.height <= 0 {
            self.close_overlay();
            trace_log("refresh_overlay: invalid region");
            return;
        }
        self.ensure_overlay();
        if self.overlay.0.is_null() {
            trace_log("refresh_overlay: no overlay hwnd");
            return;
        }

        trace_log("refresh_overlay: allocating pixels");
        let mut pixels = vec![0u8; (self.region.width * self.region.height * 4) as usize];
        if self.debug_all {
            for w in &self.all_words {
                draw_rect_outline(
                    &mut pixels,
                    self.region.width,
                    self.region.height,
                    w.x.round() as i32,
                    w.y.round() as i32,
                    (w.x + w.w).round() as i32,
                    (w.y + w.h).round() as i32,
                    1,
                    (THEME_PRIMARY.0, THEME_PRIMARY.1, THEME_PRIMARY.2, 150),
                );
            }
        }

        for (i, m) in self.matches.iter().enumerate() {
            let selected = i == self.selected;
            let color = if selected {
                (THEME_SELECTED.0, THEME_SELECTED.1, THEME_SELECTED.2, 255)
            } else {
                (THEME_ACCENT.0, THEME_ACCENT.1, THEME_ACCENT.2, 245)
            };
            draw_rect_outline(
                &mut pixels,
                self.region.width,
                self.region.height,
                m.x.round() as i32 - 4,
                m.y.round() as i32 - 4,
                (m.x + m.w).round() as i32 + 4,
                (m.y + m.h).round() as i32 + 4,
                if selected { 5 } else { 3 },
                (THEME_INK.0, THEME_INK.1, THEME_INK.2, 230),
            );
            draw_rect_outline(
                &mut pixels,
                self.region.width,
                self.region.height,
                m.x.round() as i32 - 3,
                m.y.round() as i32 - 3,
                (m.x + m.w).round() as i32 + 3,
                (m.y + m.h).round() as i32 + 3,
                if selected { 3 } else { 2 },
                color,
            );
            if !m.hint.is_empty() {
                draw_label_background(&mut pixels, self.region.width, self.region.height, m);
            }
        }

        trace_log("refresh_overlay: update_layered_overlay");
        if unsafe {
            update_layered_overlay(
                self.overlay,
                self.region,
                &pixels,
                &self.matches,
                self.selected,
            )
        }
        .is_ok()
        {
            trace_log("refresh_overlay: update done");
            if self.popup_visible {
                self.refocus_popup();
            }
        } else {
            trace_log("refresh_overlay: update failed");
            self.close_overlay();
        }
        trace_log("refresh_overlay: exit");
    }

    fn close_overlay(&mut self) {
        unsafe {
            if !self.overlay.0.is_null() && IsWindow(self.overlay).as_bool() {
                DestroyWindow(self.overlay);
            }
        }
        self.overlay = HWND(std::ptr::null_mut());
    }
}

fn premul_channel(channel: u8, alpha: u8) -> u8 {
    ((channel as u16 * alpha as u16 + 127) / 255) as u8
}

fn put_pixel(buf: &mut [u8], width: i32, height: i32, x: i32, y: i32, rgba: (u8, u8, u8, u8)) {
    if x < 0 || y < 0 || x >= width || y >= height {
        return;
    }
    let idx = ((y * width + x) * 4) as usize;
    let (r, g, b, a) = rgba;
    buf[idx] = premul_channel(b, a);
    buf[idx + 1] = premul_channel(g, a);
    buf[idx + 2] = premul_channel(r, a);
    buf[idx + 3] = a;
}

fn draw_filled_rect(
    buf: &mut [u8],
    width: i32,
    height: i32,
    x1: i32,
    y1: i32,
    x2: i32,
    y2: i32,
    rgba: (u8, u8, u8, u8),
) {
    let left = x1.max(0).min(width);
    let right = x2.max(0).min(width);
    let top = y1.max(0).min(height);
    let bottom = y2.max(0).min(height);
    for y in top..bottom {
        for x in left..right {
            put_pixel(buf, width, height, x, y, rgba);
        }
    }
}

fn draw_rect_outline(
    buf: &mut [u8],
    width: i32,
    height: i32,
    x1: i32,
    y1: i32,
    x2: i32,
    y2: i32,
    stroke: i32,
    rgba: (u8, u8, u8, u8),
) {
    for n in 0..stroke.max(1) {
        draw_filled_rect(buf, width, height, x1 + n, y1 + n, x2 - n, y1 + n + 1, rgba);
        draw_filled_rect(buf, width, height, x1 + n, y2 - n - 1, x2 - n, y2 - n, rgba);
        draw_filled_rect(buf, width, height, x1 + n, y1 + n, x1 + n + 1, y2 - n, rgba);
        draw_filled_rect(buf, width, height, x2 - n - 1, y1 + n, x2 - n, y2 - n, rgba);
    }
}

fn label_rect(m: &Candidate) -> (i32, i32, i32, i32) {
    let left = m.x.round() as i32;
    let top = if m.y > 22.0 {
        m.y.round() as i32 - 20
    } else {
        (m.y + m.h).round() as i32 + 3
    };
    let width = (m.hint.chars().count() as i32 * 9 + 10).max(24);
    (left, top, left + width, top + 16)
}

fn draw_label_background(buf: &mut [u8], width: i32, height: i32, m: &Candidate) {
    let (left, top, right, bottom) = label_rect(m);
    draw_filled_rect(
        buf,
        width,
        height,
        left - 1,
        top - 1,
        right + 1,
        bottom + 1,
        (
            THEME_LABEL_BORDER.0,
            THEME_LABEL_BORDER.1,
            THEME_LABEL_BORDER.2,
            255,
        ),
    );
    draw_filled_rect(
        buf,
        width,
        height,
        left,
        top,
        right,
        bottom,
        (THEME_LABEL_BG.0, THEME_LABEL_BG.1, THEME_LABEL_BG.2, 255),
    );
}

fn force_label_alpha(buf: &mut [u8], width: i32, height: i32, matches: &[Candidate]) {
    for m in matches {
        if m.hint.is_empty() {
            continue;
        }
        let (left, top, right, bottom) = label_rect(m);
        let left = (left - 1).max(0).min(width);
        let right = (right + 1).max(0).min(width);
        let top = (top - 1).max(0).min(height);
        let bottom = (bottom + 1).max(0).min(height);
        for y in top..bottom {
            for x in left..right {
                let idx = ((y * width + x) * 4 + 3) as usize;
                buf[idx] = 255;
            }
        }
    }
}

unsafe fn update_layered_overlay(
    hwnd: HWND,
    region: Region,
    pixels: &[u8],
    matches: &[Candidate],
    selected: usize,
) -> Result<()> {
    trace_log(format!(
        "update_layered_overlay: enter region={}x{} pixels={} matches={}",
        region.width,
        region.height,
        pixels.len(),
        matches.len()
    ));
    let screen = GetDC(None);
    if screen.0.is_null() {
        trace_log("update_layered_overlay: GetDC failed");
        return Err(Error::from_win32());
    }
    let mem = CreateCompatibleDC(screen);
    if mem.0.is_null() {
        ReleaseDC(None, screen);
        trace_log("update_layered_overlay: CreateCompatibleDC failed");
        return Err(Error::from_win32());
    }

    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: region.width,
            biHeight: -region.height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits: *mut c_void = std::ptr::null_mut();
    let bitmap = CreateDIBSection(screen, &bmi, DIB_RGB_COLORS, &mut bits, None, 0)?;
    if bits.is_null() {
        DeleteDC(mem);
        ReleaseDC(None, screen);
        trace_log("update_layered_overlay: CreateDIBSection null bits");
        return Err(Error::from_win32());
    }
    trace_log("update_layered_overlay: copy pixels");
    std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits as *mut u8, pixels.len());

    let old = SelectObject(mem, HBITMAP(bitmap.0));
    trace_log("update_layered_overlay: draw hint text");
    draw_hint_text(mem, matches, selected);
    force_label_alpha(
        std::slice::from_raw_parts_mut(bits as *mut u8, pixels.len()),
        region.width,
        region.height,
        matches,
    );

    let dst = POINT {
        x: region.left,
        y: region.top,
    };
    let size = SIZE {
        cx: region.width,
        cy: region.height,
    };
    let src = POINT { x: 0, y: 0 };
    let blend = BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: 255,
        AlphaFormat: AC_SRC_ALPHA as u8,
    };

    trace_log("update_layered_overlay: UpdateLayeredWindow call");
    let result = UpdateLayeredWindow(
        hwnd,
        screen,
        Some(&dst),
        Some(&size),
        mem,
        Some(&src),
        COLORREF(0),
        Some(&blend),
        ULW_ALPHA,
    );
    trace_log(format!(
        "update_layered_overlay: UpdateLayeredWindow returned ok={}",
        result.is_ok()
    ));

    SelectObject(mem, old);
    DeleteObject(bitmap);
    DeleteDC(mem);
    ReleaseDC(None, screen);
    result
}

unsafe fn draw_hint_text(hdc: HDC, matches: &[Candidate], selected: usize) {
    let font = CreateFontW(
        -13,
        0,
        0,
        0,
        FW_BOLD.0 as i32,
        0,
        0,
        0,
        DEFAULT_CHARSET.0 as u32,
        OUT_DEFAULT_PRECIS.0 as u32,
        CLIP_DEFAULT_PRECIS.0 as u32,
        DEFAULT_QUALITY.0 as u32,
        0,
        w!("Segoe UI"),
    );
    let old_font = SelectObject(hdc, HFONT(font.0));
    SetBkMode(hdc, TRANSPARENT);
    for (i, m) in matches.iter().enumerate() {
        if m.hint.is_empty() {
            continue;
        }
        let text_color = if i == selected {
            THEME_SELECTED
        } else {
            THEME_ACCENT
        };
        SetTextColor(hdc, rgb_tuple(text_color));
        let label = m.hint.to_uppercase();
        let ws = wide(&label);
        let (left, top, _, _) = label_rect(m);
        TextOutW(hdc, left + 5, top + 1, &ws[..ws.len() - 1]);
    }
    SelectObject(hdc, old_font);
    DeleteObject(font);
}

fn test_candidate(text: &str, hint: &str, x: f32, y: f32, w: f32, h: f32) -> Candidate {
    Candidate {
        text: text.to_string(),
        x,
        y,
        w,
        h,
        line: 0,
        word: 0,
        word_count: 1,
        n: norm(text),
        hint: hint.to_string(),
        selector: hint.to_string(),
        hint_typed: String::new(),
        sx: 0,
        sy: 0,
    }
}

fn run_overlay_test() -> Result<()> {
    unsafe {
        let hinstance = HINSTANCE(GetModuleHandleW(None)?.0);
        let cursor = LoadCursorW(None, IDC_ARROW)?;
        RegisterClassW(&WNDCLASSW {
            hCursor: cursor,
            hInstance: hinstance,
            lpszClassName: w!("ScreenSearchRustOverlay"),
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(overlay_proc),
            ..Default::default()
        });

        let mon = cursor_monitor().unwrap_or(Monitor {
            region: virtual_region(),
        });
        let (w, h) = OVERLAY_TEST_SIZE;
        let region = Region {
            left: mon.region.left + (mon.region.width - w) / 2,
            top: mon.region.top + (mon.region.height - h) / 2,
            width: w,
            height: h,
        };
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED | WS_EX_TRANSPARENT,
            w!("ScreenSearchRustOverlay"),
            w!("Screen Search Overlay Test"),
            WS_POPUP,
            region.left,
            region.top,
            region.width,
            region.height,
            None,
            None,
            hinstance,
            None,
        )?;

        let matches = vec![
            test_candidate("Overlay", "aa", 32.0, 42.0, 92.0, 28.0),
            test_candidate("Test", "sf", 166.0, 94.0, 72.0, 26.0),
        ];
        let mut pixels = vec![0u8; (region.width * region.height * 4) as usize];
        draw_rect_outline(
            &mut pixels,
            region.width,
            region.height,
            1,
            1,
            region.width - 1,
            region.height - 1,
            2,
            (34, 197, 94, 220),
        );
        for (i, m) in matches.iter().enumerate() {
            let color = if i == 0 {
                (34, 197, 94, 255)
            } else {
                (251, 146, 60, 245)
            };
            draw_rect_outline(
                &mut pixels,
                region.width,
                region.height,
                m.x.round() as i32 - 4,
                m.y.round() as i32 - 4,
                (m.x + m.w).round() as i32 + 4,
                (m.y + m.h).round() as i32 + 4,
                3,
                color,
            );
            draw_label_background(&mut pixels, region.width, region.height, m);
        }
        update_layered_overlay(hwnd, region, &pixels, &matches, 0)?;
        ShowWindow(hwnd, SW_SHOW);
        thread::sleep(Duration::from_secs(3));
        DestroyWindow(hwnd);
        Ok(())
    }
}

fn point_in_rect(x: i32, y: i32, rect: RECT) -> bool {
    x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom
}

fn window_rect(hwnd: HWND) -> RECT {
    unsafe {
        let mut rect = RECT::default();
        let _ = GetWindowRect(hwnd, &mut rect);
        rect
    }
}

fn virtual_region() -> Region {
    unsafe {
        Region {
            left: GetSystemMetrics(SM_XVIRTUALSCREEN),
            top: GetSystemMetrics(SM_YVIRTUALSCREEN),
            width: GetSystemMetrics(SM_CXVIRTUALSCREEN),
            height: GetSystemMetrics(SM_CYVIRTUALSCREEN),
        }
    }
}

unsafe extern "system" fn enum_monitor_proc(
    hmonitor: HMONITOR,
    _hdc: HDC,
    _rect: *mut RECT,
    data: LPARAM,
) -> BOOL {
    let monitors = &mut *(data.0 as *mut Vec<Monitor>);
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if GetMonitorInfoW(hmonitor, &mut info).as_bool() {
        let r = info.rcMonitor;
        monitors.push(Monitor {
            region: Region {
                left: r.left,
                top: r.top,
                width: r.right - r.left,
                height: r.bottom - r.top,
            },
        });
    }
    true.into()
}

fn monitors() -> Vec<Monitor> {
    let mut mons = Vec::new();
    unsafe {
        let ptr = &mut mons as *mut Vec<Monitor>;
        let _ = EnumDisplayMonitors(None, None, Some(enum_monitor_proc), LPARAM(ptr as isize));
    }
    mons
}

fn cursor_monitor() -> Option<Monitor> {
    let mut pt = POINT::default();
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut pt);
    }
    monitors().into_iter().find(|m| {
        pt.x >= m.region.left
            && pt.x < m.region.left + m.region.width
            && pt.y >= m.region.top
            && pt.y < m.region.top + m.region.height
    })
}

fn ordered_monitors() -> Vec<Monitor> {
    let mut mons = monitors();
    if let Some(active) = cursor_monitor() {
        mons.sort_by_key(|m| if m.region == active.region { 0 } else { 1 });
    }
    mons
}

fn capture_region(region: Region) -> std::result::Result<Shot, String> {
    unsafe {
        let screen = GetDC(None);
        if screen.0.is_null() {
            return Err("GetDC failed".into());
        }
        let mem = CreateCompatibleDC(screen);
        let bmp = CreateCompatibleBitmap(screen, region.width, region.height);
        if mem.0.is_null() || bmp.0.is_null() {
            ReleaseDC(None, screen);
            return Err("CreateCompatibleDC/Bitmap failed".into());
        }
        let old = SelectObject(mem, HBITMAP(bmp.0));
        if BitBlt(
            mem,
            0,
            0,
            region.width,
            region.height,
            screen,
            region.left,
            region.top,
            SRCCOPY,
        )
        .is_err()
        {
            SelectObject(mem, old);
            DeleteObject(bmp);
            DeleteDC(mem);
            ReleaseDC(None, screen);
            return Err("BitBlt failed".into());
        }
        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: region.width,
                biHeight: -region.height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bgra = vec![0u8; (region.width * region.height * 4) as usize];
        let got = GetDIBits(
            mem,
            bmp,
            0,
            region.height as u32,
            Some(bgra.as_mut_ptr() as *mut c_void),
            &mut bmi,
            DIB_RGB_COLORS,
        );
        SelectObject(mem, old);
        DeleteObject(bmp);
        DeleteDC(mem);
        ReleaseDC(None, screen);
        if got == 0 {
            return Err("GetDIBits failed".into());
        }
        Ok(Shot {
            width: region.width,
            height: region.height,
            bgra,
        })
    }
}

fn effective_scale(shot: &Shot, scale: f32) -> f32 {
    if scale <= 1.0 {
        return 1.0;
    }
    let max_dim = OcrEngine::MaxImageDimension().unwrap_or(10000) as f32;
    let cap = max_dim / shot.width.max(shot.height) as f32;
    scale.min(cap).max(1.0)
}

fn bitmap_from_shot(shot: &Shot, scale: f32, variant: OcrVariant) -> Result<(SoftwareBitmap, f32)> {
    let scale = effective_scale(shot, scale);
    let (w, h, mut bgra) = if (scale - 1.0).abs() < f32::EPSILON {
        (shot.width, shot.height, shot.bgra.clone())
    } else {
        let rgba = bgra_to_rgba(&shot.bgra);
        let img = image::RgbaImage::from_raw(shot.width as u32, shot.height as u32, rgba)
            .ok_or_else(Error::from_win32)?;
        let w = (shot.width as f32 * scale).round() as u32;
        let h = (shot.height as f32 * scale).round() as u32;
        let resized = image::imageops::resize(&img, w, h, image::imageops::FilterType::Lanczos3);
        let bgra = rgba_to_bgra(resized.as_raw());
        (w as i32, h as i32, bgra)
    };
    if matches!(variant, OcrVariant::HighContrast) {
        apply_high_contrast_ocr_preprocess(&mut bgra);
    }
    let writer = DataWriter::new()?;
    writer.WriteBytes(&bgra)?;
    let buffer = writer.DetachBuffer()?;
    let bitmap = SoftwareBitmap::CreateCopyFromBuffer(&buffer, BitmapPixelFormat::Bgra8, w, h)?;
    Ok((bitmap, scale))
}

fn bgra_to_rgba(bytes: &[u8]) -> Vec<u8> {
    let mut out = bytes.to_vec();
    for px in out.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    out
}

fn rgba_to_bgra(bytes: &[u8]) -> Vec<u8> {
    bgra_to_rgba(bytes)
}

fn apply_high_contrast_ocr_preprocess(bgra: &mut [u8]) {
    for px in bgra.chunks_exact_mut(4) {
        let b = px[0] as u16;
        let g = px[1] as u16;
        let r = px[2] as u16;
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let luma = (77 * r + 150 * g + 29 * b) / 256;
        let ink = luma >= 145 || (max >= 120 && max - min <= 80);
        let v = if ink { 0 } else { 255 };
        px[0] = v;
        px[1] = v;
        px[2] = v;
        px[3] = 255;
    }
}

fn recognize_words(
    shot: &Shot,
    scale: f32,
    variant: OcrVariant,
    line_offset: usize,
) -> std::result::Result<Vec<Word>, String> {
    let (bitmap, scale) = bitmap_from_shot(shot, scale, variant).map_err(|e| format!("{e:?}"))?;
    let engine = OcrEngine::TryCreateFromUserProfileLanguages()
        .map_err(|e| format!("OCR engine init failed: {e:?}"))?;
    let result = engine
        .RecognizeAsync(&bitmap)
        .map_err(|e| format!("RecognizeAsync failed: {e:?}"))?
        .get()
        .map_err(|e| format!("OCR failed: {e:?}"))?;
    let inv = 1.0 / scale;
    let mut out = Vec::new();
    let lines = result.Lines().map_err(|e| format!("{e:?}"))?;
    for line_no in 0..lines.Size().map_err(|e| format!("{e:?}"))? {
        let line = lines.GetAt(line_no).map_err(|e| format!("{e:?}"))?;
        let words = line.Words().map_err(|e| format!("{e:?}"))?;
        for word_no in 0..words.Size().map_err(|e| format!("{e:?}"))? {
            let word = words.GetAt(word_no).map_err(|e| format!("{e:?}"))?;
            let text = word.Text().map_err(|e| format!("{e:?}"))?.to_string_lossy();
            let r = word.BoundingRect().map_err(|e| format!("{e:?}"))?;
            out.push(Word {
                text: text.clone(),
                x: r.X * inv,
                y: r.Y * inv,
                w: r.Width * inv,
                h: r.Height * inv,
                line: line_offset + line_no as usize,
                word: word_no as usize,
                n: norm(&text),
            });
        }
    }
    Ok(out)
}

fn next_line_offset(words: &[Word]) -> usize {
    words.iter().map(|w| w.line).max().unwrap_or(0) + 1
}

fn ocr_words(shot: &Shot, scale: f32) -> std::result::Result<Vec<Word>, String> {
    let mut words = recognize_words(shot, scale, OcrVariant::Raw, 0)?;
    if effective_scale(shot, scale) >= HIGH_CONTRAST_OCR_MIN_SCALE {
        match recognize_words(
            shot,
            scale,
            OcrVariant::HighContrast,
            next_line_offset(&words),
        ) {
            Ok(extra) => {
                words = merge_ocr_words(&words, &extra);
            }
            Err(err) => {
                trace_log(format!("high contrast OCR failed: {err}"));
            }
        }
    }
    Ok(words)
}

fn offset_words(
    words: Vec<Word>,
    mon: Region,
    base: Region,
    line_offset: usize,
) -> (Vec<Word>, usize) {
    let dx = (mon.left - base.left) as f32;
    let dy = (mon.top - base.top) as f32;
    let mut max_line = line_offset;
    let moved = words
        .into_iter()
        .map(|mut w| {
            w.x += dx;
            w.y += dy;
            w.line += line_offset;
            max_line = max_line.max(w.line);
            w
        })
        .collect();
    (moved, max_line + 1)
}

fn word_center(w: &Word) -> (f32, f32) {
    (w.x + w.w / 2.0, w.y + w.h / 2.0)
}

fn merge_ocr_words(primary: &[Word], extra: &[Word]) -> Vec<Word> {
    let mut merged = primary.to_vec();
    let mut seen = merged
        .iter()
        .map(|w| {
            let (cx, cy) = word_center(w);
            (w.n.clone(), cx, cy, w.w.max(w.h).max(1.0))
        })
        .collect::<Vec<_>>();
    for w in extra {
        if w.n.is_empty() {
            continue;
        }
        let (cx, cy) = word_center(w);
        let duplicate = seen.iter().any(|(text, sx, sy, size)| {
            let tolerance = 10.0_f32.max(*size * 0.35);
            text == &w.n && (cx - sx).abs() <= tolerance && (cy - sy).abs() <= tolerance
        });
        if !duplicate {
            merged.push(w.clone());
            seen.push((w.n.clone(), cx, cy, w.w.max(w.h).max(1.0)));
        }
    }
    merged
}

fn make_snapshot(
    words: Vec<Word>,
    region: Region,
    complete: bool,
    quality: &'static str,
) -> Snapshot {
    let candidates = build_text_candidates(&words);
    Snapshot {
        words,
        candidates,
        region,
        complete,
        quality,
    }
}

fn capture_snapshots(
    all_monitors: bool,
    active_scale: f32,
    hwnd: HWND,
    seq: u64,
) -> std::result::Result<(), String> {
    let active = cursor_monitor().ok_or_else(|| "No active monitor".to_string())?;
    if !all_monitors {
        let shot = capture_region(active.region)?;
        let words = ocr_words(&shot, active_scale)?;
        post_snapshot(hwnd, seq, make_snapshot(words, active.region, true, "fast"));
        return Ok(());
    }

    let base = virtual_region();
    let ordered = ordered_monitors();
    let mut merged_words = Vec::new();
    let passes: Vec<(f32, &'static str)> = if active_scale > 1.0 {
        ALL_MONITOR_OCR_PASSES.to_vec()
    } else {
        vec![(1.0, "fast")]
    };
    let pass_count = passes.len();
    for (pass_idx, (scale, quality)) in passes.into_iter().enumerate() {
        let mut pass_words = Vec::new();
        let mut line_offset = 0;
        for mon in &ordered {
            let shot = capture_region(mon.region)?;
            let words = ocr_words(&shot, scale)?;
            let (moved, next) = offset_words(words, mon.region, base, line_offset);
            line_offset = next;
            pass_words.extend(moved);
        }
        merged_words = if merged_words.is_empty() {
            pass_words
        } else {
            merge_ocr_words(&merged_words, &pass_words)
        };
        post_snapshot(
            hwnd,
            seq,
            make_snapshot(
                merged_words.clone(),
                base,
                pass_idx + 1 == pass_count,
                quality,
            ),
        );
    }
    Ok(())
}

fn run_ocr_diagnostics(args: &[String], bench: bool) -> Result<()> {
    let started = Instant::now();
    let active_only = args_has(args, "--active-monitor");
    let monitors = if active_only {
        cursor_monitor().into_iter().collect::<Vec<_>>()
    } else {
        ordered_monitors()
    };
    let scales = if bench {
        ALL_MONITOR_OCR_PASSES
            .iter()
            .map(|(scale, _)| *scale)
            .collect()
    } else if args_has(args, "--no-upscale") {
        vec![1.0]
    } else {
        vec![2.0]
    };
    let mut report = String::new();
    report.push_str("Screen Search OCR diagnostics\n");
    report.push_str(&format!("mode={}\n", if bench { "bench" } else { "dump" }));
    report.push_str(&format!("active_only={active_only}\n"));
    report.push_str(&format!("monitors={}\n", monitors.len()));
    report.push_str(&format!("scales={scales:?}\n\n"));

    for (idx, mon) in monitors.iter().enumerate() {
        report.push_str(&format!(
            "monitor #{idx}: left={} top={} width={} height={}\n",
            mon.region.left, mon.region.top, mon.region.width, mon.region.height
        ));
        let capture_start = Instant::now();
        match capture_region(mon.region) {
            Ok(shot) => {
                let capture_elapsed = capture_start.elapsed();
                report.push_str(&format!("  capture_ms={}\n", capture_elapsed.as_millis()));
                for requested_scale in &scales {
                    let actual_scale = effective_scale(&shot, *requested_scale);
                    let ocr_start = Instant::now();
                    match ocr_words(&shot, *requested_scale) {
                        Ok(words) => {
                            let elapsed = ocr_start.elapsed();
                            report.push_str(&format!(
                                "  requested_scale={requested_scale:.2} actual_scale={actual_scale:.2} ocr_ms={} words={}\n",
                                elapsed.as_millis(),
                                words.len()
                            ));
                            if !bench {
                                for word in words {
                                    report.push_str(&format!(
                                        "    [{:.0},{:.0},{:.0},{:.0}] {}\n",
                                        word.x, word.y, word.w, word.h, word.text
                                    ));
                                }
                            }
                        }
                        Err(err) => {
                            report.push_str(&format!(
                                "  requested_scale={requested_scale:.2} actual_scale={actual_scale:.2} error={err}\n"
                            ));
                        }
                    }
                }
            }
            Err(err) => {
                report.push_str(&format!("  capture_error={err}\n"));
            }
        }
        report.push('\n');
    }
    report.push_str(&format!("total_ms={}\n", started.elapsed().as_millis()));

    let path = local_data_path(DIAGNOSTICS_FILE_NAME);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    fs::write(&path, report).map_err(|_| Error::from_win32())?;
    if !args_has(args, "--quiet") {
        show_message(
            "Screen Search Diagnostics",
            &format!("Diagnostics written to:\n{}", path.display()),
        );
    }
    Ok(())
}

fn post_snapshot(hwnd: HWND, seq: u64, snap: Snapshot) {
    unsafe {
        let boxed = Box::new(snap);
        let _ = PostMessageW(
            hwnd,
            WM_SNAPSHOT,
            WPARAM(seq as usize),
            LPARAM(Box::into_raw(boxed) as isize),
        );
    }
}

fn post_capture_done(hwnd: HWND, seq: u64) {
    unsafe {
        let _ = PostMessageW(hwnd, WM_CAPTURE_DONE, WPARAM(seq as usize), LPARAM(0));
    }
}

fn post_capture_failed(hwnd: HWND, seq: u64, err: String) {
    unsafe {
        let boxed = Box::new(err);
        let _ = PostMessageW(
            hwnd,
            WM_CAPTURE_FAILED,
            WPARAM(seq as usize),
            LPARAM(Box::into_raw(boxed) as isize),
        );
    }
}

fn current_confirm_action() -> ConfirmAction {
    unsafe {
        let ctrl_down = is_key_down(VK_CONTROL);
        let shift_down = is_key_down(VK_SHIFT);
        if ctrl_down && shift_down {
            ConfirmAction::RightClick
        } else if ctrl_down {
            ConfirmAction::LeftClick
        } else {
            ConfirmAction::Move
        }
    }
}

fn is_ctrl_a(vk: VIRTUAL_KEY) -> bool {
    vk.0 == VK_A_CODE && unsafe { is_key_down(VK_CONTROL) }
}

unsafe fn is_key_down(vk: VIRTUAL_KEY) -> bool {
    (windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState(vk.0 as i32) as u16 & 0x8000) != 0
}

unsafe fn move_cursor_to(x: i32, y: i32) {
    let _ = SetCursorPos(x, y);
}

unsafe fn click_at(x: i32, y: i32, button: MouseButton) {
    move_cursor_to(x, y);
    let hwnd = WindowFromPoint(POINT { x, y });
    if !hwnd.0.is_null() {
        let top = GetAncestor(hwnd, GA_ROOT);
        let _ = SetForegroundWindow(if !top.0.is_null() { top } else { hwnd });
    }
    thread::sleep(Duration::from_millis(80));
    move_cursor_to(x, y);
    let (down, up) = match button {
        MouseButton::Left => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
        MouseButton::Right => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
    };
    send_mouse(down);
    thread::sleep(Duration::from_millis(90));
    send_mouse(up);
}

unsafe fn send_mouse(flags: windows::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS) {
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let _ = SendInput(&[input], size_of::<INPUT>() as i32);
}

fn args_has(args: &[String], flag: &str) -> bool {
    args.iter().any(|arg| arg == flag)
}

fn show_message(title: &str, body: &str) {
    let title = wide(title);
    let body = wide(body);
    unsafe {
        windows::Win32::UI::WindowsAndMessaging::MessageBoxW(
            None,
            PCWSTR(body.as_ptr()),
            PCWSTR(title.as_ptr()),
            Default::default(),
        );
    }
}

fn run() -> Result<()> {
    install_panic_log();
    unsafe {
        let _ = RoInitialize(RO_INIT_MULTITHREADED);
    }
    let args = std::env::args().collect::<Vec<_>>();
    TRACE_ENABLED.store(args_has(&args, "--debug"), Ordering::Relaxed);
    let overlay_test = args.iter().any(|a| a == "--overlay-test");
    if overlay_test {
        return run_overlay_test();
    }
    if args_has(&args, "--bench-ocr") {
        return run_ocr_diagnostics(&args, true);
    }
    if args_has(&args, "--dump-ocr") {
        return run_ocr_diagnostics(&args, false);
    }
    let mut settings = load_settings();
    if args_has(&args, "--enable-overlay") {
        settings.overlay_enabled = true;
    }
    if args_has(&args, "--disable-overlay") {
        settings.overlay_enabled = false;
    }
    if args_has(&args, "--active-monitor") {
        settings.scan_all = false;
    }
    if args_has(&args, "--all-monitors") {
        settings.scan_all = true;
    }
    if args_has(&args, "--upscale") {
        settings.upscale = true;
    }
    if args_has(&args, "--no-upscale") {
        settings.upscale = false;
    }
    let test_instance = args.iter().any(|a| a == "--test-instance");
    let toggle = args.iter().any(|a| a == "--toggle");
    let toggle_all = args.iter().any(|a| a == "--toggle-all");
    let quit = args.iter().any(|a| a == "--quit");
    if quit {
        if !signal_event(EVENT_QUIT_NAME) {
            let _ = post_quit_window_message();
        }
        return Ok(());
    }
    let event = if toggle_all {
        EVENT_ALL_NAME
    } else {
        EVENT_NAME
    };
    if !test_instance && (toggle || toggle_all) && signal_event(event) {
        return Ok(());
    }

    if !test_instance {
        let mutex_name = wide(MUTEX_NAME);
        unsafe {
            let mutex = CreateMutexW(None, false, PCWSTR(mutex_name.as_ptr()))?;
            if GetLastError() == ERROR_ALREADY_EXISTS {
                if toggle || toggle_all {
                    let _ = signal_event(event);
                }
                let _ = CloseHandle(mutex);
                return Ok(());
            }
        }
    }

    let hinstance = unsafe { HINSTANCE(GetModuleHandleW(None)?.0) };
    let app = Arc::new(Mutex::new(App::new(
        hinstance,
        toggle || toggle_all,
        settings,
        !test_instance,
    )));
    let _ = APP.set(app.clone());
    App::create_windows(app)?;
    App::message_loop();
    Ok(())
}

fn main() {
    if let Err(err) = run() {
        show_message("Screen Search Rust", &format!("{err:?}"));
    }
}

#[cfg(test)]
mod app_tests {
    use super::*;

    #[test]
    fn high_contrast_preprocess_turns_light_text_into_dark_ink() {
        let mut bgra = vec![
            12, 12, 12, 255, //
            240, 240, 240, 255,
        ];
        apply_high_contrast_ocr_preprocess(&mut bgra);
        assert_eq!(&bgra[0..4], &[255, 255, 255, 255]);
        assert_eq!(&bgra[4..8], &[0, 0, 0, 255]);
    }
}
