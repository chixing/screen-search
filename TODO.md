# Screen Search — TODO

## Required next feature

- [ ] **Multi-word phrase matching.** Match a query such as `open file` across adjacent OCR words on the same line. Build phrase candidates in reading order, tolerate normal inter-word gaps, combine their bounding boxes for highlighting, and click the center of the combined phrase. Preserve existing single-word substring and whole-word behavior.

## Verification

- [ ] **Confirm physical Alt+F cold-start and resident signaling.** The first AHK invocation should launch the resident and open a focused popup; later presses should signal the existing process.
- [ ] **Confirm physical Alt+Shift+F scans all monitors.** The AHK `--toggle-all` command should force all-monitor capture for that popup session without changing the configured checkbox state.

## Robustness
- [ ] **Per-monitor DPI scaling.** Process is not DPI-aware. If monitors run at different scaling (e.g. 100% + 150%), captured pixels and click/overlay coords can drift on the scaled monitor. Fix: `SetProcessDpiAwarenessContext(PER_MONITOR_AWARE_V2)` at startup, then re-verify overlay/cursor math. (Currently fine because relevant monitors are effectively 100%.)
- [ ] **Foreground-grab edge cases.** `AttachThreadInput` is reliable but verify it behaves when the foreground app is elevated / full-screen.
- [ ] **Popup position.** Uses the monitor under the mouse cursor. With komorebi you may want the *keyboard-focused* monitor — read `komorebic query focused-monitor-index` / `komorebic state`, fall back to cursor monitor.

## Features
- [ ] **OCR preprocessing for terminals.** Upscale helps; add contrast/threshold/invert for light-on-dark terminal text (Windows OCR is tuned for dark-on-light).
- [ ] **Right-click / double-click** options on the selected match.
- [ ] **Persist settings** (checkbox states) across restarts — currently in-memory only.

## Packaging / deployment
- [ ] **PyInstaller** `--noconsole --onefile` → `ScreenSearch.exe`. Then komorebi `ignore-rule exe ScreenSearch.exe` becomes precise, and no Python dependency.
- [ ] **komorebi ignore rule** for the Settings window if you don't want it tiled (verify the `ignore_rules` schema against the installed komorebi version).

## Known facts / gotchas
- Win+Alt+G and Ctrl+Alt+G are **already registered by another app** on this machine (RegisterHotKey err 1409) — do not reuse them.
- `komorebi.ahk` owns Alt+F and Alt+Shift+F. Screen Search does not call `RegisterHotKey`.
- Windows OCR **max image dimension is 10000 px** — full-desktop 2× upscale is auto-clamped (~1.37× for the 7280-px desktop); active-monitor gets full 2×.
- No Screen Search Startup entry is required. The first AHK hotkey press cold-starts the resident.
