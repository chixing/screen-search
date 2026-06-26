# Screen Search

A Windows utility that lets you find visible on-screen text with OCR and click it from the keyboard. Summon a search box with a hotkey, type visible text from a word or phrase, narrow with the displayed selector letters, and press Enter to click the selected result.

Stable app: [`screen_click_gui.py`](screen_click_gui.py). Python 3.14, Windows only. Matcher tests live in [`test_matcher.py`](test_matcher.py).

Rust port: [`rust/screen-search-rs`](rust/screen-search-rs). This is the native rewrite branch implementation: Win32 resident/popup, GDI screen capture, Windows Runtime OCR, Rust matcher/hints, and `SendInput` clicking. The native overlay is currently disabled because the first layered-window smoke test produced an opaque full-screen surface when transparency failed.

---

## How it works (architecture)

```
Resident process (single instance, named mutex)
├─ System tray icon (Python only)     → Search / Settings / Quit
├─ Settings window (Python only)      → config toggles; close = hide to tray
├─ Search popup                       → the live search box
├─ Highlight overlay (click-through)  → boxes over matches
├─ AutoHotkey  Alt+F                  → --toggle
└─ Named Win32 events                 → signal/cold-start the resident
```

### The search pipeline
1. **Capture** — `mss` grabs all monitors by default, or only the monitor under the cursor if that setting is disabled. Raw BGRA pixels.
2. **OCR** — Windows' built-in engine (`Windows.Media.Ocr` via `winsdk`). The raw BGRA is turned straight into a `SoftwareBitmap` (no PNG encode/decode). Optional 2× upscale for small text (clamped to the engine's 10000 px max dimension).
3. **Snapshot cache** — OCR starts as soon as the popup opens. If a previous snapshot exists, it is reused immediately while a fresh snapshot refreshes in the background. Cached filtering uses a short 50 ms debounce; no re-OCR per keystroke.
4. **Match + select** — OCR words are grouped into same-line phrase candidates. Spaces and punctuation are ignored for matching, so `openf` and `open f` can both match `Open File`. Matching works from the beginning or middle of normalized text. Each highlighted match gets a short selector suffix; typing selector letters disqualifies nonmatching highlights but never clicks.
5. **Highlight** — a click-through (`WS_EX_TRANSPARENT`) overlay covering the captured region draws a box per match; the selected one is green and other matches are orange. Overlay labels show only the selector suffix.
6. **Click** — `SendInput` with a short press dwell, after bringing the target window to the foreground. Works across all monitors including negative coordinates.

### Key design decisions
- **All-monitor OCR by default** — Alt+F searches the whole virtual desktop. With upscale enabled, the active monitor is OCR'd first at 2×, then the remaining monitors are OCR'd at 1.25× and merged in when complete. A high-quality background pass then re-OCRs all monitors at 2× and 3× requested scales, clamped by Windows OCR's max image size, and merges any additional readings.
- **No PNG round-trip** — building the bitmap from raw bytes saved ~28%.
- **Cache + filter in memory** — text/selector filtering as you type stays instant.
- **Progressive refresh** — broad searches can show active-monitor matches first, expand when the fast all-monitor OCR completes, then improve again when the high-quality pass finishes.
- **Enter is the only action key** — typed selector letters only focus/narrow highlights. They do not click.
- **Overlays are input-transparent** — so the synthetic click passes through to the real target, and the highlight never steals the click.
- **Language**: OCR remains the main cost, but the Rust port removes Python/Tk from the hot path and should materially improve cold start, resident toggle latency, idle footprint, and packaging.

---

## Running it

### Dependencies
```
pip install mss pillow winsdk pystray
```
(Windows built-in OCR language pack must be present — it is by default.)

Rust port:

```powershell
cd rust\screen-search-rs
cargo build --release
```

### Launch modes
| Command | Behaviour |
|---|---|
| `python screen_click_gui.py` | Resident; shows the Settings window. |
| `pythonw screen_click_gui.py --background` | Resident, **hidden**, lives in the tray. Use for autostart. |
| `python screen_click_gui.py --toggle` | Signals the running resident to toggle the search popup (or cold-starts one). For komorebi.ahk. |
| `python screen_click_gui.py --toggle-all` | Compatibility/debug path for forcing an all-monitor search. |

Rust equivalents:

| Command | Behaviour |
|---|---|
| `rust\screen-search-rs\target\release\screen-search-rs.exe --toggle` | Signals the running Rust resident or cold-starts it and opens search. |
| `rust\screen-search-rs\target\release\screen-search-rs.exe --toggle-all` | Compatibility/debug path for forcing all-monitor search. |
| `rust\screen-search-rs\target\release\screen-search-rs.exe --quit` | Gracefully exits the Rust resident. |

The Rust port currently uses fixed defaults: scan all monitors on, contains matching on, 2× upscale on, debug OCR boxes off. Tray/settings persistence are still Python-only. Highlight overlay rendering is disabled until the layered-window transparency path is replaced or proven safe.

### Using the search popup
- **Alt+F** is owned by `komorebi.ahk` and runs `--toggle`.
- Tray → Search and `--toggle` use the configured monitor setting.
- Type visible text from the beginning or middle of a word/phrase. Matches highlight live with selector labels.
- Keep typing selector letters to disqualify other highlights. Spaces in your input do not break phrase matches.
- **Tab / Shift+Tab** cycle the selection.
- **Enter** clicks the selected match.
- **Esc** or clicking away dismisses it.
- **F5** re-captures the screen (use if the screen behind changed).

### Settings (tray → Settings, or launch with no flag)
- **Scan all monitors** — on by default; OCR the whole virtual desktop instead of just the active monitor.
- **Exact text only** — off = contains + selector matching (default), on = exact normalized text.
- **Upscale 2× for small text** — better OCR accuracy on thin/terminal fonts (default on).
- **Show all OCR words (debug)** — faint blue box around every detected word, to tell an OCR miss from a match miss.

### Search behavior

The popup matches OCR words and adjacent same-line phrases. Matching is normalized: case, spaces, and punctuation are ignored. For example, `open f` and `openf` both match `Open File`, and `tings` matches `Settings`. Selector suffixes are generated for the currently highlighted matches; typing selector characters narrows/focuses the highlight, and Enter performs the click.

---

## Komorebi integration

The active `komorebi.ahk` owns the global hotkey:

```ahk
ScreenSearch(mode) {
    script := EnvGet("USERPROFILE") . "\workspace\screen-search\screen_click_gui.py"
    command := Format('"C:\Python314\pythonw.exe" "{}" {}', script, mode)
    Run(command, , "Hide")
}

!f:: ScreenSearch("--toggle")
```

The first press cold-starts Screen Search. Later presses signal the resident through named Win32 events. Screen Search does not register global hotkeys and does not need its own Windows Startup entry.

The popup is a **borderless tool window with no taskbar button**, so komorebi leaves it floating (untiled) automatically. The Settings window IS a normal window (komorebi may tile it) — add an ignore rule if that bothers you.

**Alt+F intentionally shadows the global File-menu accelerator.**

---

## Footprint
Python resident idle: **~60 MB RAM, ~0 % CPU**. Two kernel-blocked threads wait on the normal and all-monitor events; a 25 ms Tk timer bridges them to the UI loop. No network/disk/GPU.

Alt+F latency has two parts:

- The resident checks its event queue on that 25 ms timer.
- `komorebi.ahk` currently launches `pythonw screen_click_gui.py --toggle` to signal the resident, which adds Python process startup/import overhead. Moving the named-event signal directly into AHK would remove most of that remaining resident-toggle latency.

The Rust port keeps the same named events and mutex names, so the AutoHotkey integration can switch from launching Python to launching `screen-search-rs.exe --toggle`. A later AHK direct `SetEvent` path can still remove even the tiny helper process cost.

---

## Development notes

### Validation

Run these after code edits:

```powershell
python -c "import ast; ast.parse(open('screen_click_gui.py', encoding='utf-8').read())"
python -m py_compile screen_click_gui.py test_matcher.py
python -m unittest test_matcher.py
```

Rust:

```powershell
cd rust\screen-search-rs
cargo fmt --check
cargo test
cargo build --release
```

Restart the resident:

```powershell
Get-CimInstance Win32_Process -Filter "Name='pythonw.exe' OR Name='python.exe'" |
  Where-Object { $_.CommandLine -like '*screen_click_gui.py*' } |
  ForEach-Object { Stop-Process -Id $_.ProcessId -Force }

Start-Process C:\Python314\pythonw.exe `
  -ArgumentList "$HOME\workspace\screen-search\screen_click_gui.py","--toggle" `
  -WindowStyle Hidden
```

### Known constraints

- Windows OCR rejects images over 10,000 px in either dimension; `_effective_scale` clamps upscale accordingly.
- `INACTIVE_MONITOR_SCALE` controls the lower scale used for non-active monitors during the fast all-monitor refresh.
- `HIGH_QUALITY_EXTRA_SCALES` controls the requested scales for the slower all-monitor background OCR pass.
- Overlay windows must remain `WS_EX_TRANSPARENT`, and the color key must be reapplied through `make_click_through`.
- The OS OCR engine is the main performance cost.
- Per-monitor DPI awareness is not implemented yet. Mixed monitor scaling can cause overlay/click coordinate drift.
- Do not add UI Automation; OCR is the intended recognition and targeting mechanism.
- Rust port currently has no tray/settings window.
- Rust overlay rendering is disabled for safety after a failed smoke test made the full-screen overlay opaque.

### Remaining work

- Replace or fix the Rust overlay implementation before any more runtime smoke tests. Do not create a virtual-desktop-sized layered window unless transparency is proven on a small test window first.
- Physical UX pass for the Rust popup/overlay after the overlay is safe.
- Confirm physical Alt+F cold-start/resident signaling and default all-monitor capture in Rust.
- Add OCR preprocessing variants for terminal/dark/thin text.
- Persist settings across restarts.
- Optional packaging for the Rust executable.

---
