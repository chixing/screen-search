# Screen Search

A Windows utility that lets you find visible on-screen text with OCR and click it from the keyboard. Summon a search box with a hotkey, type a visible text prefix, narrow with the displayed selector letters, and press Enter to click the selected result.

Main app: [`screen_click_gui.py`](screen_click_gui.py). Python 3.14, Windows only. Matcher tests live in [`test_matcher.py`](test_matcher.py).

---

## How it works (architecture)

```
Resident process (single instance, named mutex)
├─ System tray icon (pystray)         → Search / Settings / Quit
├─ Settings window (Tk)               → config toggles; close = hide to tray
├─ Search popup (borderless tool win) → the live search box
├─ Highlight overlay (click-through)  → red boxes over matches
├─ AutoHotkey  Alt+F                  → --toggle
├─ AutoHotkey  Alt+Shift+F            → --toggle-all
└─ Named Win32 events                 → signal/cold-start the resident
```

### The search pipeline
1. **Capture** — `mss` grabs the monitor under the cursor (or all monitors if enabled). Raw BGRA pixels.
2. **OCR** — Windows' built-in engine (`Windows.Media.Ocr` via `winsdk`). The raw BGRA is turned straight into a `SoftwareBitmap` (no PNG encode/decode). Optional 2× upscale for small text (clamped to the engine's 10000 px max dimension).
3. **Snapshot cache** — OCR runs **once** when you start typing; every keystroke filters the cached OCR result in memory (200 ms debounce). No re-OCR per keystroke.
4. **Match + select** — OCR words are grouped into same-line phrase candidates. Spaces and punctuation are ignored for matching, so `openf` and `open f` can both match `Open File`. Each highlighted match gets a short selector suffix; typing selector letters disqualifies nonmatching highlights but never clicks.
5. **Highlight** — a click-through (`WS_EX_TRANSPARENT`) overlay covering the captured monitor draws a box per match; the selected one is blue and other matches are white. Overlay labels show only the selector suffix.
6. **Click** — `SendInput` with a short press dwell, after bringing the target window to the foreground. Works across all monitors including negative coordinates.

### Key design decisions
- **Active-monitor OCR by default** — OCR cost scales with pixels; scanning one monitor instead of the whole 7280-px desktop is ~5× faster (~150 ms vs ~750 ms).
- **No PNG round-trip** — building the bitmap from raw bytes saved ~28%.
- **Cache + filter in memory** — prefix/selector filtering as you type stays instant.
- **Enter is the only action key** — typed selector letters only focus/narrow highlights. They do not click.
- **Overlays are input-transparent** — so the synthetic click passes through to the real target, and the highlight never steals the click.
- **Language**: Python is NOT the bottleneck — the OS OCR engine is, and that's language-independent. A C#/Rust rewrite would only help distribution and a flicker-free native overlay, not raw speed.

---

## Running it

### Dependencies
```
pip install mss pillow winsdk pystray
```
(Windows built-in OCR language pack must be present — it is by default.)

### Launch modes
| Command | Behaviour |
|---|---|
| `python screen_click_gui.py` | Resident; shows the Settings window. |
| `pythonw screen_click_gui.py --background` | Resident, **hidden**, lives in the tray. Use for autostart. |
| `python screen_click_gui.py --toggle` | Signals the running resident to toggle the search popup (or cold-starts one). For komorebi.ahk. |
| `python screen_click_gui.py --toggle-all` | Signals the resident or cold-starts an all-monitor search. |

### Using the search popup
- **Alt+F** is owned by `komorebi.ahk` and runs `--toggle`.
- **Alt+Shift+F** is owned by `komorebi.ahk` and runs `--toggle-all`.
- Tray → Search and `--toggle` use the configured monitor setting.
- Type a visible text prefix. Matches highlight live with selector labels.
- Keep typing selector letters to disqualify other highlights. Spaces in your input do not break phrase matches.
- **Tab / Shift+Tab** cycle the selection (green box).
- **Enter** clicks the selected match.
- **Esc** or clicking away dismisses it.
- **F5** re-captures the screen (use if the screen behind changed).

### Settings (tray → Settings, or launch with no flag)
- **Scan all monitors** — OCR the whole virtual desktop instead of just the active monitor (slower).
- **Exact text only** — off = prefix + selector matching (default), on = exact normalized text.
- **Upscale 2× for small text** — better OCR accuracy on thin/terminal fonts (default on).
- **Show all OCR words (debug)** — faint blue box around every detected word, to tell an OCR miss from a match miss.

### Search behavior

The popup matches OCR words and adjacent same-line phrases. Matching is normalized: case, spaces, and punctuation are ignored. For example, `open f` and `openf` both match `Open File`. Selector suffixes are generated for the currently highlighted matches; typing selector characters narrows/focuses the highlight, and Enter performs the click.

---

## Komorebi integration

The active `komorebi.ahk` is the sole owner of both hotkeys:

```ahk
ScreenSearch(mode) {
    script := EnvGet("USERPROFILE") . "\workspace\screen-search\screen_click_gui.py"
    command := Format('"C:\Python314\pythonw.exe" "{}" {}', script, mode)
    Run(command, , "Hide")
}

!f:: ScreenSearch("--toggle")
!+f:: ScreenSearch("--toggle-all")
```

The first press cold-starts Screen Search. Later presses signal the resident through named Win32 events. Screen Search does not register global hotkeys and does not need its own Windows Startup entry.

The popup is a **borderless tool window with no taskbar button**, so komorebi leaves it floating (untiled) automatically. The Settings window IS a normal window (komorebi may tile it) — add an ignore rule if that bothers you.

**Alt+F intentionally shadows the global File-menu accelerator.**

---

## Footprint
Resident idle: **~60 MB RAM, ~0 % CPU**. Two kernel-blocked threads wait on the normal and all-monitor events; a 150 ms timer bridges them to the Tk loop. No network/disk/GPU.

---

See [TODO.md](TODO.md) for outstanding work and [HANDOFF.md](HANDOFF.md) to resume development.
