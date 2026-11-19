#!/bin/sh
# Instructions each benchmark runs, at a small size, under valgrind's callgrind (Linux):
#   sh bench/callgrind.sh V2_BINARY [NAME ...]
# The count is deterministic, so a change of 1% shows; `--top NAME` instead prints the
# functions that run the most instructions for that benchmark.
set -e
here=$(cd "$(dirname "$0")" && pwd)
v2=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
shift
size() {
    case $1 in
        fib) echo 22 ;; loop | while) echo 300000 ;; calls) echo 30000 ;; closures) echo 6000 ;;
        lists | dicts) echo 10000 ;; errors) echo 100000 ;; strings) echo 6000 ;; *) echo "" ;;
    esac
}
if [ "$1" = --top ]; then
    out=$(mktemp)
    (cd "$here" && valgrind --tool=callgrind --callgrind-out-file="$out" "$v2" "$2.v2" $(size "$2") > /dev/null 2>&1)
    callgrind_annotate --inclusive=no "$out" 2> /dev/null | grep -E '^ *[0-9,]+ \(' | head -25 | cut -c1-150
    rm -f "$out"
    exit
fi
[ $# -gt 0 ] || set -- fib loop while calls closures lists dicts strings errors
for name in "$@"; do
    refs=$(cd "$here" && valgrind --tool=callgrind --callgrind-out-file=/dev/null "$v2" "$name.v2" $(size "$name") 2>&1 > /dev/null | sed -n 's/.*refs: *//p')
    printf '%-10s %16s\n' "$name" "$refs"
done
