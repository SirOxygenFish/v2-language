# install.ps1's choices (INTERNALS 22.6), each in a fresh profile folder whose path has a space
# and a non-ASCII letter, run as a file by Windows PowerShell with only the system folders on PATH:
# ~ and relative paths, a folder that is a file, both archive layouts, a damaged archive, a
# v2.exe that is running while it is replaced (twice), another v2 that comes first.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File tests\install\paths.ps1 path\to\v2-....zip [-ModifyPath]
#
# Every case sets V2_NO_MODIFY_PATH, and the user PATH must be unchanged at the end. -ModifyPath
# adds one case that changes the real user PATH and then restores it: only for throwaway machines (CI).
param([Parameter(Mandatory = $true)][string]$Archive, [switch]$ModifyPath)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$Archive = (Resolve-Path -LiteralPath $Archive).Path
$name = Split-Path -Leaf $Archive
$tmp = Join-Path ([IO.Path]::GetTempPath()) ("v2-paths-" + [Guid]::NewGuid().ToString('N'))
$powershell = "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe"
$sysPath = "$env:SystemRoot\System32;$env:SystemRoot;$env:SystemRoot\System32\WindowsPowerShell\v1.0"
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
$script:failed = $false
$script:problems = @()
$script:n = 0
$script:procs = @()

function Fresh {
    $script:n++
    $h = Join-Path $tmp "home $script:n $([char]0xE9)"
    New-Item -ItemType Directory -Force -Path $h | Out-Null
    $h
}
# install.ps1 as a file in a new Windows PowerShell started in $h, with USERPROFILE=$h, the system
# folders as PATH and V2_NO_MODIFY_PATH=1, then $vars (a $null value removes one).
function Install($h, $vars = @{}) {
    $set = @{ USERPROFILE = $h; Path = $sysPath; V2_ARCHIVE = $Archive; V2_NO_MODIFY_PATH = '1'; V2_INSTALL_DIR = $null }
    foreach ($k in $vars.Keys) { $set[$k] = $vars[$k] }
    $saved = @{}
    foreach ($k in $set.Keys) { $saved[$k] = [Environment]::GetEnvironmentVariable($k); [Environment]::SetEnvironmentVariable($k, $set[$k]) }
    # A program started from PowerShell starts in PowerShell's current location.
    Push-Location -LiteralPath $h
    try {
        $ErrorActionPreference = 'Continue'
        $out = (& $powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $root 'install.ps1') 2>&1 | ForEach-Object { "$_" }) -join "`n"
        $code = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = 'Stop'
        Pop-Location
        foreach ($k in $saved.Keys) { [Environment]::SetEnvironmentVariable($k, $saved[$k]) }
    }
    @{ code = $code; out = $out }
}
function Want($got, $want, $what) { if ("$got" -cne "$want") { $script:problems += "${what}: got '$got', want '$want'" } }
function Says($r, $text) { if (-not $r.out.Contains($text)) { $script:problems += "no line with: $text" } }
function Exists($path, $want, $what) { Want ([bool](Test-Path -LiteralPath $path)) $want $what }
function Result($caseName, $r) {
    if ($script:problems.Count -eq 0) { Write-Host "PASS $caseName" }
    else {
        Write-Host "FAIL $caseName"
        $script:problems | ForEach-Object { Write-Host "    $_" }
        if ($r) { $r.out -split "`n" | ForEach-Object { Write-Host "    | $_" } }
        $script:failed = $true
    }
    $script:problems = @()
}
# A v2.exe that keeps running: it waits for a line on a stdin nobody writes to.
function StartWaiting($exe, $file) {
    $psi = New-Object Diagnostics.ProcessStartInfo
    $psi.FileName = $exe
    $psi.Arguments = '"' + $file + '"'
    $psi.UseShellExecute = $false
    $psi.RedirectStandardInput = $true
    $psi.RedirectStandardOutput = $true
    $psi.CreateNoWindow = $true
    $p = [Diagnostics.Process]::Start($psi)
    $script:procs += $p
    $p
}

New-Item -ItemType Directory -Force -Path $tmp | Out-Null
try {
    # The default: %USERPROFILE%\.v2\bin; with V2_NO_MODIFY_PATH (1, true, yes) nothing else.
    foreach ($value in '1', 'true', 'YES') {
        $h = Fresh
        $r = Install $h @{ V2_NO_MODIFY_PATH = $value }
        Want $r.code 0 'exit code'
        Exists "$h\.v2\bin\v2.exe" $true 'v2.exe installed'
        Says $r "Add $h\.v2\bin to your PATH to use v2 (V2_NO_MODIFY_PATH is set, so nothing was changed)."
        Result "V2_NO_MODIFY_PATH=$value" $r
    }

    # ~ and relative paths, for the folder and the archive.
    $h = Fresh
    New-Item -ItemType Directory -Path "$h\dl" | Out-Null
    Copy-Item -LiteralPath $Archive, "$Archive.sha256" -Destination "$h\dl"
    $r = Install $h @{ V2_INSTALL_DIR = '~\tools\v2'; V2_ARCHIVE = "~\dl\$name" }
    Want $r.code 0 'exit code'
    Exists "$h\tools\v2\v2.exe" $true '~\tools\v2\v2.exe installed'
    $r2 = Install $h @{ V2_INSTALL_DIR = 'rel\bin'; V2_ARCHIVE = "dl\$name" }
    Want $r2.code 0 'relative: exit code'
    Exists "$h\rel\bin\v2.exe" $true 'relative folder installed'
    Says $r2 "Installed v2 "
    Result '~ and relative paths' $r2

    # A folder that is a file.
    $h = Fresh
    Set-Content -LiteralPath "$h\afile" -Value ''
    $r = Install $h @{ V2_INSTALL_DIR = "$h\afile" }
    Want $r.code 1 'exit code'
    Says $r "install.ps1: V2_INSTALL_DIR names a file, not a folder: $h\afile"
    Result 'V2_INSTALL_DIR names a file' $r

    # The archive layouts: files in a v2-<target> folder (since alpha.2), or at the top.
    $h = Fresh
    Expand-Archive -LiteralPath $Archive -DestinationPath "$h\x"
    $inner = Get-ChildItem -LiteralPath "$h\x" -Recurse -Filter v2.exe | Select-Object -First 1
    Compress-Archive -LiteralPath $inner.FullName -DestinationPath "$h\flat.zip"
    $r = Install $h @{ V2_ARCHIVE = "$h\flat.zip" }
    Want $r.code 0 'exit code'
    Says $r 'there is no flat.zip.sha256 next to the archive'
    Want ((& "$h\.v2\bin\v2.exe" --version) -split ' ')[0] 'v2' 'the installed v2 runs'
    Result 'an archive with v2.exe at the top' $r

    # A damaged archive names the file and says what to do.
    $h = Fresh
    Copy-Item -LiteralPath $Archive -Destination "$h\v2.zip"
    Copy-Item -LiteralPath "$Archive.sha256" -Destination "$h\v2.zip.sha256"
    [IO.File]::AppendAllText("$h\v2.zip", 'x')
    $r = Install $h @{ V2_ARCHIVE = "$h\v2.zip" }
    Want $r.code 1 'exit code'
    Says $r "install.ps1: v2.zip is damaged (its SHA-256 doesn't match): download v2.zip and its .sha256 again"
    Exists "$h\.v2" $false 'a .v2 folder'
    Result 'a damaged archive' $r

    # A folder named v2.exe where v2.exe goes is left alone; a .tar.gz names the file Windows needs.
    $h = Fresh
    New-Item -ItemType Directory -Force -Path "$h\.v2\bin\v2.exe" | Out-Null
    Set-Content -LiteralPath "$h\.v2\bin\v2.exe\keep.txt" -Value 'x'
    $r = Install $h
    Want $r.code 1 'exit code'
    Says $r "install.ps1: $h\.v2\bin\v2.exe is a folder, so v2.exe can't go there: move it away, then run this again"
    Exists "$h\.v2\bin\v2.exe\keep.txt" $true 'the folder''s file'
    Set-Content -LiteralPath "$h\v2-x86_64-unknown-linux-gnu.tar.gz" -Value 'x'
    $r2 = Install $h @{ V2_ARCHIVE = "$h\v2-x86_64-unknown-linux-gnu.tar.gz" }
    Want $r2.code 1 'exit code for a .tar.gz'
    Says $r2 'install.ps1: v2-x86_64-unknown-linux-gnu.tar.gz is the download for Linux or macOS: Windows needs v2-x86_64-pc-windows-msvc.zip'
    Result 'a folder named v2.exe, and a .tar.gz' $r

    # A trailing backslash: the folder as PATH spells it.
    $h = Fresh
    $r = Install $h @{ V2_INSTALL_DIR = "$h\tools\" }
    Want $r.code 0 'exit code'
    Says $r "Installed v2 "
    Says $r " as $h\tools\v2.exe"
    Says $r "Add $h\tools to your PATH"
    Result 'a trailing backslash' $r

    # A running v2.exe is replaced, twice while it still runs; then the old copies are removed.
    $h = Fresh
    Set-Content -LiteralPath "$h\wait.v2" -Value 'input()' -Encoding ASCII
    $bin = "$h\.v2\bin"
    $r = Install $h
    $first = StartWaiting "$bin\v2.exe" "$h\wait.v2"
    $r = Install $h
    Want $r.code 0 'exit code, one running'
    Exists "$bin\v2.exe.old" $true 'v2.exe.old (still running)'
    $second = StartWaiting "$bin\v2.exe" "$h\wait.v2"
    $r = Install $h
    Want $r.code 0 'exit code, two running'
    Want @(Get-ChildItem -LiteralPath $bin -Filter 'v2.exe.old*').Count 2 'old copies while both run'
    Want $first.HasExited $false 'the first still runs'
    Want $second.HasExited $false 'the second still runs'
    Want ((& "$bin\v2.exe" --version) -split ' ')[0] 'v2' 'the new v2 runs'
    foreach ($p in $first, $second) { $p.Kill(); $p.WaitForExit(5000) | Out-Null }
    $r = Install $h
    Want $r.code 0 'exit code, none running'
    Want @(Get-ChildItem -LiteralPath $bin -Filter 'v2.exe.old*').Count 0 'old copies after they stop'
    Result 'a running v2.exe is replaced' $r

    # Another v2 that comes first in this window.
    $h = Fresh
    New-Item -ItemType Directory -Path "$h\old" | Out-Null
    Copy-Item -LiteralPath $inner.FullName -Destination "$h\old\v2.exe"
    $r = Install $h @{ Path = "$h\old;$sysPath" }
    Says $r "Note: a window finds another v2 now: $h\old\v2.exe"
    $r2 = Install $h @{ Path = "$h\.v2\bin;$h\old;$sysPath" }
    if ($r2.out.Contains('Note:')) { $script:problems += 'a note when this v2 comes first' }
    Result 'another v2 first on PATH' $r2

    # The user PATH, then the message for a window that isn't this PowerShell's (CI only).
    if ($ModifyPath) {
        $h = Fresh
        $r = Install $h @{ V2_NO_MODIFY_PATH = $null }
        Want $r.code 0 'exit code'
        Says $r "Added $h\.v2\bin to your user PATH"
        Says $r "Open a new window to use v2 (or, in this one, run: `$env:Path = `"$h\.v2\bin;`" + `$env:Path)"
        $user = [Environment]::GetEnvironmentVariable('Path', 'User')
        Want (($user -split ';')[0]) "$h\.v2\bin" 'the user PATH starts with'
        $r2 = Install $h @{ V2_NO_MODIFY_PATH = $null }
        Want ([Environment]::GetEnvironmentVariable('Path', 'User')) $user 'the user PATH after a second install'
        [Environment]::SetEnvironmentVariable('Path', $userPath, 'User')
        Result 'the user PATH' $r
    }
} finally {
    foreach ($p in $script:procs) { if (-not $p.HasExited) { $p.Kill(); $p.WaitForExit(5000) | Out-Null } }
    Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
}
if ([Environment]::GetEnvironmentVariable('Path', 'User') -cne $userPath) {
    Write-Host "FAIL the user PATH changed"
    $script:failed = $true
}
if ($script:failed) { exit 1 }
