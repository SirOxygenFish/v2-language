<#
    V2 Language — Windows x64 installer.

    Installs the `v2` toolchain into your user profile and adds it to PATH, so
    you can run `v2` from any terminal without copying files by hand.

    Usage:
        powershell -ExecutionPolicy Bypass -File install.ps1
        powershell -ExecutionPolicy Bypass -File install.ps1 -Uninstall

    (Or just double-click install.bat.)
#>
param(
    [switch]$Uninstall,
    [string]$InstallDir = (Join-Path $env:LOCALAPPDATA "Programs\v2")
)

$ErrorActionPreference = "Stop"
$BinDir      = Join-Path $InstallDir "bin"
$DocsDir     = Join-Path $InstallDir "docs"
$RegistryDir = Join-Path $InstallDir "registry"
$ScriptDir   = Split-Path -Parent $MyInvocation.MyCommand.Path

function Add-ToUserPath($dir) {
    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if (-not $userPath) { $userPath = "" }
    $parts = $userPath.Split(';') | Where-Object { $_ -ne "" }
    if ($parts -notcontains $dir) {
        $newPath = (@($parts) + $dir) -join ';'
        [Environment]::SetEnvironmentVariable("Path", $newPath, "User")
        return $true
    }
    return $false
}

function Remove-FromUserPath($dir) {
    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if (-not $userPath) { return }
    $parts = $userPath.Split(';') | Where-Object { $_ -ne "" -and $_ -ne $dir }
    [Environment]::SetEnvironmentVariable("Path", ($parts -join ';'), "User")
}

if ($Uninstall) {
    Write-Host "Uninstalling V2..." -ForegroundColor Cyan
    if (Test-Path $InstallDir) { Remove-Item -Recurse -Force $InstallDir }
    Remove-FromUserPath $BinDir
    [Environment]::SetEnvironmentVariable("V2_REGISTRY", $null, "User")
    Write-Host "V2 removed. Restart your terminal for PATH changes to take effect." -ForegroundColor Green
    return
}

Write-Host "Installing the V2 language (Windows x64)..." -ForegroundColor Cyan

# ── Locate the v2.exe to install ────────────────────────────────────────────
$candidates = @(
    (Join-Path $ScriptDir "v2.exe"),
    (Join-Path $ScriptDir "bin\v2.exe"),
    (Join-Path $ScriptDir "v2\target\release\v2.exe"),
    (Join-Path $ScriptDir "target\release\v2.exe")
)
$exe = $candidates | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $exe) {
    Write-Host "ERROR: could not find v2.exe next to this installer." -ForegroundColor Red
    Write-Host "Looked in:`n  $($candidates -join "`n  ")" -ForegroundColor Red
    Write-Host "Build it first with:  cd v2 && cargo build --release" -ForegroundColor Yellow
    exit 1
}
Write-Host "  Using binary: $exe"

# ── Install files ───────────────────────────────────────────────────────────
New-Item -ItemType Directory -Force -Path $BinDir  | Out-Null
New-Item -ItemType Directory -Force -Path $DocsDir | Out-Null
Copy-Item $exe (Join-Path $BinDir "v2.exe") -Force

foreach ($doc in @("DOCS.md","INTERNALS.md","PACKAGES.md","NOT_YET_IMPLEMENTED.md","LIST.md")) {
    $src = Join-Path $ScriptDir $doc
    if (Test-Path $src) { Copy-Item $src (Join-Path $DocsDir $doc) -Force }
}

# Bundle the reference package registry, if present, and point V2 at it.
$srcRegistry = Join-Path $ScriptDir "registry"
if (Test-Path $srcRegistry) {
    if (Test-Path $RegistryDir) { Remove-Item -Recurse -Force $RegistryDir }
    Copy-Item -Recurse $srcRegistry $RegistryDir
    [Environment]::SetEnvironmentVariable("V2_REGISTRY", $RegistryDir, "User")
    $env:V2_REGISTRY = $RegistryDir   # usable in this session too
    Write-Host "  Registry installed and V2_REGISTRY set."
}

# ── PATH ────────────────────────────────────────────────────────────────────
$added = Add-ToUserPath $BinDir
$env:Path = "$env:Path;$BinDir"   # make it usable in this session immediately

# ── Verify ──────────────────────────────────────────────────────────────────
Write-Host ""
Write-Host "Installed to: $InstallDir" -ForegroundColor Green
& (Join-Path $BinDir "v2.exe") --version
Write-Host ""
if ($added) {
    Write-Host "Added to PATH. Open a NEW terminal, then run:  v2 --help" -ForegroundColor Green
} else {
    Write-Host "Already on PATH. Run:  v2 --help" -ForegroundColor Green
}
Write-Host "Docs:      v2 --docs   |   v2 --packages"
Write-Host "Uninstall: powershell -ExecutionPolicy Bypass -File install.ps1 -Uninstall"
