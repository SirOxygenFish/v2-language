# The install check on Windows (ROADMAP "Pre-publish"; INTERNALS 22.6): install V2 from the
# archive with install.ps1 into a fresh profile folder whose path has a space and a non-ASCII
# letter, then follow README.md's quickstart word for word: every "Save this as `X.v2`:" block is
# saved and run with plain `v2`, and must print the next "It prints:" block.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File tests\install\smoke.ps1 path\to\v2-....zip [-NoModifyPath]
#
# Without -NoModifyPath, install.ps1 changes the real user PATH: only for throwaway machines (CI).
param([Parameter(Mandatory = $true)][string]$Archive, [switch]$NoModifyPath)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$tmp = Join-Path ([IO.Path]::GetTempPath()) ("v2-smoke-" + [Guid]::NewGuid().ToString('N'))
$profileDir = Join-Path $tmp "home dir $([char]0xE9)"
$work = Join-Path $tmp "my project $([char]0xFC)"
New-Item -ItemType Directory -Force -Path $profileDir, $work | Out-Null
$failed = $false
try {
    # Install in Windows PowerShell, as the README says, with the profile folder moved.
    $env:USERPROFILE = $profileDir
    $env:V2_ARCHIVE = (Resolve-Path -LiteralPath $Archive).Path
    if ($NoModifyPath) { $env:V2_NO_MODIFY_PATH = '1' }
    $before = $env:Path
    & powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $root 'install.ps1')
    if ($LASTEXITCODE -ne 0) { throw "install.ps1 failed" }
    $dir = Join-Path $profileDir '.v2\bin'
    if (-not $NoModifyPath) {
        $user = [Environment]::GetEnvironmentVariable('Path', 'User')
        if (-not (($user -split ';') -contains $dir)) { throw "install.ps1 didn't put $dir on the user PATH: $user" }
    }
    # A new window gets the user PATH; here that is the folder in front of this one's PATH.
    $env:Path = "$dir;$before"

    # README's quickstart: each saved file, and what it must print.
    $lines = Get-Content -LiteralPath (Join-Path $root 'README.md') -Encoding UTF8
    $name = $null; $want = $null; $inside = $null; $buf = @(); $expected = @{}
    foreach ($line in $lines) {
        if ($inside) {
            if ($line.StartsWith('```')) {
                $text = ($buf -join "`n")
                if ($inside -eq 'code') {
                    [IO.File]::WriteAllText((Join-Path $work $name), "$text`n", (New-Object Text.UTF8Encoding($false)))
                    $want = 'out'
                } else {
                    $expected[$name] = $text
                    $want = $null
                }
                $inside = $null; $buf = @()
            } else { $buf += $line }
        } elseif ($line -match 'Save this as `([^`]*\.v2)`') {
            $name = $Matches[1]; $want = 'code'
        } elseif ($want -eq 'code' -and $line.StartsWith('```v2')) {
            $inside = 'code'
        } elseif ($want -eq 'out' -and $line.StartsWith('```text')) {
            $inside = 'out'
        }
    }
    if ($expected.Count -eq 0) { throw "README.md has no quickstart files" }
    Push-Location -LiteralPath $work
    try {
        foreach ($n in $expected.Keys) {
            # Native stderr lines become error records in Windows PowerShell: take their text.
            $ErrorActionPreference = 'Continue'
            $got = (& v2 $n 2>&1 | ForEach-Object { "$_" }) -join "`n"
            $ErrorActionPreference = 'Stop'
            if ($got -eq $expected[$n]) { Write-Host "PASS $n" }
            else { Write-Host "FAIL ${n}: printed"; $got -split "`n" | ForEach-Object { Write-Host "    $_" }; $failed = $true }
        }
    } finally { Pop-Location }
} finally {
    Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
}
if ($failed) { exit 1 }
