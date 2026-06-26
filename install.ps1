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

Get-CimInstance Win32_Process -Filter "Name='screen-search-rs.exe'" |
    ForEach-Object { Stop-Process -Id $_.ProcessId -Force }

New-Item -ItemType Directory -Force -Path $installDir | Out-Null
Copy-Item -LiteralPath $sourceExe -Destination $installExe -Force

if (-not $NoStart) {
    Start-Process -FilePath $installExe -WindowStyle Hidden
}

Write-Host "Installed Screen Search to $installExe"
