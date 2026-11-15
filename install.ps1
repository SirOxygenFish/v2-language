# Install V2 on Windows: the prebuilt v2.exe goes to %USERPROFILE%\.v2\bin, and that folder goes
# on your user PATH. README.md "Install" explains it. In PowerShell:
#
#   irm https://raw.githubusercontent.com/SirOxygenFish/v2-language/main/install.ps1 | iex
#
# Settings, as environment variables set first in the same window (all optional), for example
# $env:V2_INSTALL_DIR = "C:\Tools\v2":
#   V2_VERSION=v0.1.0-alpha.2   the release to install (default: the one this script names)
#   V2_INSTALL_DIR=C:\some\dir  where v2.exe goes (default: %USERPROFILE%\.v2\bin)
#   V2_ARCHIVE=C:\path\v2-....zip   install from an archive you downloaded instead (a .sha256
#                               file next to it is checked)
#   V2_NO_MODIFY_PATH=1         don't change your user PATH
#
# It runs in a script block and never calls `exit` when run through `iex`, so it can't close your
# window; a problem is one red line.

& {
    $ErrorActionPreference = 'Stop'
    $ProgressPreference = 'SilentlyContinue'   # Invoke-WebRequest is very slow with its progress bar
    $repo = 'SirOxygenFish/v2-language'
    $version = if ($env:V2_VERSION) { $env:V2_VERSION } else { 'v0.1.0-alpha.2' }
    if (-not $version.StartsWith('v')) { $version = "v$version" }
    $fromSource = 'build it from source instead (README.md, "Build from source")'
    $noPath = $env:V2_NO_MODIFY_PATH -match '^\s*(1|true|yes)\s*$'

    # A failure: thrown with this mark, so the handler below prints it as one line.
    function Fail($message) { throw [System.Exception]::new("install.ps1: $message") }
    # A path as PowerShell reads it: a leading ~ is your user folder, and a relative one starts at
    # the current folder.
    function FullPath($path) {
        $path = $path.Trim().Trim('"')
        if ($path -eq '~') { $path = $env:USERPROFILE }
        elseif ($path -match '^~[\\/]') { $path = Join-Path $env:USERPROFILE $path.Substring(2) }
        $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($path)
    }
    # The first v2.exe a window with this PATH runs, or $null.
    function FirstV2($pathText) {
        foreach ($entry in ($pathText -split ';')) {
            if (-not $entry) { continue }
            try {
                $candidate = Join-Path ([Environment]::ExpandEnvironmentVariables($entry.Trim('"'))) 'v2.exe'
                if (Test-Path -LiteralPath $candidate -PathType Leaf) { return $candidate }
            } catch {}
        }
        $null
    }

    $tmp = Join-Path ([IO.Path]::GetTempPath()) ("v2-install-" + [Guid]::NewGuid().ToString('N'))
    try {
        $dir = if ($env:V2_INSTALL_DIR) { FullPath $env:V2_INSTALL_DIR } else { Join-Path $env:USERPROFILE '.v2\bin' }
        # As PATH spells a folder: without a trailing separator (C:\ keeps its own).
        if ($dir.Length -gt 3) { $dir = $dir.TrimEnd('\', '/') }
        if (Test-Path -LiteralPath $dir -PathType Leaf) { Fail "V2_INSTALL_DIR names a file, not a folder: $dir" }

        # 1. Which binary: x64 (Windows on Arm runs it too, emulated).
        if (-not [Environment]::Is64BitOperatingSystem) { Fail "V2 needs 64-bit Windows: $fromSource" }
        $target = 'x86_64-pc-windows-msvc'
        $name = "v2-$target.zip"
        New-Item -ItemType Directory -Path $tmp | Out-Null
        $zip = Join-Path $tmp $name

        # 2. Get the archive and its checksum.
        if ($env:V2_ARCHIVE) {
            $archive = FullPath $env:V2_ARCHIVE
            $shownArchive = Split-Path -Leaf $archive
            $again = "download $shownArchive and its .sha256 again"
            if (-not (Test-Path -LiteralPath $archive -PathType Leaf)) { Fail "V2_ARCHIVE names no file: $archive" }
            if ($shownArchive -match '\.(tar\.gz|tgz)$') { Fail "$shownArchive is the download for Linux or macOS: Windows needs $name" }
            Copy-Item -LiteralPath $archive -Destination $zip
            if (Test-Path -LiteralPath "$archive.sha256" -PathType Leaf) {
                Copy-Item -LiteralPath "$archive.sha256" -Destination "$zip.sha256"
            } else {
                Write-Host "Note: there is no $shownArchive.sha256 next to the archive, so it isn't checked"
            }
        } else {
            $shownArchive = $name
            $again = 'try again'
            # Windows PowerShell 5.1 on older systems doesn't offer TLS 1.2 by itself.
            [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
            $url = "https://github.com/$repo/releases/download/$version/$name"
            Write-Host "Downloading $url"
            try {
                Invoke-WebRequest -UseBasicParsing -Uri $url -OutFile $zip
                Invoke-WebRequest -UseBasicParsing -Uri "$url.sha256" -OutFile "$zip.sha256"
            } catch {
                Fail "could not download $url ($($_.Exception.Message)); is $version a release? See https://github.com/$repo/releases"
            }
        }

        # 3. Check it.
        $checked = $false
        if (Test-Path -LiteralPath "$zip.sha256") {
            $want = ((Get-Content -LiteralPath "$zip.sha256" -Raw).Trim() -split '\s+')[0].ToLowerInvariant()
            $got = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash.ToLowerInvariant()
            if ($want -ne $got) { Fail "$shownArchive is damaged (its SHA-256 doesn't match): $again" }
            $checked = $true
        }

        # 4. Unpack (the files are in a folder named like the archive), and check that it runs here
        # before it replaces anything.
        $unpacked = Join-Path $tmp 'unpacked'
        try { Expand-Archive -LiteralPath $zip -DestinationPath $unpacked } catch { Fail "could not unpack $shownArchive (it isn't a zip file, or is damaged): $again" }
        $exe = Join-Path $unpacked "v2-$target\v2.exe"
        if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) { $exe = Join-Path $unpacked 'v2.exe' }
        if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) { Fail "$shownArchive has no v2.exe in it" }
        # Unchecked, a file that doesn't run may just be damaged.
        $next = if ($checked) { $fromSource } else { "if the file may be damaged, download it again with its .sha256, so it gets checked; else $fromSource" }
        # Windows's own reason, without PowerShell's "At …install.ps1:108 char:24" position.
        try { $shown = & $exe --version } catch {
            $why = if ($_.Exception.InnerException) { $_.Exception.InnerException.Message } else { $_.Exception.Message }
            $why = ($why -replace '(?s)\s*At [^\r\n]*:\d+ char:\d+.*$', '').Trim().TrimEnd('.')
            Fail "the v2.exe in $shownArchive doesn't run here ($why): $next"
        }
        if ($LASTEXITCODE -ne 0) { Fail "the v2.exe in $shownArchive doesn't run here: $next" }
        $shown = (($shown -split ' ')[0..1]) -join ' '

        # 5. Put v2.exe in place. A running v2.exe can't be overwritten or deleted, but it can be
        # renamed, so the old one moves aside first, to a name no other running v2.exe has.
        New-Item -ItemType Directory -Force -Path $dir | Out-Null
        $dest = Join-Path $dir 'v2.exe'
        if (Test-Path -LiteralPath $dest -PathType Container) { Fail "$dest is a folder, so v2.exe can't go there: move it away, then run this again" }
        Get-ChildItem -LiteralPath $dir -Filter 'v2.exe.old*' -File -Force -ErrorAction SilentlyContinue | Remove-Item -Force -ErrorAction SilentlyContinue
        $old = "$dest.old"
        if (Test-Path -LiteralPath $old) { $old = "$dest.old-" + [Guid]::NewGuid().ToString('N').Substring(0, 8) }
        $moved = $false
        if (Test-Path -LiteralPath $dest) {
            try { Move-Item -LiteralPath $dest -Destination $old -Force } catch { Fail "could not replace $dest ($($_.Exception.Message))" }
            $moved = $true
        }
        try {
            Copy-Item -LiteralPath $exe -Destination $dest
        } catch {
            if ($moved) { Move-Item -LiteralPath $old -Destination $dest -Force -ErrorAction SilentlyContinue }
            Fail "could not write $dest ($($_.Exception.Message))"
        }
        if ($moved) { Remove-Item -LiteralPath $old -Force -ErrorAction SilentlyContinue }
        Write-Host "Installed $shown as $dest"
        if ($moved -and (Test-Path -LiteralPath $old)) {
            Write-Host "Note: the old v2.exe is still running, so $(Split-Path -Leaf $old) stays until it ends: the next install removes it, or delete it then"
        }

        # 6. Put the folder on the user PATH (kept unexpanded, as Windows stores it), and on this
        # window's PATH.
        $same = { param($entry) [Environment]::ExpandEnvironmentVariables($entry.Trim('"')).TrimEnd('\') -ieq $dir.TrimEnd('\') }
        $onPath = [bool](@($env:Path -split ';') | Where-Object { & $same $_ })
        if ($noPath) {
            if (-not $onPath) { Write-Host "Add $dir to your PATH to use v2 (V2_NO_MODIFY_PATH is set, so nothing was changed)." }
        } else {
            $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)
            try {
                $raw = [string]$key.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
                $entries = @($raw -split ';' | Where-Object { $_ -ne '' })
                if (-not ($entries | Where-Object { & $same $_ })) {
                    $key.SetValue('Path', ((@($dir) + $entries) -join ';'), [Microsoft.Win32.RegistryValueKind]::ExpandString)
                    # Setting a user variable tells Windows and new terminals that the environment changed.
                    [Environment]::SetEnvironmentVariable('V2_INSTALL_DONE', '1', 'User')
                    [Environment]::SetEnvironmentVariable('V2_INSTALL_DONE', $null, 'User')
                    Write-Host "Added $dir to your user PATH"
                }
            } finally {
                $key.Close()
            }
            if (-not $onPath) {
                $env:Path = "$dir;$env:Path"
                if ($PSCommandPath) {
                    # Run as a file, this may be another PowerShell than the window's.
                    Write-Host "Open a new window to use v2 (or, in this one, run: `$env:Path = `"$dir;`" + `$env:Path)"
                } else {
                    Write-Host "v2 works in this window now; other open windows need to be reopened first."
                }
            }
        }

        # 7. Another v2.exe that would come first: in this window when nothing was changed, else
        # in new windows (the system PATH comes before the user PATH there).
        $first = if ($noPath) { FirstV2 $env:Path } else {
            FirstV2 ([Environment]::GetEnvironmentVariable('Path', 'Machine') + ';' + [Environment]::GetEnvironmentVariable('Path', 'User'))
        }
        if ($first -and ($first -ine $dest)) {
            if ($noPath) {
                Write-Host "Note: a window finds another v2 now: $first (put $dir before it in PATH, or remove it)"
            } else {
                Write-Host "Note: new windows find another v2 first: $first (remove it, or move $dir before it in PATH)"
            }
        }
        Write-Host "Next: run 'v2 doctor' to see whether Python and Node.js are ready for @py and @js blocks."
    } catch {
        $text = $_.Exception.Message
        if (-not $text.StartsWith('install.ps1: ')) { $text = "install.ps1: $text" }
        Write-Host $text -ForegroundColor Red
        # Run as a file (`powershell -File install.ps1`), the exit code says it failed; through
        # `iex` there is no file, and `exit` would close the window.
        if ($PSCommandPath) { exit 1 }
    } finally {
        Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
    }
}
