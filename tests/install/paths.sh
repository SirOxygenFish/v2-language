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
line='export PATH="$HOME/.v2/bin:$PATH"'

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
install /bin/bash V2_INSTALL_DIR=rel/bin V2_ARCHIVE="dl/$(basename "$archive")" V2_NO_MODIFY_PATH=1
expect "$code" 0 "relative: exit code"
expect "$([ -x "$h/rel/bin/v2" ] && echo yes || echo no)" yes "relative folder installed"
says "Installed v2 " || { echo "    no 'Installed' line"; ok=no; }
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
