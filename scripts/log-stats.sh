#!/bin/sh
# Runs scripts/log-stats.awk over every log given: plain, rotated (.1, .2, ...) and compressed
# (.gz, .bz2, .xz, .zst) files, or directories (every file in them with "log" in its name).
# Order doesn't matter; the report goes to stdout, the list of files read first.
#
#   scripts/log-stats.sh /var/log/nginx/f0ckm.access.log*            # current, .1, .2.gz, ...
#   scripts/log-stats.sh /var/log/nginx > stats.txt                  # a whole directory
#   docker compose logs --no-color f0ckm | scripts/log-stats.sh -    # stdin as well
#   TOP=50 scripts/log-stats.sh /var/log/nginx/f0ckm.access.log*     # longer top lists
#
# Without arguments: /var/log/nginx/*access.log*
set -eu

here=$(cd "$(dirname "$0")" && pwd)
awkfile="$here/log-stats.awk"
[ -r "$awkfile" ] || { echo "log-stats.sh: $awkfile not found" >&2; exit 1; }

[ $# -gt 0 ] || set -- /var/log/nginx/*access.log*

list=$(mktemp)
trap 'rm -f "$list"' EXIT INT TERM

add() {
    if [ -d "$1" ]; then
        find "$1" -maxdepth 1 -type f -name '*log*' | sort >> "$list"
    elif [ "$1" = - ] || [ -f "$1" ]; then
        printf '%s\n' "$1" >> "$list"
    else
        echo "log-stats.sh: skipping $1 (not found)" >&2
    fi
}
for a in "$@"; do add "$a"; done
[ -s "$list" ] || { echo "log-stats.sh: no log files" >&2; exit 1; }

# Each file decompressed by its extension; a file that can't be read is reported and skipped
dump() {
    case "$1" in
        -) cat ;;
        *.gz) gzip -dc -- "$1" ;;
        *.bz2) bzip2 -dc -- "$1" ;;
        *.xz) xz -dc -- "$1" ;;
        *.zst) zstd -dcq -- "$1" ;;
        *) cat -- "$1" ;;
    esac
}

{
    echo "== files =="
    while IFS= read -r f; do
        if [ "$f" = - ]; then echo "  (stdin)"; continue; fi
        printf '  %s  %s\n' "$(du -h -- "$f" 2>/dev/null | cut -f1)" "$(basename -- "$f")"
    done < "$list"
    echo
    # the list on fd 3, so "-" (cat) reads the real stdin
    while IFS= read -r f <&3; do
        dump "$f" || echo "log-stats.sh: could not read $f" >&2
    done 3< "$list" | LC_ALL=C awk -v TOP="${TOP:-25}" -v SAMPLES="${SAMPLES:-5}" -f "$awkfile"
}
