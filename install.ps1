# Install V2 on Windows: the prebuilt v2.exe goes to %USERPROFILE%\.v2\bin, and that folder goes
# on your user PATH. README.md "Install" explains it. In PowerShell:
#
#   irm https://raw.githubusercontent.com/SirOxygenFish/v2-language/main/install.ps1 | iex
#
# Settings, as environment variables (all optional):
#   V2_VERSION=v0.1.0-alpha.2   the release to install (default: the one this script names)
#   V2_INSTALL_DIR=C:\some\dir  where v2.exe goes (default: %USERPROFILE%\.v2\bin)
#   V2_ARCHIVE=C:\path\v2-....zip   install from an archive you downloaded instead
#   V2_NO_MODIFY_PATH=1         don't change your user PATH
#
# It runs in a script block and never calls `exit`, so `irm ... | iex` can't close your window.

& {
    $ErrorActionPreference = 'Stop'
    $ProgressPreference = 'SilentlyContinue'   # Invoke-WebRequest is very slow with its progress bar
    $repo = 'SirOxygenFish/v2-language'
    $version = if ($env:V2_VERSION) { $env:V2_VERSION } else { 'v0.1.0-alpha.2' }
    $dir = if ($env:V2_INSTALL_DIR) { $env:V2_INSTALL_DIR } else { Join-Path $env:USERPROFILE '.v2\bin' }
    $fromSource = 'build it from source instead (README.md, "Build from source")'

    function Fail($message) { throw "install.ps1: $message" }

    # 1. Which binary: x64 (Windows on Arm runs it too, emulated).
    if (-not [Environment]::Is64BitOperatingSystem) { Fail "V2 needs 64-bit Windows: $fromSource" }
    $target = 'x86_64-pc-windows-msvc'
    $name = "v2-$target.zip"
    $tmp = Join-Path ([IO.Path]::GetTempPath()) ("v2-install-" + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $tmp | Out-Null
    try {
        $zip = Join-Path $tmp $name
        # 2. Get the archive and its checksum.
        if ($env:V2_ARCHIVE) {
            if (-not (Test-Path -LiteralPath $env:V2_ARCHIVE -PathType Leaf)) { Fail "V2_ARCHIVE names no file: $env:V2_ARCHIVE" }
            Copy-Item -LiteralPath $env:V2_ARCHIVE -Destination $zip
            if (Test-Path -LiteralPath "$($env:V2_ARCHIVE).sha256") { Copy-Item -LiteralPath "$($env:V2_ARCHIVE).sha256" -Destination "$zip.sha256" }
        } else {
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
        if (Test-Path -LiteralPath "$zip.sha256") {
            $want = ((Get-Content -LiteralPath "$zip.sha256" -Raw).Trim() -split '\s+')[0].ToLowerInvariant()
            $got = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash.ToLowerInvariant()
            if ($want -ne $got) { Fail "the download is damaged (its SHA-256 doesn't match): try again" }
        }

        # 4. Unpack, and check that it runs here before it replaces anything.
        $unpacked = Join-Path $tmp 'unpacked'
        Expand-Archive -LiteralPath $zip -DestinationPath $unpacked
        $exe = Join-Path $unpacked 'v2.exe'
        if (-not (Test-Path -LiteralPath $exe)) { Fail "$name has no v2.exe in it" }
        try { $shown = & $exe --version } catch { Fail "the downloaded v2.exe doesn't run here ($($_.Exception.Message)): $fromSource" }
        if ($LASTEXITCODE -ne 0) { Fail "the downloaded v2.exe doesn't run here: $fromSource" }

        # 5. Put v2.exe in place. A running v2.exe can't be overwritten, but it can be renamed, so
        # the old one moves aside first.
        New-Item -ItemType Directory -Force -Path $dir | Out-Null
        $dest = Join-Path $dir 'v2.exe'
        if (Test-Path -LiteralPath $dest) { Move-Item -LiteralPath $dest -Destination "$dest.old" -Force }
        Copy-Item -LiteralPath $exe -Destination $dest
        Remove-Item -LiteralPath "$dest.old" -Force -ErrorAction SilentlyContinue
        Write-Host "Installed $shown as $dest"

        # 6. Put the folder on the user PATH (kept unexpanded, as Windows stores it), and on this
        # window's PATH.
        $same = { param($entry) [Environment]::ExpandEnvironmentVariables($entry).TrimEnd('\') -ieq $dir.TrimEnd('\') }
        if ($env:V2_NO_MODIFY_PATH -eq '1') {
            Write-Host "Add $dir to your PATH to use v2 (V2_NO_MODIFY_PATH is set, so nothing was changed)."
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
            if (-not (@($env:Path -split ';') | Where-Object { & $same $_ })) { $env:Path = "$dir;$env:Path" }
            Write-Host "v2 works in this window now; other open windows need to be reopened first."
        }

        # 7. Another v2.exe that would come first?
        $first = Get-Command v2 -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($first -and ($first.Source -ine $dest)) {
            Write-Host "Note: another v2 comes first on your PATH: $($first.Source) (remove it, or move $dir before it)"
        }
        Write-Host "Next: run 'v2 doctor' to see whether Python and Node.js are ready for @py and @js blocks."
    } finally {
        Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
    }
}
