#!/bin/sh
# The install check (ROADMAP "Pre-publish"; INTERNALS 22.6): install V2 from ARCHIVE with
# install.sh into a fresh home whose path has a space and a non-ASCII letter, open "a new
# terminal" (a shell that reads only the startup file install.sh wrote), then follow README.md's
# quickstart word for word: every "Save this as `X.v2`:" block is saved and run, and must print
# the next "It prints:" block. CI runs it on each release build; it also runs on clean machines.
#
#   sh tests/install/smoke.sh path/to/v2-<target>.tar.gz
set -eu

archive=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
root=$(cd "$(dirname "$0")/../.." && pwd)
tmp=$(mktemp -d 2>/dev/null || mktemp -d -t v2-smoke)
trap 'rm -rf "$tmp"' EXIT
home="$tmp/home dir é"
work="$tmp/my project ü"
mkdir -p "$home" "$work"

# Install as a bash user would, with nothing but the system folders on PATH.
env -i HOME="$home" SHELL=/bin/bash PATH=/usr/bin:/bin:/usr/sbin:/sbin V2_ARCHIVE="$archive" sh "$root/install.sh"
case "$(uname -s)" in
Darwin) rc="$home/.bash_profile" ;;
*) rc="$home/.bashrc" ;;
esac
grep -q '.v2/bin' "$rc" || { echo "FAIL: install.sh didn't put ~/.v2/bin on PATH in $rc"; exit 1; }

# README's quickstart: each saved file, and what it must print.
awk -v dir="$work" '
    /Save this as `[^`]*\.v2`/ { match($0, /`[^`]*\.v2`/); name = substr($0, RSTART + 1, RLENGTH - 2); want = "code"; next }
    want == "code" && /^```v2/ { inside = "code"; file = dir "/" name; printf "" > file; next }
    want == "out" && /^```text/ { inside = "out"; file = dir "/" name ".expected"; printf "" > file; next }
    inside != "" && /^```/ { close(file); want = (inside == "code") ? "out" : ""; inside = ""; next }
    inside != "" { print >> file }
' "$root/README.md"
ls "$work"/*.v2 >/dev/null 2>&1 || { echo "FAIL: README.md has no quickstart files"; exit 1; }

# "Open a new terminal": a bash that reads only what install.sh wrote, in the project folder.
# The caller's PATH comes after the system folders, so it can supply Python and Node.
failed=0
for f in "$work"/*.v2; do
    n=$(basename "$f")
    got=$(cd "$work" && env -i HOME="$home" PATH="/usr/bin:/bin:$PATH" bash -c '. "$HOME/'"$(basename "$rc")"'"; v2 "$1"' bash "$n" 2>&1) || true
    if [ "$got" = "$(cat "$f.expected")" ]; then
        echo "PASS $n"
    else
        echo "FAIL $n: printed"
        printf '%s\n' "$got" | sed 's/^/    /'
        failed=1
    fi
done
exit "$failed"
