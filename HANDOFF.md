# Screen Search Handoff

Run future development sessions from:

`C:\Users\chix\workspace\screen-search`

Read [README.md](README.md) for architecture and usage, then [TODO.md](TODO.md) for prioritized work.

## Project

Screen Search is a Windows 11 tray utility implemented in `screen_click_gui.py`. It captures the screen with `mss`, runs Windows OCR through `winsdk`, filters cached OCR results as the user types, highlights matches in a click-through Tk overlay, and clicks the selected match with `SendInput`.

The popup uses prefix + selector matching. OCR words are grouped into adjacent same-line phrase candidates, normalized by removing spaces/punctuation/case, and displayed with selector suffixes. Typing selector letters only narrows/focuses highlights; Enter is the only click action.

The project uses Python 3.14 from `C:\Python314`. Installed dependencies are `mss`, `pillow`, `winsdk`, and `pystray`. It must support multiple monitors, including negative desktop coordinates.

## Current controls

- Alt+F: `komorebi.ahk` runs `--toggle`.
- Tab / Shift+Tab: cycle matches.
- Selector letters: narrow/focus highlighted matches.
- Enter: click the selected match.
- Escape: dismiss.
- F5: recapture.
- `--background`: start hidden in the tray.
- `--toggle`: signal or cold-start search. The default setting scans all monitors.
- `--toggle-all`: compatibility/debug path for forcing all-monitor search.

Alt+F intentionally overrides the global File-menu accelerator.
AutoHotkey owns the global hotkey. Screen Search does not register hotkeys and has no Windows Startup entry; the first hotkey press cold-starts it.

## Current priority

Run a physical UX pass for prefix + selector mode. Validate that labels are readable, selector characters narrow matches quickly enough, phrase bounds click the intended target center, and Alt+F behaves correctly when invoked through the elevated AutoHotkey config.

Do not add UI Automation. OCR remains the intended recognition and targeting mechanism.

## Required validation

After every code edit:

```powershell
python -c "import ast; ast.parse(open('screen_click_gui.py', encoding='utf-8').read())"
python -m py_compile screen_click_gui.py test_matcher.py
python -m unittest test_matcher.py
```

To restart the resident:

```powershell
Get-CimInstance Win32_Process -Filter "Name='pythonw.exe' OR Name='python.exe'" |
  Where-Object { $_.CommandLine -like '*screen_click_gui.py*' } |
  ForEach-Object { Stop-Process -Id $_.ProcessId -Force }

Start-Process C:\Python314\pythonw.exe `
  -ArgumentList "$HOME\workspace\screen-search\screen_click_gui.py","--background" `
  -WindowStyle Hidden
```

Physical verification is still required for the AHK cold-start path, focus retention, and default all-monitor capture.

## Important constraints

- Windows OCR rejects images over 10,000 pixels in either dimension; `_effective_scale` clamps upscale accordingly.
- Overlay windows must remain `WS_EX_TRANSPARENT`, and the color key must be reapplied through `make_click_through`.
- The OS OCR engine is the main performance cost.
