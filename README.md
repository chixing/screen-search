# Screen Search

A native Windows utility for finding visible on-screen text with OCR and clicking it from the keyboard. Press Alt+F, type text from the screen, narrow with the displayed selector letters, and press Enter to click.

The active implementation is the Rust app in [`rust/screen-search-rs`](rust/screen-search-rs).

## Architecture

```text
Rust resident process
├─ single instance guard          → named mutex
├─ AutoHotkey Alt+F               → screen-search-rs.exe --toggle --enable-overlay
├─ named Win32 events             → wake an existing resident
├─ system tray menu               → search/settings toggles/quit
├─ search popup                   → live query input
├─ Windows Runtime OCR            → text detection
├─ GDI capture                    → all-monitor screenshots
├─ click-through overlay          → visible match boxes/selectors
└─ SendInput                      → final click on Enter
```

## Search behavior

- Alt+F searches all monitors by default.
- OCR starts when the popup opens. Existing results can be filtered immediately while fresh OCR finishes in the background.
- Matching is normalized: case, spaces, and punctuation are ignored.
- Prefix and middle-of-word matching are supported.
- Same-line words are grouped into phrase candidates, so `openf` can match `Open File`.
- Hints are generated from the highlighted text, not from a separate hint prefix.
- Typing hint letters narrows/focuses matches. Enter is the only key that clicks.
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

## Run

| Command | Behavior |
| --- | --- |
| `screen-search-rs.exe --toggle --enable-overlay` | Start or signal the resident and open search. |
| `screen-search-rs.exe --toggle-all --enable-overlay` | Compatibility path for forcing all-monitor search. |
| `screen-search-rs.exe --active-monitor --toggle --enable-overlay` | Search only the monitor under the cursor. |
| `screen-search-rs.exe --quit` | Gracefully exit the resident. |
| `screen-search-rs.exe --test-instance --toggle --enable-overlay` | Start an isolated manual test instance without the shared singleton/events. |
| `screen-search-rs.exe --overlay-test` | Run the bounded overlay smoke test. |

Launching without `--toggle` starts the resident and leaves it available from the tray.

## Tray menu

The tray icon provides:

- Open Search
- Scan all monitors
- Upscale OCR
- Show overlay
- Quit

These settings are runtime toggles. Persistent settings are still pending.

## Komorebi / AutoHotkey integration

The active hotkey lives in `C:\Users\chix\workspace\dotfiles\windows\komorebi\komorebi.ahk`:

```ahk
ScreenSearchRust(mode) {
    exe := EnvGet("USERPROFILE") . "\workspace\screen-search\rust\screen-search-rs\target\release\screen-search-rs.exe"
    command := Format('"{}" {} --enable-overlay', exe, mode)
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
- A broad all-monitor search can show active-monitor results first, then merge wider and higher-quality OCR passes as they finish.
- Windows OCR rejects images over 10,000 px in either dimension, so upscale is clamped.

## Development

```powershell
cd rust\screen-search-rs
cargo fmt --check
cargo test
cargo build --release
```

Restart the resident after building:

```powershell
rust\screen-search-rs\target\release\screen-search-rs.exe --quit
rust\screen-search-rs\target\release\screen-search-rs.exe --toggle --enable-overlay
```

## Known constraints

- Windows OCR accuracy is still the main quality limit.
- Mixed-DPI monitor setups may need more coordinate validation.
- Settings toggles are not persisted yet.
- Optional packaging/install flow is still pending.
