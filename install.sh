#!/bin/sh
# Install V2 on Linux or macOS: the prebuilt binary goes to ~/.v2/bin, and that folder goes on
# your PATH. README.md "Install" explains it.
#
#   curl -fsSL https://raw.githubusercontent.com/SirOxygenFish/v2-language/main/install.sh | sh
#
# Settings, as environment variables (all optional). With the one-liner they go after the pipe,
# where sh sees them: `curl -fsSL .../install.sh | V2_INSTALL_DIR=~/.local/bin sh`.
#   V2_VERSION=v0.1.0-alpha.2   the release to install (default: the one this script names)
#   V2_INSTALL_DIR=/some/dir    where v2 goes (default: ~/.v2/bin)
#   V2_ARCHIVE=/path/v2-....tar.gz   install from an archive you downloaded instead (a .sha256
#                               file next to it is checked)
#   V2_NO_MODIFY_PATH=1         don't add the folder to your shell's startup files
set -eu

repo="SirOxygenFish/v2-language"
marker="# V2 (added by install.sh)"
status=0

say() { printf '%s\n' "$*"; }
fail() {
    printf 'install.sh: %s\n' "$*" >&2
    exit 1
}
from_source='build it from source instead (README.md, "Build from source")'
# A path as a shell would read it: a leading ~ (kept by quotes) is the home folder, and a
# relative one starts at the current folder.
full_path() {
    case "$1" in
    "~") printf '%s\n' "$HOME" ;;
    "~/"*) printf '%s\n' "$HOME/${1#"~/"}" ;;
    /*) printf '%s\n' "$1" ;;
    *) printf '%s\n' "$PWD/$1" ;;
    esac
}
yes_value() {
    case "$1" in
    1 | [Tt][Rr][Uu][Ee] | [Yy][Ee][Ss]) return 0 ;;
    *) return 1 ;;
    esac
}

version="${V2_VERSION:-v0.1.0-alpha.2}"
case "$version" in
v*) ;;
*) version="v$version" ;;
esac
dir=$(full_path "${V2_INSTALL_DIR:-$HOME/.v2/bin}")
[ ! -e "$dir" ] || [ -d "$dir" ] || fail "V2_INSTALL_DIR names a file, not a folder: $dir"

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
        fail "this needs curl or wget to download V2: install one (sudo apt update && sudo apt install curl), or download the archive yourself and set V2_ARCHIVE"
    fi
}

# 2. Get the archive and its checksum.
if [ -n "${V2_ARCHIVE:-}" ]; then
    archive=$(full_path "$V2_ARCHIVE")
    shown_archive=$(basename "$archive")
    [ -f "$archive" ] || fail "V2_ARCHIVE names no file: $archive"
    case "$shown_archive" in
    *.zip) fail "$shown_archive is the download for Windows: this computer needs $name" ;;
    esac
    cp "$archive" "$tmp/$name"
    if [ -f "$archive.sha256" ]; then
        cp "$archive.sha256" "$tmp/$name.sha256"
    else
        say "Note: there is no $shown_archive.sha256 next to the archive, so it isn't checked"
    fi
    again="download $shown_archive and its .sha256 again"
else
    shown_archive=$name
    url="https://github.com/$repo/releases/download/$version/$name"
    say "Downloading $url"
    download "$url" "$tmp/$name" || fail "could not download $url (is $version a release? see https://github.com/$repo/releases)"
    download "$url.sha256" "$tmp/$name.sha256" || fail "could not download $url.sha256"
    again="try again"
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
        fail "$shown_archive is damaged (its SHA-256 doesn't match): $again"
    fi
fi

# 4. Unpack (the files are in a folder named like the archive), and check that v2 runs here
# before it replaces anything.
mkdir "$tmp/unpacked"
tar -xzf "$tmp/$name" -C "$tmp/unpacked" || fail "could not unpack $shown_archive: $again"
if [ -f "$tmp/unpacked/v2-$target/v2" ]; then
    bin="$tmp/unpacked/v2-$target/v2"
elif [ -f "$tmp/unpacked/v2" ]; then
    bin="$tmp/unpacked/v2"
else
    # A release for another computer is in a folder named for it.
    for other in "$tmp/unpacked"/v2-*; do
        [ ! -d "$other" ] || fail "$shown_archive is the download for another computer (${other##*/v2-}): this one needs $name"
    done
    fail "$shown_archive has no v2 in it"
fi
chmod 755 "$bin"
if ! "$bin" --version >"$tmp/version" 2>"$tmp/error"; then
    cat "$tmp/error" >&2
    need=$(grep -o 'GLIBC_[0-9.]*' "$tmp/error" | head -n 1 | cut -d _ -f 2)
    if [ -n "$need" ]; then
        fail "this Linux is too old for the prebuilt V2, which needs glibc $need or newer: $from_source"
    fi
    if grep -qi 'exec format\|cannot execute' "$tmp/error"; then
        fail "that v2 is for another kind of computer: this one needs $name"
    fi
    fail "the downloaded v2 doesn't run here (see above): $from_source"
fi
installed=$(cut -d ' ' -f 1-2 <"$tmp/version")

# 5. Put v2 in place (a rename, so a running v2 is not touched), in the folder as PATH spells it:
# absolute, without `.`, `..` or a trailing `/`.
mkdir -p "$dir" || fail "could not create $dir"
dir=$(CDPATH='' cd -- "$dir" && pwd) || fail "could not open $dir"
[ ! -d "$dir/v2" ] || fail "$dir/v2 is a folder, so v2 can't go there: move it away, then run this again"
cp "$bin" "$dir/v2.new"
chmod 755 "$dir/v2.new"
mv -f "$dir/v2.new" "$dir/v2"
say "Installed $installed as $dir/v2"

# 6. Put the folder on PATH for new terminals: in each startup file the shell reads, the marker
# and the setting, once per folder.
case ":$PATH:" in
*":$dir:"*) on_path=yes ;;
*) on_path=no ;;
esac
case "$dir" in
"$HOME"/*) shown="\$HOME/${dir#"$HOME"/}" ;;
*) shown="$dir" ;;
esac
# add_line FILE LINE: 0 when it adds the line, 1 when FILE has that exact line already, 2 when
# FILE can't be written.
add_line() {
    if [ -f "$1" ] && grep -qxF -- "$2" "$1"; then
        return 1
    fi
    mkdir -p "$(dirname "$1")" 2>/dev/null || return 2
    {
        # A file whose last line has no line break gets one first.
        if [ -s "$1" ] && [ -n "$(tail -c 1 "$1")" ]; then printf '\n'; fi
        printf '%s\n%s\n' "$marker" "$2"
    } 2>/dev/null >>"$1" || return 2
}
if yes_value "${V2_NO_MODIFY_PATH:-}"; then
    if [ "$on_path" = no ]; then
        say "Add $dir to your PATH to use v2 (V2_NO_MODIFY_PATH is set, so nothing was changed)."
    fi
else
    line="export PATH=\"$shown:\$PATH\""
    # The startup files, as the positional parameters (a home folder's path can have spaces).
    case "$(basename "${SHELL:-sh}")" in
    # Every zsh reads .zshenv: new terminals, `zsh -lc`, ssh commands.
    zsh) set -- "${ZDOTDIR:-$HOME}/.zshenv" ;;
    # A new terminal reads .bashrc, a login shell .bash_profile or else .profile (macOS
    # terminals are login shells).
    bash)
        if [ "$os" = Darwin ] || [ -f "$HOME/.bash_profile" ]; then
            set -- "$HOME/.bashrc" "$HOME/.bash_profile"
        else
            set -- "$HOME/.bashrc" "$HOME/.profile"
        fi
        ;;
    fish)
        set -- "$HOME/.config/fish/conf.d/v2.fish"
        # Global, not universal: deleting the file undoes it.
        line="contains \"$dir\" \$PATH; or set -gx PATH \"$dir\" \$PATH"
        ;;
    *) set -- "$HOME/.profile" ;;
    esac
    added=""
    unwritable=""
    for f in "$@"; do
        case "$f" in
        "$HOME"/*) short="~/${f#"$HOME"/}" ;;
        *) short=$f ;;
        esac
        if add_line "$f" "$line"; then
            added="${added:+$added and }$short"
        elif [ $? -eq 2 ]; then
            unwritable="${unwritable:+$unwritable and }$short"
        fi
    done
    if [ -n "$unwritable" ]; then
        printf 'install.sh: could not write %s, so new terminals may not find v2: add this line to it yourself:\n    %s\n' "$unwritable" "$line" >&2
        status=1
    fi
    if [ -n "$added" ]; then
        say "Added $dir to PATH in $added"
    elif [ "$on_path" = no ] && [ -z "$unwritable" ]; then
        say "Your startup files already put $dir on PATH"
    fi
    if [ "$on_path" = no ]; then
        if [ -n "$added" ] || [ -z "$unwritable" ]; then
            say "Open a new terminal to use v2 (or, in this one, run: export PATH=\"$dir:\$PATH\")"
        else
            say "To use v2 in this terminal, run: export PATH=\"$dir:\$PATH\""
        fi
    fi
fi

# 7. Another v2 that would come first: on PATH before this folder, or, when this folder isn't
# on PATH yet and nothing was added, the one a terminal finds now.
other=$(command -v v2 2>/dev/null || true)
if [ -n "$other" ] && [ "$other" != "$dir/v2" ]; then
    if [ "$on_path" = yes ]; then
        say "Note: another v2 comes first on your PATH: $other (remove it, or put $dir before it)"
    elif yes_value "${V2_NO_MODIFY_PATH:-}"; then
        say "Note: a terminal finds another v2 now: $other"
    fi
fi
say "Next: run 'v2 doctor' to see whether Python and Node.js are ready for @py and @js blocks."
exit "$status"
