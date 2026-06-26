$ErrorActionPreference = "Stop"

$projectDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$scriptPath = Join-Path $projectDir "screen_click_gui.py"
$pythonw = "C:\Python314\pythonw.exe"
$startupDir = [Environment]::GetFolderPath("Startup")
$shortcutPath = Join-Path $startupDir "Screen Search.lnk"

if (-not (Test-Path -LiteralPath $pythonw -PathType Leaf)) {
    throw "pythonw.exe not found at $pythonw"
}
if (-not (Test-Path -LiteralPath $scriptPath -PathType Leaf)) {
    throw "Screen Search script not found at $scriptPath"
}

$shell = New-Object -ComObject WScript.Shell
$shortcut = $shell.CreateShortcut($shortcutPath)
$shortcut.TargetPath = $pythonw
$shortcut.Arguments = "`"$scriptPath`" --background"
$shortcut.WorkingDirectory = $projectDir
$shortcut.Description = "Screen Search OCR utility"
$shortcut.Save()

Get-CimInstance Win32_Process -Filter "Name='pythonw.exe' OR Name='python.exe'" |
    Where-Object { $_.CommandLine -like "*screen_click_gui.py*" } |
    ForEach-Object { Stop-Process -Id $_.ProcessId -Force }

Start-Process -FilePath $pythonw `
    -ArgumentList $scriptPath, "--background" `
    -WorkingDirectory $projectDir `
    -WindowStyle Hidden

Write-Output "Installed Startup shortcut and started Screen Search."
