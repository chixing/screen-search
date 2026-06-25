# Screen Search

A Windows utility that lets you find visible on-screen text with OCR and click it from the keyboard. Summon a search box with a hotkey, type a query, cycle through highlighted matches, and press Enter to click the selected result.

Single file: [`screen_click_gui.py`](screen_click_gui.py). Python 3.14, Windows only.

---

## How it works (architecture)

```
Resident process (single instance, named mutex)
├─ System tray icon (pystray)         → Search / Settings / Quit
├─ Settings window (Tk)               → config toggles; close = hide to tray
├─ Search popup (borderless tool win) → the live search box
├─ Highlight overlay (click-through)  → red boxes over matches
├─ Built-in hotkey  Alt+F             → normal search
├─ Built-in hotkey  Alt+Shift+F       → force all-monitor search
└─ External trigger  --toggle         → named Win32 event, for komorebi.ahk
```

### The search pipeline
1. **Capture** — `mss` grabs the monitor under the cursor (or all monitors if enabled). Raw BGRA pixels.
2. **OCR** — Windows' built-in engine (`Windows.Media.Ocr` via `winsdk`). The raw BGRA is turned straight into a `SoftwareBitmap` (no PNG encode/decode). Optional 2× upscale for small text (clamped to the engine's 10000 px max dimension).
3. **Snapshot cache** — OCR runs **once** when you start typing; every keystroke filters the cached OCR result in memory (200 ms debounce). No re-OCR per keystroke.
4. **Highlight** — a click-through (`WS_EX_TRANSPARENT`) overlay covering the captured monitor draws a red box per match; the selected one is green.
5. **Click** — `SendInput` with a short press dwell, after bringing the target window to the foreground. Works across all monitors including negative coordinates.

### Key design decisions
- **Active-monitor OCR by default** — OCR cost scales with pixels; scanning one monitor instead of the whole 7280-px desktop is ~5× faster (~150 ms vs ~750 ms).
- **No PNG round-trip** — building the bitmap from raw bytes saved ~28%.
- **Cache + filter in memory** — substring search as you type stays instant.
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

### Using the search popup
- **Alt+F** opens normal search using the configured monitor setting.
- **Alt+Shift+F** opens search and forces an all-monitor scan for that session.
- Tray → Search and `--toggle` use the configured monitor setting.
- Type to filter (substring by default). Matches highlight live.
- **Tab / Shift+Tab** cycle the selection (green box).
- **Enter** clicks the selected match.
- **Esc** or clicking away dismisses it.
- **F5** re-captures the screen (use if the screen behind changed).

### Settings (tray → Settings, or launch with no flag)
- **Scan all monitors** — OCR the whole virtual desktop instead of just the active monitor (slower).
- **Whole word only** — off = substring matching (default), on = exact token match.
- **Upscale 2× for small text** — better OCR accuracy on thin/terminal fonts (default on).
- **Show all OCR words (debug)** — faint blue box around every detected word, to tell an OCR miss from a match miss.

### Current search limitation

Matching currently operates on individual OCR words. A query containing spaces does not yet match a phrase across adjacent words. Multi-word phrase matching is the highest-priority feature in [TODO.md](TODO.md).

---

## Komorebi integration

The user runs **komorebi.ahk** (AutoHotkey). Two options for triggering:

1. **Built-in hotkeys** — Alt+F for normal search and Alt+Shift+F to force all monitors.
2. **AHK-bound `--toggle`** — keeps all keybinds in one file. Autostart the resident hidden, then bind a key:
```ahk
; autostart (run hidden):
Run('"C:\Python314\pythonw.exe" "C:\Users\chix\workspace\screen-search\screen_click_gui.py" --background',, "Hide")
; optional alternative binding to toggle search:
^!f::Run('"C:\Python314\pythonw.exe" "C:\Users\chix\workspace\screen-search\screen_click_gui.py" --toggle',, "Hide")
```
The popup is a **borderless tool window with no taskbar button**, so komorebi leaves it floating (untiled) automatically. The Settings window IS a normal window (komorebi may tile it) — add an ignore rule if that bothers you.

**Alt+F intentionally shadows the global File-menu accelerator.**

---

## Footprint
Resident idle: **~60 MB RAM, ~0 % CPU**. One kernel-blocked thread waits on the toggle event (free); a 150 ms timer bridges that thread to the Tk loop (the only idle heartbeat). No network/disk/GPU.

---

See [TODO.md](TODO.md) for outstanding work and [HANDOFF.md](HANDOFF.md) to resume development.
