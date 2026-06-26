#![windows_subsystem = "windows"]
#![allow(unsafe_op_in_unsafe_fn)]
#![allow(unused_must_use)]

mod matcher;

use std::ffi::c_void;
use std::mem::{size_of, zeroed};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

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
    BitBlt, CLIP_DEFAULT_PRECIS, CreateCompatibleBitmap, CreateCompatibleDC, CreateDIBSection,
    CreateFontW, DEFAULT_CHARSET, DEFAULT_QUALITY, DIB_RGB_COLORS, DeleteDC, DeleteObject,
    EndPaint, EnumDisplayMonitors, FW_BOLD, GetDC, GetDIBits, GetMonitorInfoW, HBITMAP, HDC, HFONT,
    HMONITOR, MONITORINFO, OUT_DEFAULT_PRECIS, ReleaseDC, SRCCOPY, SelectObject, SetBkMode,
    SetTextColor, TRANSPARENT, TextOutW,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{
    AttachThreadInput, CreateEventW, CreateMutexW, EVENT_MODIFY_STATE, GetCurrentThreadId,
    INFINITE, OpenEventW, SetEvent, WaitForSingleObject,
};
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEINPUT, SendInput,
    SetFocus, VIRTUAL_KEY, VK_ESCAPE, VK_F5, VK_RETURN, VK_TAB,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW,
    DestroyWindow, DispatchMessageW, EN_CHANGE, FindWindowW, GA_ROOT, GetAncestor,
    GetForegroundWindow, GetMessageW, GetSystemMetrics, GetWindowRect, GetWindowTextLengthW,
    GetWindowTextW, GetWindowThreadProcessId, HMENU, IDC_ARROW, IsWindow, LoadCursorW, MSG,
    MoveWindow, PostMessageW, PostQuitMessage, RegisterClassW, SM_CXVIRTUALSCREEN,
    SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SW_HIDE, SW_SHOW, SWP_NOZORDER,
    SetCursorPos, SetForegroundWindow, SetTimer, SetWindowPos, ShowWindow, TranslateMessage,
    ULW_ALPHA, UpdateLayeredWindow, WINDOW_EX_STYLE, WINDOW_STYLE, WM_ACTIVATE, WM_APP, WM_COMMAND,
    WM_CREATE, WM_DESTROY, WM_KEYDOWN, WM_PAINT, WM_TIMER, WNDCLASSW, WS_BORDER, WS_CHILD,
    WS_CLIPSIBLINGS, WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
    WS_VISIBLE, WindowFromPoint,
};
use windows::core::{Error, PCWSTR, Result, w};

const EVENT_NAME: &str = "ScreenSearchToggleEvent";
const EVENT_ALL_NAME: &str = "ScreenSearchToggleAllEvent";
const EVENT_QUIT_NAME: &str = "ScreenSearchQuitEvent";
const MUTEX_NAME: &str = "ScreenSearchSingletonMutex";

const INACTIVE_MONITOR_SCALE: f32 = 1.25;
const HIGH_QUALITY_EXTRA_SCALES: &[f32] = &[2.0, 3.0];
const OVERLAY_ENABLED: bool = false;
const OVERLAY_TEST_SIZE: (i32, i32) = (360, 180);

const WM_TOGGLE: u32 = WM_APP + 1;
const WM_TOGGLE_ALL: u32 = WM_APP + 2;
const WM_SNAPSHOT: u32 = WM_APP + 3;
const WM_CAPTURE_DONE: u32 = WM_APP + 4;
const WM_CAPTURE_FAILED: u32 = WM_APP + 5;
const WM_QUIT_APP: u32 = WM_APP + 6;
const TIMER_FILTER: usize = 1;

static APP: OnceCell<Arc<Mutex<App>>> = OnceCell::new();

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
    cold_show: bool,
}

unsafe impl Send for App {}

fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF((r as u32) | ((g as u32) << 8) | ((b as u32) << 16))
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
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
    match msg {
        WM_CREATE => LRESULT(0),
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
            }
            LRESULT(0)
        }
        WM_KEYDOWN => {
            let vk = VIRTUAL_KEY(wparam.0 as u16);
            if let Some(app) = APP.get() {
                let mut app = app.lock();
                match vk {
                    VK_RETURN => {
                        app.confirm();
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
            if wparam.0 == 0 {
                if let Some(app) = APP.get() {
                    let mut app = app.lock();
                    if app.popup_visible {
                        app.hide_popup();
                    }
                }
            }
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
        WM_DESTROY => {
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
    match msg {
        WM_PAINT => {
            if let Some(app) = APP.get() {
                app.lock().paint_overlay(hwnd);
            } else {
                let mut ps = zeroed();
                let hdc = BeginPaint(hwnd, &mut ps);
                EndPaint(hwnd, &ps);
                let _ = hdc;
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

impl App {
    fn new(hinstance: HINSTANCE, cold_show: bool) -> Self {
        Self {
            hinstance,
            scan_all: true,
            exact: false,
            upscale: true,
            debug_all: false,
            cold_show,
            ..Default::default()
        }
    }

    fn create_windows(app: Arc<Mutex<App>>) -> Result<()> {
        let hinstance = app.lock().hinstance;
        unsafe {
            let popup_class = w!("ScreenSearchRustPopup");
            let overlay_class = w!("ScreenSearchRustOverlay");
            let cursor = LoadCursorW(None, IDC_ARROW)?;
            RegisterClassW(&WNDCLASSW {
                hCursor: cursor,
                hInstance: hinstance,
                lpszClassName: popup_class,
                style: CS_HREDRAW | CS_VREDRAW,
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
                480,
                74,
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
                13,
                10,
                454,
                28,
                hwnd,
                HMENU(1001usize as *mut c_void),
                hinstance,
                None,
            )?;
            let status = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("Type to search."),
                WS_CHILD | WS_VISIBLE,
                13,
                44,
                454,
                18,
                hwnd,
                HMENU(1002usize as *mut c_void),
                hinstance,
                None,
            )?;

            {
                let mut a = app.lock();
                a.hwnd = hwnd;
                a.edit = edit;
                a.status = status;
            }
            create_resident_events(hwnd)?;
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
                    let edit = app.lock().edit;
                    if msg.hwnd == edit && msg.message == WM_KEYDOWN {
                        if app.lock().handle_key(VIRTUAL_KEY(msg.wParam.0 as u16)) {
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
        match vk {
            VK_RETURN => {
                self.confirm();
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
        }
        self.popup_visible = true;
        self.start_capture(true, force_all);
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
        let w = 480;
        let h = 74;
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
        if !self.popup_visible {
            return;
        }
        let text = get_text(self.edit).trim().to_string();
        if text == self.last_query {
            return;
        }
        self.last_query = text.clone();
        if text.is_empty() {
            self.close_overlay();
            set_text(self.status, "Type to search.");
            return;
        }
        if self.snap.is_none() {
            if !self.capturing {
                self.start_capture(false, false);
            } else {
                set_text(self.status, "Reading screen...");
            }
            return;
        }
        self.live_filter();
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
        let inactive_scale = if self.upscale && all_mon {
            INACTIVE_MONITOR_SCALE
        } else {
            active_scale
        };
        thread::spawn(move || {
            let hwnd = HWND(hwnd_raw as *mut c_void);
            if let Err(err) = unsafe { RoInitialize(RO_INIT_MULTITHREADED) } {
                post_capture_failed(hwnd, seq, format!("WinRT init failed: {err:?}"));
                return;
            }
            match capture_snapshots(all_mon, active_scale, inactive_scale, hwnd, seq) {
                Ok(()) => post_capture_done(hwnd, seq),
                Err(err) => post_capture_failed(hwnd, seq, err),
            }
        });
    }

    fn accept_snapshot(&mut self, seq: u64, snap: Snapshot) {
        if seq != self.capture_seq {
            return;
        }
        self.snap = Some(snap.clone());
        self.capturing =
            !snap.complete || (snap.quality != "high" && self.scan_all && self.upscale);
        if !self.popup_visible {
            return;
        }
        if !get_text(self.edit).trim().is_empty() {
            self.live_filter();
            return;
        }
        if snap.quality == "high" {
            set_text(self.status, "Type to search.");
        } else if snap.complete {
            set_text(
                self.status,
                "Type to search. Full scan ready; improving OCR...",
            );
        } else {
            set_text(
                self.status,
                "Type to search. Active monitor ready; scanning others...",
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
        let Some(snap) = &self.snap else {
            return;
        };
        let text = get_text(self.edit).trim().to_string();
        let q = norm(&text);
        if q.is_empty() {
            self.close_overlay();
            self.hint_context = None;
            return;
        }
        let (mut matches, ctx, hint_suffix) =
            resolve_selector_matches(&q, &snap.candidates, self.hint_context.as_ref(), self.exact);
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
            return;
        }
        self.refresh_overlay();
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
                    "{} {mode} match(es) for '{text}'. Type selector letters; Enter = click.",
                    self.matches.len()
                ),
            );
        }
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

    fn confirm(&mut self) {
        if self.matches.is_empty() {
            return;
        }
        let m = self.matches[self.selected].clone();
        self.snap = None;
        self.hide_popup();
        thread::spawn(move || unsafe {
            click_at(m.sx, m.sy);
        });
    }

    fn recapture(&mut self) {
        self.snap = None;
        self.hint_context = None;
        self.close_overlay();
        self.start_capture(true, false);
    }

    fn ensure_overlay(&mut self) {
        if !OVERLAY_ENABLED {
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
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED | WS_EX_TRANSPARENT,
                w!("ScreenSearchRustOverlay"),
                w!("Screen Search Overlay"),
                WS_POPUP,
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
        if !OVERLAY_ENABLED {
            self.close_overlay();
            return;
        }
        if self.region.width <= 0 || self.region.height <= 0 {
            self.close_overlay();
            return;
        }
        self.ensure_overlay();
        if self.overlay.0.is_null() {
            return;
        }

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
                    (58, 123, 213, 150),
                );
            }
        }

        for (i, m) in self.matches.iter().enumerate() {
            let selected = i == self.selected;
            let color = if selected {
                (34, 197, 94, 255)
            } else {
                (251, 146, 60, 245)
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
                (17, 24, 39, 240),
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
                let label_y = if m.y > 22.0 {
                    m.y.round() as i32 - 20
                } else {
                    (m.y + m.h).round() as i32 + 3
                };
                draw_filled_rect(
                    &mut pixels,
                    self.region.width,
                    self.region.height,
                    m.x.round() as i32,
                    label_y,
                    m.x.round() as i32 + 24,
                    label_y + 16,
                    color,
                );
            }
        }

        if unsafe { update_layered_overlay(self.overlay, self.region, &pixels, &self.matches) }
            .is_ok()
        {
            unsafe {
                ShowWindow(self.overlay, SW_SHOW);
            }
        } else {
            self.close_overlay();
        }
    }

    fn close_overlay(&mut self) {
        unsafe {
            if !self.overlay.0.is_null() && IsWindow(self.overlay).as_bool() {
                DestroyWindow(self.overlay);
            }
        }
        self.overlay = HWND(std::ptr::null_mut());
    }

    fn paint_overlay(&self, hwnd: HWND) {
        let _ = hwnd;
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

unsafe fn update_layered_overlay(
    hwnd: HWND,
    region: Region,
    pixels: &[u8],
    matches: &[Candidate],
) -> Result<()> {
    let screen = GetDC(None);
    if screen.0.is_null() {
        return Err(Error::from_win32());
    }
    let mem = CreateCompatibleDC(screen);
    if mem.0.is_null() {
        ReleaseDC(None, screen);
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
        return Err(Error::from_win32());
    }
    std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits as *mut u8, pixels.len());

    let old = SelectObject(mem, HBITMAP(bitmap.0));
    draw_hint_text(mem, matches);

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

    SelectObject(mem, old);
    DeleteObject(bitmap);
    DeleteDC(mem);
    ReleaseDC(None, screen);
    result
}

unsafe fn draw_hint_text(hdc: HDC, matches: &[Candidate]) {
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
    SetTextColor(hdc, rgb(17, 24, 39));
    for m in matches {
        if m.hint.is_empty() {
            continue;
        }
        let label = m.hint.to_uppercase();
        let ws = wide(&label);
        let y = if m.y > 22.0 {
            m.y.round() as i32 - 19
        } else {
            (m.y + m.h).round() as i32 + 4
        };
        TextOutW(hdc, m.x.round() as i32 + 4, y, &ws[..ws.len() - 1]);
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
            draw_filled_rect(
                &mut pixels,
                region.width,
                region.height,
                m.x.round() as i32,
                m.y.round() as i32 - 20,
                m.x.round() as i32 + 24,
                m.y.round() as i32 - 4,
                color,
            );
        }
        update_layered_overlay(hwnd, region, &pixels, &matches)?;
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

fn bitmap_from_shot(shot: &Shot, scale: f32) -> Result<(SoftwareBitmap, f32)> {
    let scale = effective_scale(shot, scale);
    let (w, h, bgra) = if (scale - 1.0).abs() < f32::EPSILON {
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

fn ocr_words(shot: &Shot, scale: f32) -> std::result::Result<Vec<Word>, String> {
    let (bitmap, scale) = bitmap_from_shot(shot, scale).map_err(|e| format!("{e:?}"))?;
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
                line: line_no as usize,
                word: word_no as usize,
                n: norm(&text),
            });
        }
    }
    Ok(out)
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
    inactive_scale: f32,
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
    let mut all_words = Vec::new();
    let mut line_offset = 0;
    let ordered = ordered_monitors();
    for (idx, mon) in ordered.iter().enumerate() {
        let scale = if idx == 0 {
            active_scale
        } else {
            inactive_scale
        };
        let shot = capture_region(mon.region)?;
        let words = ocr_words(&shot, scale)?;
        let (moved, next) = offset_words(words, mon.region, base, line_offset);
        line_offset = next;
        all_words.extend(moved);
        if idx == 0 {
            post_snapshot(
                hwnd,
                seq,
                make_snapshot(all_words.clone(), base, false, "fast"),
            );
        }
    }
    post_snapshot(
        hwnd,
        seq,
        make_snapshot(all_words.clone(), base, true, "fast"),
    );

    if active_scale > inactive_scale {
        let mut hq_words = Vec::new();
        let mut hq_line_offset = 0;
        for scale in HIGH_QUALITY_EXTRA_SCALES
            .iter()
            .copied()
            .filter(|s| *s >= active_scale)
        {
            for mon in &ordered {
                let shot = capture_region(mon.region)?;
                let words = ocr_words(&shot, scale)?;
                let (moved, next) = offset_words(words, mon.region, base, hq_line_offset);
                hq_line_offset = next;
                hq_words.extend(moved);
            }
        }
        let merged = merge_ocr_words(&all_words, &hq_words);
        post_snapshot(hwnd, seq, make_snapshot(merged, base, true, "high"));
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

unsafe fn click_at(x: i32, y: i32) {
    let _ = SetCursorPos(x, y);
    let hwnd = WindowFromPoint(POINT { x, y });
    if !hwnd.0.is_null() {
        let top = GetAncestor(hwnd, GA_ROOT);
        let _ = SetForegroundWindow(if !top.0.is_null() { top } else { hwnd });
    }
    thread::sleep(Duration::from_millis(80));
    let _ = SetCursorPos(x, y);
    send_mouse(MOUSEEVENTF_LEFTDOWN);
    thread::sleep(Duration::from_millis(90));
    send_mouse(MOUSEEVENTF_LEFTUP);
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

fn run() -> Result<()> {
    unsafe {
        let _ = RoInitialize(RO_INIT_MULTITHREADED);
    }
    let args = std::env::args().collect::<Vec<_>>();
    let overlay_test = args.iter().any(|a| a == "--overlay-test");
    if overlay_test {
        return run_overlay_test();
    }
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
    if (toggle || toggle_all) && signal_event(event) {
        return Ok(());
    }

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

    let hinstance = unsafe { HINSTANCE(GetModuleHandleW(None)?.0) };
    let app = Arc::new(Mutex::new(App::new(hinstance, toggle || toggle_all)));
    let _ = APP.set(app.clone());
    App::create_windows(app)?;
    App::message_loop();
    Ok(())
}

fn main() {
    if let Err(err) = run() {
        let msg = format!("{err:?}");
        let title = wide("Screen Search Rust");
        let body = wide(&msg);
        unsafe {
            windows::Win32::UI::WindowsAndMessaging::MessageBoxW(
                None,
                PCWSTR(body.as_ptr()),
                PCWSTR(title.as_ptr()),
                Default::default(),
            );
        }
    }
}
