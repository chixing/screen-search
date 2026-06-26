param(
    [switch]$NoBuild,
    [switch]$NoStart
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$crateDir = Join-Path $repoRoot "rust\screen-search-rs"
$sourceExe = Join-Path $crateDir "target\release\screen-search-rs.exe"
$installDir = Join-Path $env:LOCALAPPDATA "ScreenSearch"
$installExe = Join-Path $installDir "screen-search-rs.exe"

if (-not $NoBuild) {
    Push-Location $crateDir
    try {
        cargo build --release
    } finally {
        Pop-Location
    }
}

if (-not (Test-Path $sourceExe)) {
    throw "Release executable not found: $sourceExe"
}

if (Test-Path $installExe) {
    & $installExe --quit | Out-Null
}
if (Test-Path $sourceExe) {
    & $sourceExe --quit | Out-Null
}

Get-CimInstance Win32_Process -Filter "Name='screen-search-rs.exe'" |
    ForEach-Object {
        try {
            Stop-Process -Id $_.ProcessId -Force -ErrorAction Stop
        } catch {
            Write-Warning "Could not force-stop screen-search-rs.exe PID $($_.ProcessId): $($_.Exception.Message)"
        }
    }

for ($i = 0; $i -lt 20; $i++) {
    $running = Get-CimInstance Win32_Process -Filter "Name='screen-search-rs.exe'" -ErrorAction SilentlyContinue
    if (-not $running) {
        break
    }
    Start-Sleep -Milliseconds 100
}

if (Get-CimInstance Win32_Process -Filter "Name='screen-search-rs.exe'" -ErrorAction SilentlyContinue) {
    throw "screen-search-rs.exe is still running. Reload/exit the elevated AutoHotkey Screen Search resident, then rerun install.ps1."
}

New-Item -ItemType Directory -Force -Path $installDir | Out-Null
Copy-Item -LiteralPath $sourceExe -Destination $installExe -Force

if (-not $NoStart) {
    Start-Process -FilePath $installExe -WindowStyle Hidden
}

Write-Host "Installed Screen Search to $installExe"
