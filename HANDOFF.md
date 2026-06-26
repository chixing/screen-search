# Screen Search Handoff

Run future development sessions from:

`C:\Users\chix\workspace\screen-search`

Read [README.md](README.md) for architecture and usage, then [TODO.md](TODO.md) for prioritized work.

## Project

Screen Search is a Windows 11 tray utility implemented in `screen_click_gui.py`. It captures the screen with `mss`, runs Windows OCR through `winsdk`, filters cached OCR results as the user types, highlights matches in a click-through Tk overlay, and clicks the selected match with `SendInput`.

The project uses Python 3.14 from `C:\Python314`. Installed dependencies are `mss`, `pillow`, `winsdk`, and `pystray`. It must support multiple monitors, including negative desktop coordinates.

## Current controls

- Alt+F: `komorebi.ahk` runs `--toggle`.
- Alt+Shift+F: `komorebi.ahk` runs `--toggle-all`.
- Tab / Shift+Tab: cycle matches.
- Enter: click the selected match.
- Escape: dismiss.
- F5: recapture.
- `--background`: start hidden in the tray.
- `--toggle`: signal or cold-start normal search.
- `--toggle-all`: signal or cold-start all-monitor search.

Alt+F intentionally overrides the global File-menu accelerator.
AutoHotkey owns both global hotkeys. Screen Search does not register hotkeys and has no Windows Startup entry; the first hotkey press cold-starts it.

## Current priority

Implement multi-word phrase matching. OCR currently returns and matches individual words, so a query such as `open file` cannot match adjacent words. Phrase matching must:

1. Group adjacent OCR words on the same line in reading order.
2. Match normalized multi-word queries against those groups.
3. Highlight the union of all word bounds in the phrase.
4. Click the center of the combined phrase bounds.
5. Preserve current single-word substring and whole-word behavior.

Do not add UI Automation. OCR remains the intended recognition and targeting mechanism.

## Required validation

After every code edit:

```powershell
python -c "import ast; ast.parse(open('screen_click_gui.py', encoding='utf-8').read())"
python -m py_compile screen_click_gui.py
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

Physical verification is still required for the AHK cold-start path, focus retention, and Alt+Shift+F all-monitor capture.

## Important constraints

- Windows OCR rejects images over 10,000 pixels in either dimension; `_effective_scale` clamps upscale accordingly.
- Overlay windows must remain `WS_EX_TRANSPARENT`, and the color key must be reapplied through `make_click_through`.
- The OS OCR engine is the main performance cost.
