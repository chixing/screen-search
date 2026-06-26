"""
Screen Search + Click  --  GUI prototype.

Flow:
  1. Type a visible text prefix.
  2. Matching words/phrases get boxes plus a short selector label.
  3. Keep typing the selector letters to disqualify other highlights.
  4. Press Enter to click the focused match. Press Esc to cancel.

Visual feedback:
  orange box   = a match
  green box    = currently selected match

OCR uses Windows' BUILT-IN engine (Windows.Media.Ocr via winsdk).
Overlays are input-transparent (WS_EX_TRANSPARENT) so they never eat the click.

Run:   python screen_click_gui.py
Deps:  pip install mss pillow winsdk
"""

import asyncio
import ctypes
from ctypes import wintypes
import os
import queue
import re
import sys
import threading
import time
import tkinter as tk
from tkinter import ttk

import mss
from PIL import Image, ImageDraw
import pystray

from winsdk.windows.graphics.imaging import SoftwareBitmap, BitmapPixelFormat
from winsdk.windows.media.ocr import OcrEngine
from winsdk.windows.storage.streams import DataWriter

user32 = ctypes.windll.user32
kernel32 = ctypes.windll.kernel32
user32.GetForegroundWindow.restype = wintypes.HWND  # avoid 64-bit handle truncation

# ---------- single-instance + external triggers (owned by AutoHotkey) ----
EVENT_NAME = "ScreenSearchToggleEvent"
EVENT_ALL_NAME = "ScreenSearchToggleAllEvent"
MUTEX_NAME = "ScreenSearchSingletonMutex"


def _signal_event(event_name):
    """Pulse a resident app event. True if a resident was found."""
    EVENT_MODIFY_STATE = 0x0002
    h = kernel32.OpenEventW(EVENT_MODIFY_STATE, False, event_name)
    if not h:
        return False
    kernel32.SetEvent(h)
    kernel32.CloseHandle(h)
    return True


def make_toggle_command(toggle_all=False):
    """The command invoked by komorebi.ahk."""
    pyw = os.path.join(os.path.dirname(sys.executable), "pythonw.exe")
    if not os.path.exists(pyw):
        pyw = sys.executable
    mode = "--toggle-all" if toggle_all else "--toggle"
    return f'"{pyw}" "{os.path.abspath(__file__)}" {mode}'

# ---------- real mouse click via SendInput (works across ALL monitors) ----
MOUSEEVENTF_LEFTDOWN = 0x0002
MOUSEEVENTF_LEFTUP = 0x0004
INPUT_MOUSE = 0
GA_ROOT = 2

_PUL = ctypes.POINTER(ctypes.c_ulong)


class _MOUSEINPUT(ctypes.Structure):
    _fields_ = [("dx", wintypes.LONG), ("dy", wintypes.LONG),
                ("mouseData", wintypes.DWORD), ("dwFlags", wintypes.DWORD),
                ("time", wintypes.DWORD), ("dwExtraInfo", _PUL)]


class _INPUT(ctypes.Structure):
    class _U(ctypes.Union):
        _fields_ = [("mi", _MOUSEINPUT)]
    _anonymous_ = ("u",)
    _fields_ = [("type", wintypes.DWORD), ("u", _U)]


def _send_mouse(flags):
    extra = ctypes.c_ulong(0)
    mi = _MOUSEINPUT(0, 0, 0, flags, 0, ctypes.pointer(extra))
    inp = _INPUT(INPUT_MOUSE, _INPUT._U(mi=mi))
    user32.SendInput(1, ctypes.byref(inp), ctypes.sizeof(inp))


def move_cursor(x, y):
    user32.SetCursorPos(int(x), int(y))


def _focus_window_at(x, y):
    """Bring the window under (x, y) to the foreground so the click isn't
    swallowed as a background-activation click."""
    hwnd = user32.WindowFromPoint(wintypes.POINT(int(x), int(y)))
    if hwnd:
        top = user32.GetAncestor(hwnd, GA_ROOT) or hwnd
        user32.SetForegroundWindow(top)


def click_at(x, y):
    """Reliable left-click: SendInput with a real press dwell, after focusing
    the target window. SetCursorPos spans all monitors incl. negative coords."""
    move_cursor(x, y)
    _focus_window_at(x, y)
    time.sleep(0.08)
    move_cursor(x, y)  # re-assert position in case focusing nudged things
    _send_mouse(MOUSEEVENTF_LEFTDOWN)
    time.sleep(0.09)   # dwell so the target registers a genuine press+release
    _send_mouse(MOUSEEVENTF_LEFTUP)


def make_click_through(win, colorkey=(255, 255, 255)):
    """Add WS_EX_TRANSPARENT so the overlay never intercepts mouse input, then
    RE-APPLY the color key (changing ex-style clears the -transparentcolor key,
    which would otherwise leave a solid black window)."""
    win.update_idletasks()
    GWL_EXSTYLE = -20
    WS_EX_LAYERED = 0x00080000
    WS_EX_TRANSPARENT = 0x00000020
    LWA_COLORKEY = 0x1
    hwnd = win.winfo_id()
    cur = user32.GetWindowLongW(hwnd, GWL_EXSTYLE)
    user32.SetWindowLongW(hwnd, GWL_EXSTYLE,
                          cur | WS_EX_LAYERED | WS_EX_TRANSPARENT)
    r, g, b = colorkey
    user32.SetLayeredWindowAttributes(hwnd, r | (g << 8) | (b << 16), 0,
                                      LWA_COLORKEY)


# ---------- OCR (Windows built-in, NO PNG round-trip) -------------
_ocr_engine = None


def _engine():
    """Create the OCR engine once and reuse it (creation isn't free)."""
    global _ocr_engine
    if _ocr_engine is None:
        _ocr_engine = OcrEngine.try_create_from_user_profile_languages()
        if _ocr_engine is None:
            raise RuntimeError("No OCR language pack available on this system.")
    return _ocr_engine


def _max_dim():
    """The OCR engine rejects images whose width or height exceeds this."""
    try:
        return int(OcrEngine.max_image_dimension)
    except Exception:
        return 10000


def _effective_scale(shot, scale):
    """Clamp the requested upscale so neither dimension exceeds the engine's
    limit (otherwise recognize_async throws 'parameter is incorrect')."""
    if scale <= 1.0:
        return 1.0
    cap = _max_dim() / max(shot.width, shot.height)
    return max(1.0, min(scale, cap))


def _bitmap_from_shot(shot, scale=1.0):
    """Build a BGRA8 SoftwareBitmap from mss's raw pixels. scale>1 upscales the
    image first (small/thin text reads far better when enlarged). At scale==1 we
    keep the fast no-copy path (opt #2). scale must already be clamped."""
    if scale == 1.0:
        bgra, w, h = bytes(shot.bgra), shot.width, shot.height
    else:
        img = Image.frombytes("RGB", (shot.width, shot.height),
                              shot.bgra, "raw", "BGRX")
        w, h = int(shot.width * scale), int(shot.height * scale)
        img = img.resize((w, h), Image.LANCZOS)
        bgra = img.convert("RGBA").tobytes("raw", "BGRA")
    writer = DataWriter()
    writer.write_bytes(bgra)
    buf = writer.detach_buffer()
    return SoftwareBitmap.create_copy_from_buffer(
        buf, BitmapPixelFormat.BGRA8, w, h)


async def _recognize(bitmap):
    return await _engine().recognize_async(bitmap)


def ocr_words(shot, scale=1.0):
    """OCR the shot (optionally upscaled). Bounding boxes are divided back by
    scale so coordinates stay in the original capture's pixel space."""
    scale = _effective_scale(shot, scale)
    result = asyncio.run(_recognize(_bitmap_from_shot(shot, scale)))
    inv = 1.0 / scale
    words = []
    for line_no, line in enumerate(result.lines):
        for word_no, w in enumerate(line.words):
            r = w.bounding_rect
            words.append({"text": w.text, "x": r.x * inv, "y": r.y * inv,
                          "w": r.width * inv, "h": r.height * inv,
                          "line": line_no, "word": word_no})
    return words


# ---------- screen capture (active monitor by default, opt #1) ----
def _cursor_monitor(sct):
    """The single monitor the mouse cursor is currently on."""
    pt = wintypes.POINT()
    user32.GetCursorPos(ctypes.byref(pt))
    for m in sct.monitors[1:]:
        if (m["left"] <= pt.x < m["left"] + m["width"]
                and m["top"] <= pt.y < m["top"] + m["height"]):
            return m
    return sct.monitors[0]


def grab_screen(all_monitors=False):
    with mss.mss() as sct:
        mon = sct.monitors[0] if all_monitors else _cursor_monitor(sct)
        shot = sct.grab(mon)
        region = (mon["left"], mon["top"], mon["width"], mon["height"])
        return shot, region


def _capture_region(mon):
    return (mon["left"], mon["top"], mon["width"], mon["height"])


def _offset_words(words, mon, base_region, line_offset=0):
    """Move OCR words from monitor-local coordinates into base-region space."""
    dx = mon["left"] - base_region[0]
    dy = mon["top"] - base_region[1]
    moved = []
    for w in words:
        ww = dict(w)
        ww["x"] += dx
        ww["y"] += dy
        ww["line"] = ww.get("line", 0) + line_offset
        moved.append(ww)
    next_line = max((w.get("line", 0) for w in moved), default=line_offset) + 1
    return moved, next_line


def _snapshot_from_words(words, region, mode, complete):
    for w in words:
        w["n"] = _norm(w["text"])
    return {
        "words": words,
        "candidates": build_text_candidates(words),
        "region": region,
        "mode": mode,
        "complete": complete,
    }


def _norm(s):
    return re.sub(r"[^\w]", "", s.lower())


HINT_KEYS = "asdfjklghqwertyuiopzxcvbnm"
MAX_PHRASE_WORDS = 6
INACTIVE_MONITOR_SCALE = 1.25


def _candidate_from_words(parts):
    x1 = min(w["x"] for w in parts)
    y1 = min(w["y"] for w in parts)
    x2 = max(w["x"] + w["w"] for w in parts)
    y2 = max(w["y"] + w["h"] for w in parts)
    text = " ".join(w["text"] for w in parts)
    return {
        "text": text,
        "x": x1,
        "y": y1,
        "w": x2 - x1,
        "h": y2 - y1,
        "n": _norm(text),
        "line": parts[0].get("line", 0),
        "word": parts[0].get("word", 0),
        "word_count": len(parts),
    }


def build_text_candidates(words, max_words=MAX_PHRASE_WORDS):
    """Build single-word and adjacent same-line phrase targets.

    OCR returns words, but visible UI labels often span words. Candidate text is
    normalized without spaces, so "openf" and "open f" can match "Open File".
    """
    lines = {}
    for fallback_index, w in enumerate(words):
        line_no = w.get("line", 0)
        ww = dict(w)
        ww.setdefault("word", fallback_index)
        lines.setdefault(line_no, []).append(ww)

    candidates = []
    for line_no in sorted(lines):
        line = sorted(lines[line_no], key=lambda w: (w.get("word", 0), w["x"]))
        for start in range(len(line)):
            parts = []
            for end in range(start, min(len(line), start + max_words)):
                parts.append(line[end])
                c = _candidate_from_words(parts)
                if c["n"]:
                    candidates.append(c)
    candidates.sort(key=lambda c: (c["y"], c["x"], c["word_count"]))
    return candidates


def _text_prefix_matches(query, candidates, exact=False):
    """Return the shortest viable candidate for each same-line start word."""
    by_start = {}
    for c in candidates:
        hit = (c["n"] == query) if exact else c["n"].startswith(query)
        if not hit:
            continue
        key = (c["line"], c["word"])
        prev = by_start.get(key)
        if prev is None or c["word_count"] < prev["word_count"]:
            by_start[key] = c
    return sorted(by_start.values(), key=lambda c: (c["y"], c["x"]))


def _hint_code(index, first_chars):
    first_chars = first_chars or HINT_KEYS
    second_chars = HINT_KEYS
    width = len(first_chars)
    if index < width * len(second_chars):
        return first_chars[index % width] + second_chars[index // width]
    index -= width * len(second_chars)
    return (first_chars[index % width]
            + second_chars[index // width % len(second_chars)]
            + second_chars[index // (width * len(second_chars))
                           % len(second_chars)])


def assign_hints(matches, base_query):
    """Attach collision-aware selector suffixes to currently highlighted matches."""
    next_chars = {
        m["n"][len(base_query)]
        for m in matches
        if len(m["n"]) > len(base_query)
    }
    first_chars = "".join(k for k in HINT_KEYS if k not in next_chars)
    hinted = []
    for i, m in enumerate(matches):
        h = _hint_code(i, first_chars)
        mm = dict(m)
        mm["hint"] = h
        mm["selector"] = base_query + h
        mm["hint_base"] = base_query
        hinted.append(mm)
    return hinted


def resolve_selector_matches(query, candidates, hint_context=None, exact=False):
    """Resolve normalized user input into visible text or selector matches.

    Text-prefix matching wins. If the input stops matching visible text, the
    previous text-prefix context is reused and the extra chars filter selector
    suffixes. Enter is handled elsewhere; this function only narrows focus.
    """
    text_matches = _text_prefix_matches(query, candidates, exact=exact)
    if text_matches:
        matches = assign_hints(text_matches, query)
        return matches, {"base": query, "matches": matches}, ""

    if hint_context is not None and query.startswith(hint_context["base"]):
        hint_suffix = query[len(hint_context["base"]):]
        matches = [
            dict(m, selector=hint_context["base"] + m["hint"])
            for m in hint_context["matches"]
            if m["hint"].startswith(hint_suffix)
        ]
        return matches, hint_context, hint_suffix

    return [], None, ""


def search(query, all_monitors=False, whole_word=True, scale=1.0):
    """OCR the active monitor (or all, if requested). Return (matches, all_words,
    region). whole_word=True -> exact token match; False -> substring/contains.
    scale>1 upscales before OCR for better accuracy on small text.
    matches carry screen center sx, sy; all_words is every detected word (debug)."""
    q = _norm(query)
    shot, region = grab_screen(all_monitors)
    off_x, off_y = region[0], region[1]
    words = ocr_words(shot, scale)
    matches = []
    for w in words:
        nw = _norm(w["text"])
        hit = (nw == q) if whole_word else (q in nw)
        if q and hit:
            m = dict(w)
            m["sx"] = int(w["x"] + w["w"] / 2 + off_x)
            m["sy"] = int(w["y"] + w["h"] / 2 + off_y)
            matches.append(m)
    return matches, words, region


# ---------- click confirmation ------------------------------------
def flash_click(root, x, y):
    pass


# ---------- GUI ---------------------------------------------------
class App:
    # KeyRelease fires for these too -- ignore them so we don't re-filter.
    NAV_KEYS = {"Tab", "ISO_Left_Tab", "Return", "Escape", "Up", "Down",
                "Left", "Right", "Shift_L", "Shift_R", "Control_L",
                "Control_R", "Alt_L", "Alt_R", "F5"}

    def __init__(self, root, background=False):
        self.root = root
        self._tray_icon = None
        # search state
        self.matches = []
        self.selected = 0
        self.off_x = self.off_y = 0
        self.region = (0, 0, 0, 0)
        self.all_words = []
        self.overlay = None
        self.overlay_cv = None
        self.ov_origin = (0, 0)
        self.snap = None
        self.capturing = False
        self._capture_seq = 0
        self.hint_context = None
        self._last_query = ""
        self._debounce_id = None
        self.popup_visible = False
        self._suppress_hide = False
        self._scan_all_override = None
        self._toggle_q = queue.Queue()

        # shared settings (used by the live search)
        self.all_monitors = tk.BooleanVar(value=True)
        self.whole_word = tk.BooleanVar(value=False)
        self.upscale = tk.BooleanVar(value=True)
        self.debug_all = tk.BooleanVar(value=False)

        self._build_settings()
        self._build_popup()
        self._build_tray()
        self._start_toggle_listener()
        self._poll_toggle()
        if background:
            self.root.withdraw()  # start hidden -> live only in the tray

    # -- settings window (shown when you launch the app) ----------
    def _build_settings(self):
        root = self.root
        root.title("Screen Search - Settings")
        root.geometry("440x300")
        # Closing the window hides it to the tray; quit only via the tray/button.
        root.protocol("WM_DELETE_WINDOW", self._hide_settings)

        frm = ttk.Frame(root, padding=14)
        frm.pack(fill="both", expand=True)

        ttk.Label(frm, text="AutoHotkey:  Alt + F",
                  font=("Segoe UI", 10, "bold")).pack(anchor="w")
        ttk.Label(frm, text="Normal search command:",
                  foreground="#555").pack(anchor="w", pady=(6, 0))
        cmd = ttk.Entry(frm)
        cmd.insert(0, make_toggle_command())
        cmd.configure(state="readonly")
        cmd.pack(fill="x", pady=(4, 6))
        ttk.Label(frm, text="In the search box: type a prefix, keep typing "
                            "selector letters to narrow, Enter to click, "
                            "Esc to close.",
                  foreground="#555", wraplength=410,
                  justify="left").pack(anchor="w", pady=(0, 10))

        ttk.Checkbutton(frm, text="Scan all monitors",
                        variable=self.all_monitors).pack(anchor="w")
        ttk.Checkbutton(frm, text="Exact text only (off = prefix + selectors)",
                        variable=self.whole_word,
                        command=self._refilter).pack(anchor="w")
        ttk.Checkbutton(frm, text="Upscale 2x for small text (more accurate)",
                        variable=self.upscale).pack(anchor="w")
        ttk.Checkbutton(frm, text="Show all OCR words (debug)",
                        variable=self.debug_all,
                        command=self._refilter).pack(anchor="w")

        self.settings_status = ttk.Label(
            frm, text="Running in background. Trigger via your komorebi hotkey.",
            foreground="#888")
        self.settings_status.pack(anchor="w", pady=(12, 0))
        ttk.Button(frm, text="Quit", command=self._quit).pack(anchor="w",
                                                              pady=(8, 0))

    # -- search popup (shown by the hotkey) -----------------------
    def _build_popup(self):
        pop = tk.Toplevel(self.root)
        pop.withdraw()
        pop.overrideredirect(True)         # borderless -> komorebi won't tile it
        pop.attributes("-topmost", True)
        pop.configure(bg="#202020")

        border = tk.Frame(pop, bg="#4a4a4a", padx=2, pady=2)
        border.pack(fill="both", expand=True)
        inner = tk.Frame(border, bg="#202020", padx=12, pady=10)
        inner.pack(fill="both", expand=True)

        self.entry = tk.Entry(inner, font=("Segoe UI", 14), bg="#202020",
                              fg="#f0f0f0", insertbackground="#f0f0f0",
                              relief="flat", highlightthickness=0)
        self.entry.pack(fill="x")
        self.pop_status = tk.Label(inner, text="Type to search.", anchor="w",
                                   bg="#202020", fg="#888", font=("Segoe UI", 9))
        self.pop_status.pack(fill="x", pady=(6, 0))

        self.entry.bind("<KeyRelease>", self._on_type)
        self.entry.bind("<Tab>", self._select_next)
        self.entry.bind("<Shift-Tab>", self._select_prev)
        self.entry.bind("<ISO_Left_Tab>", self._select_prev)
        self.entry.bind("<Escape>", self._hide_popup)
        self.entry.bind("<FocusOut>", self._on_focus_out)
        pop.bind("<Return>", self._on_return)
        pop.bind("<F5>", self._recapture)
        self.popup = pop
        self._make_toolwindow(pop)

    def _make_toolwindow(self, win):
        """Tool window + no taskbar button so komorebi leaves it alone."""
        win.update_idletasks()
        GWL_EXSTYLE = -20
        WS_EX_TOOLWINDOW = 0x00000080
        WS_EX_APPWINDOW = 0x00040000
        hwnd = win.winfo_id()
        cur = user32.GetWindowLongW(hwnd, GWL_EXSTYLE)
        user32.SetWindowLongW(hwnd, GWL_EXSTYLE,
                              (cur | WS_EX_TOOLWINDOW) & ~WS_EX_APPWINDOW)

    def _position_popup(self):
        with mss.mss() as sct:
            mon = _cursor_monitor(sct)  # the monitor the cursor is on
        w, h = 480, 74
        x = mon["left"] + (mon["width"] - w) // 2
        y = mon["top"] + int(mon["height"] * 0.28)
        self.popup.geometry(f"{w}x{h}+{x}+{y}")

    # -- popup show/hide ------------------------------------------
    def _toggle_popup(self, scan_all=None):
        self._hide_popup() if self.popup_visible else self._show_popup(scan_all)

    def _popup_rect(self):
        self.popup.update_idletasks()
        wx, wy = self.popup.winfo_rootx(), self.popup.winfo_rooty()
        return (wx, wy, wx + self.popup.winfo_width(),
                wy + self.popup.winfo_height())

    def _capture_mode(self, scan_all=None):
        all_mon = (scan_all if scan_all is not None else self.all_monitors.get())
        active_scale = 2.0 if self.upscale.get() else 1.0
        inactive_scale = (INACTIVE_MONITOR_SCALE
                          if self.upscale.get() and all_mon else active_scale)
        return (bool(all_mon), active_scale, inactive_scale)

    def _show_popup(self, scan_all=None):
        self._scan_all_override = scan_all
        mode = self._capture_mode(scan_all)
        if self.snap is not None and self.snap.get("mode") != mode:
            self.snap = None
        self.hint_context = None
        self._last_query = ""
        self.selected = 0
        self.entry.delete(0, "end")
        status = "Type to search. Refreshing screen..."
        if self.snap is not None:
            status = "Type to search. Cached results ready; refreshing..."
        self.pop_status.config(text=status)
        self._position_popup()
        self.popup.deiconify()
        self.popup.lift()
        self.popup_visible = True
        # grace window so the open can't trip the focus-loss auto-hide
        self._suppress_hide = True
        self.popup.after(400, lambda: setattr(self, "_suppress_hide", False))
        self.popup.after(10, self._grab_focus)
        self._capture_then_filter(force=True)

    def _grab_focus(self):
        self._force_foreground(self.popup.winfo_id())
        self.entry.focus_force()

    def _force_foreground(self, hwnd):
        """Reliably bring our window to the foreground even when summoned from a
        background process (Windows blocks a plain SetForegroundWindow there)."""
        try:
            fg = user32.GetForegroundWindow()
            target_tid = user32.GetWindowThreadProcessId(fg, None) if fg else 0
            our_tid = kernel32.GetCurrentThreadId()
            attached = target_tid and target_tid != our_tid
            if attached:
                user32.AttachThreadInput(our_tid, target_tid, True)
            user32.BringWindowToTop(hwnd)
            user32.SetForegroundWindow(hwnd)
            if attached:
                user32.AttachThreadInput(our_tid, target_tid, False)
        except Exception:
            pass

    def _hide_popup(self, e=None):
        self._close_overlay()
        self.popup.withdraw()
        self.popup_visible = False
        self._scan_all_override = None
        return "break"

    def _on_focus_out(self, e=None):
        # Defer the check: internal focus shuffles (e.g. our overlay mapping)
        # briefly drop focus, so only dismiss if focus truly left our process.
        self.root.after(120, self._check_focus)

    def _check_focus(self):
        if not self.popup_visible or self._suppress_hide:
            return
        fg = user32.GetForegroundWindow()
        ours = {self.popup.winfo_id(), self.root.winfo_id()}
        if self.overlay is not None:
            ours.add(self.overlay.winfo_id())
        if fg not in ours:
            self._hide_popup()   # focus left us entirely -> dismiss

    # -- system tray ----------------------------------------------
    def _build_tray(self):
        img = Image.new("RGBA", (64, 64), (0, 0, 0, 0))
        d = ImageDraw.Draw(img)
        d.ellipse((12, 12, 44, 44), outline=(70, 160, 255, 255), width=6)  # lens
        d.line((40, 40, 56, 56), fill=(70, 160, 255, 255), width=8)        # handle
        menu = pystray.Menu(
            pystray.MenuItem("Search", self._tray_search, default=True),
            pystray.MenuItem("Settings", self._tray_settings),
            pystray.MenuItem("Quit", self._tray_quit),
        )
        self._tray_icon = pystray.Icon("ScreenSearch", img, "Screen Search", menu)
        threading.Thread(target=self._tray_icon.run, daemon=True).start()

    # tray callbacks run on pystray's thread -> hand off to the Tk thread
    def _tray_search(self, icon=None, item=None):
        self._toggle_q.put("search")

    def _tray_settings(self, icon=None, item=None):
        self._toggle_q.put("settings")

    def _tray_quit(self, icon=None, item=None):
        self._toggle_q.put("quit")

    def _show_settings(self):
        self.root.deiconify()
        self.root.lift()
        self.root.focus_force()

    def _hide_settings(self):
        self.root.withdraw()

    # -- external triggers from komorebi.ahk ----------------------
    def _start_toggle_listener(self):
        self._toggle_event = kernel32.CreateEventW(None, False, False, EVENT_NAME)
        self._toggle_all_event = kernel32.CreateEventW(
            None, False, False, EVENT_ALL_NAME)
        threading.Thread(
            target=self._toggle_wait_loop,
            args=(self._toggle_event, "toggle"),
            daemon=True,
        ).start()
        threading.Thread(
            target=self._toggle_wait_loop,
            args=(self._toggle_all_event, "toggle_all"),
            daemon=True,
        ).start()

    def _toggle_wait_loop(self, event_handle, action):
        INFINITE = 0xFFFFFFFF
        while True:
            if kernel32.WaitForSingleObject(event_handle, INFINITE) == 0:
                self._toggle_q.put(action)

    def _poll_toggle(self):
        actions = []
        try:
            while True:
                actions.append(self._toggle_q.get_nowait())
        except queue.Empty:
            pass
        for a in actions:
            if a == "toggle":
                self._toggle_popup()
            elif a == "toggle_all":
                self._toggle_popup(scan_all=True)
            elif a == "search":
                self._show_popup()
            elif a == "settings":
                self._show_settings()
            elif a == "quit":
                self._quit()
                return
        self.root.after(150, self._poll_toggle)

    def _quit(self):
        if self._tray_icon is not None:
            self._tray_icon.stop()
        self.root.destroy()

    def set_settings_status(self, msg):
        self.root.after(0, lambda: self.settings_status.config(text=msg))

    def set_status(self, msg):
        self.root.after(0, lambda: self.pop_status.config(text=msg))

    @property
    def overlay_active(self):
        return self.overlay is not None

    # -- live typing -----------------------------------------------
    def _on_type(self, e=None):
        if e is not None and e.keysym in self.NAV_KEYS:
            return
        # OCR is expensive; cached filtering is cheap. Keep the first-capture
        # debounce modest, but make cache-backed selector/filtering feel live.
        if self._debounce_id is not None:
            self.root.after_cancel(self._debounce_id)
        delay = 50 if self.snap is not None else 100
        self._debounce_id = self.root.after(delay, self._do_type)

    def _do_type(self):
        self._debounce_id = None
        text = self.entry.get().strip()
        if text == self._last_query:
            return
        self._last_query = text
        if not text:
            self._close_overlay()
            self.set_status("Type to search.")
            return
        if self.snap is None:
            if not self.capturing:
                self._capture_then_filter()   # OCR once, then filter
            else:
                self.set_status("Reading screen...")
            return
        self._live_filter()                   # filter cached snapshot (instant)

    def _capture_then_filter(self, force=False):
        """Refresh OCR off the UI thread.

        All-monitor + upscale mode publishes the active monitor first at 2x,
        then replaces it with a full-desktop snapshot after lower-scale OCR of
        the remaining monitors completes.
        """
        if self.capturing and not force:
            return
        self._capture_seq += 1
        seq = self._capture_seq
        self.capturing = True
        self.set_status("Reading screen...")
        mode = self._capture_mode(self._scan_all_override)
        all_mon, active_scale, inactive_scale = mode

        def publish(words, region, complete):
            snap = _snapshot_from_words(words, region, mode, complete)
            self.root.after(0, lambda: self._accept_snapshot(seq, snap))

        def fail(ex):
            self.root.after(0, lambda: self._capture_failed(seq, ex))

        def work():
            try:
                with mss.mss() as sct:
                    active = dict(_cursor_monitor(sct))
                    if not all_mon:
                        shot = sct.grab(active)
                        words = ocr_words(shot, active_scale)
                        publish(words, _capture_region(active), True)
                        return

                    base = dict(sct.monitors[0])
                    base_region = _capture_region(base)
                    monitors = [dict(m) for m in sct.monitors[1:]]
                    active_key = (active["left"], active["top"],
                                  active["width"], active["height"])

                    ordered = []
                    for m in monitors:
                        key = (m["left"], m["top"], m["width"], m["height"])
                        if key == active_key:
                            ordered.insert(0, m)
                        else:
                            ordered.append(m)

                    all_words = []
                    line_offset = 0
                    for idx, mon in enumerate(ordered):
                        scale = active_scale if idx == 0 else inactive_scale
                        shot = sct.grab(mon)
                        words = ocr_words(shot, scale)
                        moved, line_offset = _offset_words(
                            words, mon, base_region, line_offset)
                        all_words.extend(moved)
                        if idx == 0:
                            publish(list(all_words), base_region, False)
                    publish(all_words, base_region, True)
            except Exception as ex:
                fail(ex)

        threading.Thread(target=work, daemon=True).start()

    def _accept_snapshot(self, seq, snap):
        if seq != self._capture_seq:
            return
        self.snap = snap
        self.capturing = not snap.get("complete", True)
        if self.entry.get().strip():
            self._live_filter()
            return
        if snap.get("complete", True):
            self.set_status("Type to search.")
        else:
            self.set_status("Type to search. Active monitor ready; scanning others...")

    def _capture_failed(self, seq, ex):
        if seq != self._capture_seq:
            return
        self.capturing = False
        self.set_status(f"OCR error: {ex}")

    def _refilter(self):
        """Re-run the filter on the cached snapshot (e.g. a toggle changed)."""
        if self.snap is not None and self.entry.get().strip():
            self._live_filter()

    def _live_filter(self):
        if self.snap is None:
            return
        text = self.entry.get().strip()
        q = _norm(text)
        if not q:
            self.hint_context = None
            self._close_overlay()
            return
        whole = self.whole_word.get()
        region = self.snap["region"]
        off_x, off_y = region[0], region[1]
        wr = self._popup_rect()
        words = self.snap["words"]
        candidates = self.snap["candidates"]
        matches, self.hint_context, hint_suffix = resolve_selector_matches(
            q, candidates, self.hint_context, exact=whole)

        filtered = []
        for m in matches:
            sx = int(m["x"] + m["w"] / 2 + off_x)
            sy = int(m["y"] + m["h"] / 2 + off_y)
            if wr[0] <= sx <= wr[2] and wr[1] <= sy <= wr[3]:
                continue
            m = dict(m)
            m["sx"], m["sy"] = sx, sy
            m["hint_typed"] = hint_suffix
            filtered.append(m)

        self.matches = filtered
        self.region = region
        self.off_x, self.off_y = off_x, off_y
        self.all_words = words
        if self.selected >= len(filtered):
            self.selected = 0
        if len(filtered) == 1:
            self.selected = 0

        if not filtered and not self.debug_all.get():
            self._close_overlay()
            freshness = " yet" if not self.snap.get("complete", True) else ""
            self.set_status(f"No match{freshness} for '{text}' "
                            f"({len(words)} words read).")
            return
        self._ensure_overlay()
        self._draw_overlay()
        if filtered:
            mode = "selector" if hint_suffix else "text"
            freshness = "" if self.snap.get("complete", True) else " (partial)"
            self.set_status(f"{len(filtered)} {mode} match(es){freshness} "
                            f"for '{text}'. Type selector letters; Enter = click.")
        else:
            freshness = "" if self.snap.get("complete", True) else " yet"
            self.set_status(f"No match{freshness} -- showing all "
                            f"{len(words)} OCR words.")

    def _recapture(self, e=None):
        """Force a fresh OCR snapshot (use after the screen behind changed)."""
        self.snap = None
        self.hint_context = None
        self._close_overlay()
        if self.entry.get().strip():
            self._capture_then_filter()
        return "break"

    def _ensure_overlay(self):
        if self.overlay is None:
            self._open_overlay()
            self.overlay.deiconify()  # entry keeps focus -> typing continues

    # -- keys ------------------------------------------------------
    def _on_return(self, e=None):
        if self.overlay_active and self.matches:
            return self._confirm()
        return "break"

    def _select_next(self, e=None):
        if self.overlay_active and self.matches:
            self.selected = (self.selected + 1) % len(self.matches)
            self._draw_overlay()
        return "break"

    def _select_prev(self, e=None):
        if self.overlay_active and self.matches:
            self.selected = (self.selected - 1) % len(self.matches)
            self._draw_overlay()
        return "break"

    def _confirm(self, e=None):
        if not (self.overlay_active and self.matches):
            return "break"
        m = self.matches[self.selected]
        self.snap = None  # screen will likely change after the click -> re-OCR
        threading.Thread(target=lambda: self._click_match(m),
                         daemon=True).start()
        self._hide_popup()  # dismiss the search box after acting
        return "break"

    # -- persistent overlay ---------------------------------------
    def _open_overlay(self):
        # Cover ONLY the captured region, not the whole desktop, so a stray
        # frame can't blink across every monitor.
        rx, ry, rw, rh = self.region
        self.ov_origin = (rx, ry)
        ov = tk.Toplevel(self.root)
        ov.withdraw()  # stay hidden while we set up transparency -> no opaque flash
        ov.overrideredirect(True)
        ov.attributes("-topmost", True)
        ov.geometry(f"{rw}x{rh}+{rx}+{ry}")
        ov.config(bg="white")
        ov.attributes("-transparentcolor", "white")
        cv = tk.Canvas(ov, bg="white", highlightthickness=0)
        cv.pack(fill="both", expand=True)
        make_click_through(ov)
        self.overlay, self.overlay_cv = ov, cv

    def _draw_overlay(self):
        cv = self.overlay_cv
        cv.delete("all")
        ox, oy = self.ov_origin
        # debug: faint outline around EVERY detected word, so you can see what
        # OCR actually read (an unboxed word = OCR didn't detect it).
        if self.debug_all.get():
            for w in self.all_words:
                x1 = w["x"] + self.off_x - ox
                y1 = w["y"] + self.off_y - oy
                cv.create_rectangle(x1, y1, x1 + w["w"], y1 + w["h"],
                                    outline="#3a7bd5", width=1)
        for i, m in enumerate(self.matches):
            x1 = m["x"] + self.off_x - ox
            y1 = m["y"] + self.off_y - oy
            x2, y2 = x1 + m["w"], y1 + m["h"]
            sel = (i == self.selected)
            color = "#22c55e" if sel else "#fb923c"
            cv.create_rectangle(x1 - 4, y1 - 4, x2 + 4, y2 + 4,
                                outline="#111827", width=5 if sel else 3)
            cv.create_rectangle(x1 - 3, y1 - 3, x2 + 3, y2 + 3,
                                outline=color, width=3 if sel else 2)
            hint = m.get("hint")
            if hint:
                label_y = y1 - 6 if y1 > 22 else y2 + 16
                label = hint.upper()
                bg = "#22c55e" if sel else "#fb923c"
                fg = "#111827"
                text_id = cv.create_text(
                    x1 - 3, label_y, text=label, anchor="sw",
                    fill=fg, font=("Segoe UI", 10, "bold"))
                bx1, by1, bx2, by2 = cv.bbox(text_id)
                cv.create_rectangle(bx1 - 3, by1 - 1, bx2 + 3, by2 + 1,
                                    fill=bg, outline="#f8fafc", width=1)
                cv.tag_raise(text_id)

    def _close_overlay(self):
        if self.overlay is not None:
            self.overlay.destroy()
            self.overlay = self.overlay_cv = None

    # -- click selected match -------------------------------------
    def _click_match(self, m):
        # No window hiding needed: self-matches are filtered out at search time,
        # so the target is always outside our window -- no blink.
        sx, sy = m["sx"], m["sy"]
        click_at(sx, sy)
        self.root.after(0, lambda: flash_click(self.root, sx, sy))


def main():
    toggle = "--toggle" in sys.argv
    toggle_all = "--toggle-all" in sys.argv
    background = "--background" in sys.argv
    # If a resident instance exists, just signal it and exit.
    event_name = EVENT_ALL_NAME if toggle_all else EVENT_NAME
    if (toggle or toggle_all) and _signal_event(event_name):
        return
    # Become the single resident instance (named mutex).
    kernel32.CreateMutexW(None, False, MUTEX_NAME)
    if kernel32.GetLastError() == 183:  # ERROR_ALREADY_EXISTS
        if toggle or toggle_all:
            _signal_event(event_name)
        return
    root = tk.Tk()
    # A toggle command with no resident cold-starts it and opens immediately.
    app = App(root, background=background or toggle or toggle_all)
    if toggle or toggle_all:
        root.after(300, lambda: app._show_popup(scan_all=True)
                   if toggle_all else app._show_popup())
    root.mainloop()


if __name__ == "__main__":
    main()
