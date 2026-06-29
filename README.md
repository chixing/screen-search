# Screen Search

A native Windows utility for finding visible on-screen text with OCR and targeting it from the keyboard. Press Alt+F, type text from the screen, narrow with the displayed selector letters, then move or click the selected target.

The active implementation is the Rust app in [`rust/screen-search-rs`](rust/screen-search-rs).

## Architecture

```text
Rust resident process
├─ single instance guard          → named mutex
├─ AutoHotkey Alt+F               → screen-search-rs.exe --toggle
├─ named Win32 events             → wake an existing resident
├─ system tray menu               → search/settings toggles/quit
├─ persisted config               → %APPDATA%\ScreenSearch\config.ini
├─ search popup                   → live query input
├─ Windows Runtime OCR            → text detection
├─ GDI capture                    → all-monitor screenshots
├─ click-through overlay          → visible match boxes/selectors
└─ SendInput                      → optional click on Ctrl+Enter / Ctrl+Shift+Enter
```

## Search behavior

- Alt+F searches all monitors by default.
- OCR starts when the popup opens. For all-monitor search, every monitor is OCR'd first at 1×, then refined with 2× and 3× passes. Upscaled passes also merge a high-contrast OCR variant to improve small light-on-dark text.
- Matching is normalized: case, spaces, and punctuation are ignored.
- Prefix and middle-of-word matching are supported.
- Same-line words are grouped into phrase candidates, so `openf` can match `Open File`.
- Hints are generated from the highlighted text, not from a separate hint prefix.
- Typing hint letters narrows/focuses matches.
- Enter moves the mouse to the selected match.
- Ctrl+Enter left-clicks the selected match.
- Ctrl+Shift+Enter right-clicks the selected match.
- Esc dismisses the popup.
- F5 forces a fresh capture.

## Build

```powershell
cd rust\screen-search-rs
cargo build --release
```

The built executable is:

```text
rust\screen-search-rs\target\release\screen-search-rs.exe
```

## Install

Install the release executable to a stable user-local path:

```powershell
.\install.ps1
```

That copies the binary to:

```text
%LOCALAPPDATA%\ScreenSearch\screen-search-rs.exe
```

Use this installed path from AutoHotkey or other launchers. The repo `target\release` path is only a build artifact.

## Run

| Command | Behavior |
| --- | --- |
| `screen-search-rs.exe --toggle` | Start or signal the resident and open search. |
| `screen-search-rs.exe --toggle-all` | Compatibility path for forcing all-monitor search. |
| `screen-search-rs.exe --active-monitor --toggle` | Search only the monitor under the cursor for that launch. |
| `screen-search-rs.exe --quit` | Gracefully exit the resident. |
| `screen-search-rs.exe --test-instance --toggle` | Start an isolated manual test instance without the shared singleton/events. |
| `screen-search-rs.exe --overlay-test` | Run the bounded overlay smoke test. |
| `screen-search-rs.exe --debug` | Enable trace logging for that resident process. |
| `screen-search-rs.exe --bench-ocr` | Benchmark capture/OCR at 1×, 2×, and 3×. |
| `screen-search-rs.exe --dump-ocr` | Dump recognized OCR words and boxes. |
| `screen-search-rs.exe --bench-ocr --quiet` | Write diagnostics without showing a completion dialog. |

Launching without `--toggle` starts the resident and leaves it available from the tray.

## Tray menu

The tray icon provides:

- Open Search
- Scan all monitors
- Upscale OCR
- Show overlay
- Quit

Scan all monitors, upscale OCR, and show overlay default to on.

These settings persist to:

```text
%APPDATA%\ScreenSearch\config.ini
```

## Komorebi / AutoHotkey integration

The active hotkey lives in `C:\Users\chix\workspace\dotfiles\windows\komorebi\komorebi.ahk`:

```ahk
ScreenSearchRust(mode) {
    exe := EnvGet("LOCALAPPDATA") . "\ScreenSearch\screen-search-rs.exe"
    command := Format('"{}" {}', exe, mode)
    Run(command, , "Hide")
}

!f:: ScreenSearchRust("--toggle") ; Alt+F
```

Screen Search does not register its own global hotkey. AutoHotkey owns Alt+F and starts/signals the Rust resident.

## Performance notes

- OCR is the dominant cost.
- Rust removes the previous Python/Tk startup and UI path.
- Capture uses raw pixels rather than a PNG encode/decode round trip.
- Filtering existing OCR results happens in memory and should be effectively instant.
- A broad all-monitor search shows a 1× result set across every monitor first, then merges higher-quality 2× and 3× passes as they finish. The 2×/3× passes include a high-contrast variant for small dark-theme text.
- Windows OCR rejects images over 10,000 px in either dimension, so upscale is clamped.

## Diagnostics

Diagnostics are written under:

```text
%LOCALAPPDATA%\ScreenSearch
```

Useful commands:

```powershell
%LOCALAPPDATA%\ScreenSearch\screen-search-rs.exe --bench-ocr
%LOCALAPPDATA%\ScreenSearch\screen-search-rs.exe --dump-ocr
%LOCALAPPDATA%\ScreenSearch\screen-search-rs.exe --debug
```

`--bench-ocr` writes capture/OCR timings and word counts. `--dump-ocr` writes recognized words and bounding boxes. Add `--quiet` to suppress the completion dialog. `--debug` enables the trace log; trace logging is off by default.

## Development

```powershell
cd rust\screen-search-rs
cargo fmt --check
cargo test
cargo build --release
```

Restart the resident after building:

```powershell
.\install.ps1
```

## Known constraints

- Windows OCR accuracy is still the main quality limit.
- Mixed-DPI monitor setups may need more coordinate validation.
- Packaging is currently a per-user install script, not an MSI/MSIX.
