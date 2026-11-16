#!/bin/sh
# install.sh's choices (INTERNALS 22.6), each in a fresh home with only the system folders on
# PATH: the startup files each shell gets, once per folder, and that a new shell then finds v2;
# V2_NO_MODIFY_PATH; ~ and relative paths; a folder that is a file; both archive layouts; a
# damaged archive; another v2 that comes first.
#
#   sh tests/install/paths.sh path/to/v2-<target>.tar.gz
set -eu

archive=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
root=$(cd "$(dirname "$0")/../.." && pwd)
tmp=$(mktemp -d 2>/dev/null || mktemp -d -t v2-paths)
trap 'rm -rf "$tmp"' EXIT
sys=/usr/bin:/bin:/usr/sbin:/sbin
os=$(uname -s)
failed=0
n=0
line='case ":$PATH:" in *":$HOME/.v2/bin:"*) ;; *) export PATH="$HOME/.v2/bin:$PATH" ;; esac'

fresh() {
    n=$((n + 1))
    h="$tmp/home $n é"
    mkdir -p "$h"
}
# install SHELL [VAR=value...]: install.sh in $h, run from $h; its output in $out, exit code in $code.
install() {
    shell=$1
    shift
    code=0
    out=$(cd "$h" && env -i HOME="$h" SHELL="$shell" PATH="$sys" V2_ARCHIVE="$archive" "$@" sh "$root/install.sh" 2>&1) || code=$?
}
count() { # FILE TEXT: the lines of FILE that contain TEXT
    if [ -f "$1" ]; then grep -cF -- "$2" "$1" || true; else echo 0; fi
}
says() { case "$out" in *"$1"*) return 0 ;; *) return 1 ;; esac; }
result() { # NAME, after a test that set ok
    if [ "$ok" = yes ]; then
        echo "PASS $1"
    else
        echo "FAIL $1"
        printf '%s\n' "$out" | sed 's/^/    /'
        failed=1
    fi
}
expect() { [ "$1" = "$2" ] || { echo "    $3: got '$1', want '$2'"; ok=no; }; }
# finds SHELL FLAG [SETUP]: that shell, started with FLAG from HOME's startup files, finds this
# v2 (after SETUP). With a UTF-8 locale, as in a terminal: fish 3.3 misreads a non-ASCII home
# folder in the C locale.
finds() {
    found=$(env -i HOME="$h" PATH="$sys" LANG=C.UTF-8 "$1" "$2" "${3:-}command -v v2" 2>/dev/null || true)
    expect "$found" "$h/.v2/bin/v2" "$1 $2 finds"
}

# bash: .bashrc for new terminals, and the login file: .profile, or .bash_profile when it exists
# (on macOS, where terminals are login shells, always .bash_profile). Once per folder.
login=.profile
[ "$os" = Darwin ] && login=.bash_profile
fresh
install /bin/bash
ok=yes
expect "$code" 0 "exit code"
expect "$(count "$h/.bashrc" "$line")" 1 ".bashrc lines"
expect "$(count "$h/$login" "$line")" 1 "$login lines"
expect "$(count "$h/.bashrc" '# V2 (added by install.sh)')" 1 ".bashrc marker"
finds bash -lc
finds bash -c '. "$HOME/.bashrc"; '
install /bin/bash
expect "$(count "$h/.bashrc" "$line")" 1 ".bashrc lines after a second install"
says "already put $h/.v2/bin on PATH" || { echo "    no 'already' line"; ok=no; }
install /bin/bash V2_INSTALL_DIR="$h/other"
expect "$(count "$h/.bashrc" 'export PATH="$HOME/other:$PATH"')" 1 "a second folder's line"
expect "$(count "$h/$login" 'export PATH="$HOME/other:$PATH"')" 1 "a second folder's login line"
result "bash: .bashrc and $login, once per folder"

if [ "$os" != Darwin ]; then
    fresh
    : >"$h/.bash_profile"
    install /bin/bash
    ok=yes
    expect "$(count "$h/.bash_profile" "$line")" 1 ".bash_profile lines"
    expect "$([ -e "$h/.profile" ] && echo yes || echo no)" no ".profile written"
    finds bash -lc
    result "bash: an existing .bash_profile is the login file"
fi

# A startup file whose last line has no line break keeps that line whole.
fresh
printf 'alias ll="ls -l"' >"$h/.bashrc"
install /bin/bash
ok=yes
expect "$(sed -n 1p "$h/.bashrc")" 'alias ll="ls -l"' "first line"
expect "$(sed -n 2p "$h/.bashrc")" '# V2 (added by install.sh)' "second line"
result "a last line without a line break"

# zsh: .zshenv (every zsh reads it), in ZDOTDIR when that is set; nothing else.
fresh
install /usr/bin/zsh
ok=yes
expect "$(count "$h/.zshenv" "$line")" 1 ".zshenv lines"
expect "$(ls -A "$h")" ".v2
.zshenv" "files written"
if command -v zsh >/dev/null 2>&1; then finds zsh -c; fi
install /usr/bin/zsh ZDOTDIR="$h/zdot"
expect "$(count "$h/zdot/.zshenv" "$line")" 1 "ZDOTDIR's .zshenv lines"
result "zsh: .zshenv"

# fish: a conf.d file with a global setting (deleting the file undoes it).
fresh
install /usr/bin/fish
ok=yes
expect "$(cat "$h/.config/fish/conf.d/v2.fish" 2>/dev/null | sed -n 2p)" "contains \"$h/.v2/bin\" \$PATH; or set -gx PATH \"$h/.v2/bin\" \$PATH" "v2.fish"
if command -v fish >/dev/null 2>&1; then finds fish -c; fi
result "fish: conf.d/v2.fish"

# Other shells, or no SHELL at all: .profile.
fresh
install /bin/dash
ok=yes
expect "$(count "$h/.profile" "$line")" 1 ".profile lines"
finds sh -lc
fresh
code=0
out=$(cd "$h" && env -i HOME="$h" PATH="$sys" V2_ARCHIVE="$archive" sh "$root/install.sh" 2>&1) || code=$?
expect "$code" 0 "exit code without SHELL"
# bash as sh (macOS) sets SHELL to the user's login shell when it is unset, and install.sh follows it.
if [ -z "$(env -i HOME="$h" PATH="$sys" sh -c 'printf %s "${SHELL:-}"')" ]; then
    expect "$(count "$h/.profile" "$line")" 1 ".profile lines without SHELL"
fi
result "other shells: .profile"

# V2_NO_MODIFY_PATH: 1, true, yes (any case) change nothing; other values are no.
for value in 1 true YES; do
    fresh
    install /bin/bash V2_NO_MODIFY_PATH="$value"
    ok=yes
    expect "$(ls -A "$h")" .v2 "files written"
    says "Add $h/.v2/bin to your PATH to use v2" || { echo "    no 'Add' line"; ok=no; }
    result "V2_NO_MODIFY_PATH=$value"
done
fresh
install /bin/bash V2_NO_MODIFY_PATH=0
ok=yes
expect "$(count "$h/.bashrc" "$line")" 1 ".bashrc lines"
result "V2_NO_MODIFY_PATH=0 changes PATH"

# ~ (kept by quotes) and relative paths, for the folder and the archive.
fresh
mkdir "$h/dl"
cp "$archive" "$archive.sha256" "$h/dl/"
install /bin/bash V2_INSTALL_DIR='~/tools/v2' V2_ARCHIVE="~/dl/$(basename "$archive")"
ok=yes
expect "$code" 0 "exit code"
expect "$([ -x "$h/tools/v2/v2" ] && echo yes || echo no)" yes "~/tools/v2/v2 installed"
expect "$(count "$h/.bashrc" 'export PATH="$HOME/tools/v2:$PATH"')" 1 ".bashrc lines"
install /bin/bash V2_INSTALL_DIR=rel/bin V2_ARCHIVE="dl/$(basename "$archive")" V2_NO_MODIFY_PATH=1 V2_VERSION=v9.9.9
expect "$code" 0 "relative: exit code"
expect "$([ -x "$h/rel/bin/v2" ] && echo yes || echo no)" yes "relative folder installed"
says "Installed v2 " || { echo "    no 'Installed' line"; ok=no; }
says "Note: V2_VERSION is ignored: V2_ARCHIVE names the archive to install" || { echo "    no V2_VERSION note"; ok=no; }
result "~ and relative paths"

# A folder that is a file.
fresh
: >"$h/afile"
install /bin/bash V2_INSTALL_DIR="$h/afile"
ok=yes
expect "$code" 1 "exit code"
says "V2_INSTALL_DIR names a file, not a folder: $h/afile" || { echo "    no message"; ok=no; }
result "V2_INSTALL_DIR names a file"

# The archive layouts: files in a v2-<target> folder (since alpha.2), or at the top.
fresh
mkdir "$h/x" "$h/flat"
tar -xzf "$archive" -C "$h/x"
inner=$(find "$h/x" -name v2 -type f)
cp "$inner" "$h/flat/v2"
(cd "$h/flat" && tar -czf "$h/flat.tar.gz" v2)
install /bin/bash V2_ARCHIVE="$h/flat.tar.gz" V2_NO_MODIFY_PATH=1
ok=yes
expect "$code" 0 "exit code"
says "there is no flat.tar.gz.sha256 next to the archive" || { echo "    no sha256 note"; ok=no; }
expect "$("$h/.v2/bin/v2" --version | cut -d ' ' -f 1)" v2 "the installed v2 runs"
result "an archive with v2 at the top"

# A damaged archive names the file and says what to do.
fresh
cp "$archive" "$h/v2.tar.gz"
cp "$archive.sha256" "$h/v2.tar.gz.sha256"
printf 'x' >>"$h/v2.tar.gz"
install /bin/bash V2_ARCHIVE="$h/v2.tar.gz"
ok=yes
expect "$code" 1 "exit code"
says "v2.tar.gz is damaged (its SHA-256 doesn't match): download v2.tar.gz and its .sha256 again" || { echo "    no message"; ok=no; }
expect "$(ls -A "$h")" "v2.tar.gz
v2.tar.gz.sha256" "files written"
result "a damaged archive"

# An archive whose v2 doesn't run, checked against its .sha256: the message names that archive.
fresh
mkdir -p "$h/pack"
printf '#!/bin/sh\necho "cannot start" >&2\nexit 1\n' >"$h/pack/v2"
(cd "$h/pack" && tar -czf ../broken.tar.gz v2)
if command -v sha256sum >/dev/null 2>&1; then sum=$(sha256sum "$h/broken.tar.gz"); else sum=$(shasum -a 256 "$h/broken.tar.gz"); fi
printf '%s  broken.tar.gz\n' "${sum%% *}" >"$h/broken.tar.gz.sha256"
install /bin/bash V2_ARCHIVE="$h/broken.tar.gz"
ok=yes
expect "$code" 1 "exit code"
says "the v2 in broken.tar.gz doesn't run here (see above): " || { echo "    no message"; ok=no; }
result "an archive whose v2 doesn't run"

# A temp folder that can't run programs (a server's /tmp mounted noexec) doesn't matter: the
# check runs the copy in the install folder. An install folder that can't is named. Root only,
# on Linux: it mounts a tmpfs.
if [ "$(id -u)" = 0 ] && [ "$(uname -s)" = Linux ] && mkdir -p "$tmp/noexec" && mount -t tmpfs -o noexec tmpfs "$tmp/noexec" 2>/dev/null; then
    fresh
    install /bin/bash TMPDIR="$tmp/noexec"
    ok=yes
    expect "$code" 0 "exit code"
    expect "$("$h/.v2/bin/v2" --version | cut -d ' ' -f 1)" v2 "the installed v2 runs"
    result "a temp folder mounted noexec"
    fresh
    install /bin/bash V2_INSTALL_DIR="$tmp/noexec/bin"
    ok=yes
    expect "$code" 1 "exit code"
    says "$tmp/noexec/bin doesn't let programs run (its file system is mounted noexec): install into a folder that does, with V2_INSTALL_DIR" || { echo "    no message"; ok=no; }
    # Only that line: not the shell's own "v2.new: Permission denied" before it.
    ! says "v2.new" || { echo "    the shell's error is shown too"; ok=no; }
    expect "$(ls -A "$tmp/noexec")" "" "files left in the folder"
    result "an install folder mounted noexec"
    umount "$tmp/noexec"
else
    echo "SKIP noexec mounts (they need root on Linux)"
fi

# A startup file that can't be written is named, with the line to add; the others are written.
fresh
mkdir "$h/$login"
install /bin/bash
ok=yes
expect "$code" 1 "exit code"
says "install.sh: could not write ~/$login, so new terminals may not find v2: add this line to it yourself:" || { echo "    no message"; ok=no; }
says "already put" && { echo "    an 'already' line"; ok=no; }
expect "$(count "$h/.bashrc" "$line")" 1 ".bashrc lines"
result "a startup file that can't be written"

# Only the exact line counts: one commented out is not there.
fresh
printf '# %s\n' "$line" >"$h/.bashrc"
install /bin/bash
ok=yes
expect "$(grep -cxF "$line" "$h/.bashrc")" 1 "exact .bashrc lines"
result "a commented-out line"

# The folder as PATH spells it: a trailing slash or a . changes nothing.
fresh
mkdir "$h/tools"
install /bin/bash V2_INSTALL_DIR="$h/tools/"
ok=yes
says " as $h/tools/v2" || { echo "    not installed as $h/tools/v2"; ok=no; }
install /bin/bash V2_INSTALL_DIR="$h/tools"
expect "$(count "$h/.bashrc" 'export PATH="$HOME/tools:$PATH"')" 1 ".bashrc lines"
expect "$(grep -c 'tools/:' "$h/.bashrc")" 0 "lines with tools/"
install /bin/bash V2_INSTALL_DIR="$h/tools/" PATH="$h/tools:$sys" V2_NO_MODIFY_PATH=1
says "Note:" && { echo "    a note about this v2"; ok=no; }
says "Add " && { echo "    an 'Add' line though the folder is on PATH"; ok=no; }
mkdir "$h/dot"
(cd "$h/dot" && env -i HOME="$h" SHELL=/bin/bash PATH="$sys" V2_ARCHIVE="$archive" V2_INSTALL_DIR=. V2_NO_MODIFY_PATH=1 sh "$root/install.sh" >/dev/null 2>&1) || ok=no
expect "$([ -x "$h/dot/v2" ] && echo yes || echo no)" yes "V2_INSTALL_DIR=. installs there"
result "a trailing slash and ."

# A folder named v2 where v2 goes is left alone.
fresh
mkdir -p "$h/.v2/bin/v2"
install /bin/bash
ok=yes
expect "$code" 1 "exit code"
says "$h/.v2/bin/v2 is a folder, so v2 can't go there: move it away, then run this again" || { echo "    no message"; ok=no; }
expect "$(ls -A "$h/.v2/bin/v2")" "" "the folder's files"
result "a folder named v2"

# An archive for another computer, or for Windows, names the file this computer needs.
case "$os" in
Darwin) mine=v2-aarch64-apple-darwin.tar.gz other=x86_64-unknown-linux-gnu ;;
*) mine=v2-x86_64-unknown-linux-gnu.tar.gz other=aarch64-apple-darwin ;;
esac
fresh
mkdir -p "$h/stage/v2-$other"
printf '#!/bin/sh\n' >"$h/stage/v2-$other/v2"
tar -czf "$h/v2-$other.tar.gz" -C "$h/stage" "v2-$other"
install /bin/bash V2_ARCHIVE="$h/v2-$other.tar.gz"
ok=yes
expect "$code" 1 "exit code"
says "v2-$other.tar.gz is the download for another computer ($other): this one needs $mine" || { echo "    no message"; ok=no; }
cp "$archive" "$h/v2-x86_64-pc-windows-msvc.zip"
install /bin/bash V2_ARCHIVE="$h/v2-x86_64-pc-windows-msvc.zip"
expect "$code" 1 "exit code for a .zip"
says "v2-x86_64-pc-windows-msvc.zip is the download for Windows: this computer needs $mine" || { echo "    no message for a .zip"; ok=no; }
result "an archive for another computer"

# Folder names with characters the shell treats specially still work in new shells.
fresh
odd="$h/my \"v2\" \$x \`t\` dir"
mkdir -p "$odd"
install /bin/bash V2_INSTALL_DIR="$odd"
ok=yes
expect "$code" 0 "exit code"
found=$(env -i HOME="$h" PATH="$sys" bash -lc 'command -v v2' 2>&1 || true)
expect "$found" "$odd/v2" "bash -lc finds"
if command -v fish >/dev/null 2>&1; then
    install /usr/bin/fish V2_INSTALL_DIR="$odd"
    found=$(env -i HOME="$h" PATH="$sys" LANG=C.UTF-8 fish -c 'command -v v2' 2>&1 || true)
    expect "$found" "$odd/v2" "fish -c finds"
fi
result "a folder named with \", \$ and \`"

# The command shown for this terminal works when pasted, escaped as the startup line is: for a
# folder in the home folder, one outside it, and when no startup file could be written.
# now TEXT: the command after TEXT in install.sh's output.
now() { printf '%s\n' "$out" | sed -n "s/^$1\(.*\)/\1/p" | sed 's/)$//'; }
fresh
odd="$h/we\$ird \`dir\`"
mkdir -p "$odd"
install /bin/bash V2_INSTALL_DIR="$odd"
ok=yes
expect "$code" 0 "exit code"
cmd=$(now 'Open a new terminal to use v2 (or, in this one, run: ')
found=$(env -i HOME="$h" PATH="$sys" sh -c "$cmd; command -v v2" 2>&1 || true)
expect "$found" "$odd/v2" "sh finds it after '$cmd'"
outside="$tmp/out\$side \`dir\` $n"
install /bin/bash V2_INSTALL_DIR="$outside"
cmd=$(now 'Open a new terminal to use v2 (or, in this one, run: ')
found=$(env -i HOME="$h" PATH="$sys" sh -c "$cmd; command -v v2" 2>&1 || true)
expect "$found" "$outside/v2" "sh finds it outside the home folder after '$cmd'"
if command -v fish >/dev/null 2>&1; then
    install /usr/bin/fish V2_INSTALL_DIR="$odd"
    cmd=$(now 'Open a new terminal to use v2 (or, in this one, run: ')
    found=$(env -i HOME="$h" PATH="$sys" LANG=C.UTF-8 fish -c "$cmd; command -v v2" 2>&1 || true)
    expect "$found" "$odd/v2" "fish finds it after '$cmd'"
fi
fresh
mkdir "$h/.bashrc" "$h/$login"
install /bin/bash V2_INSTALL_DIR="$h/we\$ird \`dir\`"
expect "$code" 1 "exit code when no startup file can be written"
cmd=$(now 'To use v2 in this terminal, run: ')
found=$(env -i HOME="$h" PATH="$sys" sh -c "$cmd; command -v v2" 2>&1 || true)
expect "$found" "$h/we\$ird \`dir\`/v2" "sh finds it after '$cmd' (no startup file written)"
result "the command for this terminal, for a folder named with \$ and \`"

# A system folder already on PATH: no startup file changes.
fresh
sysdir="$tmp/system bin $n"
install /bin/bash PATH="$sysdir:$sys" V2_INSTALL_DIR="$sysdir"
ok=yes
says "$sysdir is already on your PATH, so no startup file was changed" || { echo "    no message"; ok=no; }
expect "$(ls -A "$h")" "" "files written"
result "a system folder already on PATH"

# The home folder's ~/.v2/bin on this terminal's PATH (an export, or lines an uninstall deleted):
# the startup files still get the setting, and a shell that reads two files has it once.
fresh
install /bin/bash PATH="$h/.v2/bin:$sys"
ok=yes
says "Added $h/.v2/bin to PATH in ~/.bashrc and ~/." || { echo "    no 'Added' line"; ok=no; }
says "New terminals will find v2 too" || { echo "    no 'new terminals' line"; ok=no; }
found=$(env -i HOME="$h" PATH="$sys" bash -lc 'command -v v2' 2>&1 || true)
expect "$found" "$h/.v2/bin/v2" "bash -lc finds"
install /bin/bash
twice=$(env -i HOME="$h" PATH="$sys" bash -lc '. "$HOME/.bashrc"; printf "%s" "$PATH"' | tr ':' '\n' | grep -cxF "$h/.v2/bin")
expect "$twice" 1 "times on PATH after .profile and .bashrc"
result "the home folder's v2 on PATH already, and only once"

# fish's startup folder follows XDG_CONFIG_HOME.
fresh
install /usr/bin/fish XDG_CONFIG_HOME="$h/xdg"
ok=yes
expect "$([ -f "$h/xdg/fish/conf.d/v2.fish" ] && echo yes || echo no)" yes "xdg/fish/conf.d/v2.fish"
result "fish with XDG_CONFIG_HOME"

# An update into another folder says where the v2 this terminal runs is; latest isn't a release.
fresh
mkdir "$h/old"
printf '#!/bin/sh\necho old\n' >"$h/old/v2"
chmod 755 "$h/old/v2"
install /bin/bash PATH="$h/old:$sys"
ok=yes
says "Note: this terminal runs another v2, $h/old/v2; new terminals run this one. To update that one instead, run this again with V2_INSTALL_DIR=\"\$HOME/old\"" || { echo "    no note"; ok=no; }
# The folder it names works when pasted, also with $ and a backtick, in the home folder or not.
for odd in "$h/o\$ld \`x\`" "$tmp/o\$ld \`x\` $n"; do
    mkdir -p "$odd"
    cp "$h/old/v2" "$odd/v2"
    install /bin/bash PATH="$odd:$sys"
    set_dir=$(printf '%s\n' "$out" | sed -n 's/.*run this again with \(V2_INSTALL_DIR=.*\)$/\1/p')
    got=$(env -i HOME="$h" PATH="$sys" sh -c "$set_dir; printf '%s' \"\$V2_INSTALL_DIR\"" 2>&1 || true)
    expect "$got" "$odd" "V2_INSTALL_DIR after '$set_dir'"
done
install /bin/bash V2_VERSION=latest
expect "$code" 1 "exit code for latest"
says "V2_VERSION=latest isn't a release name: leave V2_VERSION unset" || { echo "    no message for latest"; ok=no; }
result "another v2 elsewhere, and V2_VERSION=latest"

# Another v2 that comes first.
fresh
mkdir "$h/old"
printf '#!/bin/sh\necho old\n' >"$h/old/v2"
chmod 755 "$h/old/v2"
install /bin/bash PATH="$h/old:$h/.v2/bin:$sys"
ok=yes
says "Note: another v2 comes first on your PATH: $h/old/v2" || { echo "    no note"; ok=no; }
install /bin/bash PATH="$h/old:$sys" V2_NO_MODIFY_PATH=1
says "Note: a terminal finds another v2 now: $h/old/v2" || { echo "    no note with V2_NO_MODIFY_PATH"; ok=no; }
install /bin/bash PATH="$h/.v2/bin:$h/old:$sys"
says "Note:" && { echo "    a note when this v2 comes first"; ok=no; }
result "another v2 first on PATH"

exit "$failed"
