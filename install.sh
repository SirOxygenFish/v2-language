#!/bin/sh
# Install V2 on Linux or macOS: the prebuilt binary goes to ~/.v2/bin, and that folder goes on
# your PATH. README.md "Install" explains it.
#
#   curl -fsSL https://raw.githubusercontent.com/SirOxygenFish/v2-language/main/install.sh | sh
#
# Settings, as environment variables (all optional):
#   V2_VERSION=v0.1.0-alpha.2   the release to install (default: the one this script names)
#   V2_INSTALL_DIR=/some/dir    where v2 goes (default: ~/.v2/bin)
#   V2_ARCHIVE=/path/v2-....tar.gz   install from an archive you downloaded instead
#   V2_NO_MODIFY_PATH=1         don't add the folder to your shell's startup file
set -eu

repo="SirOxygenFish/v2-language"
version="${V2_VERSION:-v0.1.0-alpha.2}"
dir="${V2_INSTALL_DIR:-$HOME/.v2/bin}"

say() { printf '%s\n' "$*"; }
fail() {
    printf 'install.sh: %s\n' "$*" >&2
    exit 1
}
from_source='build it from source instead (README.md, "Build from source")'

# 1. Which binary this computer needs.
os=$(uname -s)
arch=$(uname -m)
case "$os/$arch" in
Linux/x86_64 | Linux/amd64) target=x86_64-unknown-linux-gnu ;;
Darwin/arm64 | Darwin/aarch64) target=aarch64-apple-darwin ;;
Darwin/x86_64) fail "there is no prebuilt V2 for Intel Macs yet: $from_source" ;;
*) fail "there is no prebuilt V2 for $os on $arch yet: $from_source" ;;
esac
name="v2-$target.tar.gz"

tmp=$(mktemp -d 2>/dev/null || mktemp -d -t v2-install)
trap 'rm -rf "$tmp"' EXIT

download() { # URL FILE
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL --retry 2 -o "$2" "$1"
    elif command -v wget >/dev/null 2>&1; then
        wget -q -O "$2" "$1"
    else
        fail "this needs curl or wget to download V2: install one (sudo apt install curl), or download the archive yourself and set V2_ARCHIVE"
    fi
}

# 2. Get the archive and its checksum.
if [ -n "${V2_ARCHIVE:-}" ]; then
    [ -f "$V2_ARCHIVE" ] || fail "V2_ARCHIVE names no file: $V2_ARCHIVE"
    cp "$V2_ARCHIVE" "$tmp/$name"
    if [ -f "$V2_ARCHIVE.sha256" ]; then cp "$V2_ARCHIVE.sha256" "$tmp/$name.sha256"; fi
else
    url="https://github.com/$repo/releases/download/$version/$name"
    say "Downloading $url"
    download "$url" "$tmp/$name" || fail "could not download $url (is $version a release? see https://github.com/$repo/releases)"
    download "$url.sha256" "$tmp/$name.sha256" || fail "could not download $url.sha256"
fi

# 3. Check it.
if [ -f "$tmp/$name.sha256" ]; then
    want=$(cut -d ' ' -f 1 <"$tmp/$name.sha256")
    if command -v sha256sum >/dev/null 2>&1; then
        got=$(sha256sum "$tmp/$name" | cut -d ' ' -f 1)
    elif command -v shasum >/dev/null 2>&1; then
        got=$(shasum -a 256 "$tmp/$name" | cut -d ' ' -f 1)
    else
        got=""
        say "Note: no sha256sum or shasum here, so the download isn't checked"
    fi
    if [ -n "$got" ] && [ "$got" != "$want" ]; then
        fail "the download is damaged (its SHA-256 doesn't match): try again"
    fi
fi

# 4. Unpack, and check that it runs here before it replaces anything.
tar -xzf "$tmp/$name" -C "$tmp" || fail "could not unpack $name"
[ -f "$tmp/v2" ] || fail "$name has no v2 in it"
chmod 755 "$tmp/v2"
if ! "$tmp/v2" --version >"$tmp/version" 2>"$tmp/error"; then
    cat "$tmp/error" >&2
    if grep -q GLIBC "$tmp/error"; then
        fail "this Linux is too old for the prebuilt V2, which needs glibc 2.35 or newer (Ubuntu 22.04, Debian 12, Fedora 36 or later): $from_source"
    fi
    fail "the downloaded v2 doesn't run here (see above): $from_source"
fi

# 5. Put v2 in place (a rename, so a running v2 is not touched).
mkdir -p "$dir" || fail "could not create $dir"
cp "$tmp/v2" "$dir/v2.new"
chmod 755 "$dir/v2.new"
mv -f "$dir/v2.new" "$dir/v2"
say "Installed $(cat "$tmp/version") as $dir/v2"

# 6. Put the folder on PATH for new terminals.
case ":$PATH:" in
*":$dir:"*) on_path=yes ;;
*) on_path=no ;;
esac
if [ "$on_path" = no ]; then
    case "$dir" in
    "$HOME"/*) shown="\$HOME/${dir#"$HOME"/}" ;;
    *) shown="$dir" ;;
    esac
    if [ "${V2_NO_MODIFY_PATH:-}" = 1 ]; then
        say "Add $dir to your PATH to use v2 (V2_NO_MODIFY_PATH is set, so nothing was changed)."
    else
        shell_name=$(basename "${SHELL:-sh}")
        line="export PATH=\"$shown:\$PATH\""
        case "$shell_name" in
        zsh) rc="${ZDOTDIR:-$HOME}/.zshrc" ;;
        bash) if [ "$os" = Darwin ]; then rc="$HOME/.bash_profile"; else rc="$HOME/.bashrc"; fi ;;
        fish)
            rc="$HOME/.config/fish/conf.d/v2.fish"
            line="fish_add_path \"$dir\""
            ;;
        *) rc="$HOME/.profile" ;;
        esac
        if [ -f "$rc" ] && grep -qF "$shown" "$rc"; then
            say "$rc already puts $dir on PATH"
        else
            mkdir -p "$(dirname "$rc")"
            printf '\n# V2 (added by install.sh)\n%s\n' "$line" >>"$rc"
            say "Added $dir to PATH in $rc"
        fi
        say "Open a new terminal to use v2 (or, in this one, run: export PATH=\"$dir:\$PATH\")"
    fi
fi

# 7. Another v2 that would come first?
other=$(command -v v2 2>/dev/null || true)
if [ -n "$other" ] && [ "$other" != "$dir/v2" ] && [ "$on_path" = yes ]; then
    say "Note: another v2 comes first on your PATH: $other (remove it, or put $dir before it)"
fi
say "Next: run 'v2 doctor' to see whether Python and Node.js are ready for @py and @js blocks."
