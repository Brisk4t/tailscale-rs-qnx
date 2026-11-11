#!/bin/bash
# Add `target_os = "qnx"` (QNX SDP 8.0) next to every `target_os = "nto"` cfg in the given files.
#
#   - `cfg(target_os = "nto")` / `not(...)` / `cfg_attr(target_os = "nto", ...)` become
#     `...(any(target_os = "nto", target_os = "qnx")...`
#   - `target_os = "nto",` inside an existing any(...)/all(...) list gains a sibling `target_os = "qnx",`
#   - `all(target_os = "nto", ...)` (version-specific branches) is left alone and reported: decide by hand.
#
# Idempotent: running it again on a patched file changes nothing.
# Usage: scripts/add-qnx-cfg.sh FILE...      (e.g. $(grep -rl 'target_os = "nto"' src))
# Needs GNU sed (for -z) and awk.
set -euo pipefail

[ $# -gt 0 ] || { echo "usage: $0 FILE..." >&2; exit 2; }

for path in "$@"; do
    tmp=$(mktemp)
    manual=$(mktemp)

    # 1. Single-predicate wrappers, over the whole file (-z), since `cfg(` and the predicate may be on
    #    different lines.
    sed -z -E 's/\b(cfg|not|cfg_attr)\(\s*target_os = "nto"\s*([,)])/\1(any(target_os = "nto", target_os = "qnx")\2/g' \
        "$path" |
    # 2. Line by line: report all(...) sites, add a sibling line to one-predicate-per-line lists,
    #    and add a sibling inline everywhere else.
    awk -v manual="$manual" -v path="$path" '
        { line[NR] = $0 }
        END {
            for (i = 1; i <= NR; i++) {
                l = line[i]
                if (index(l, "target_os = \"nto\"") && !index(l, "target_os = \"qnx\"")) {
                    if (l ~ /all\([[:space:]]*target_os = "nto"/) {
                        t = l; gsub(/^[[:space:]]+|[[:space:]]+$/, "", t)
                        printf "  MANUAL %s:%d: %s\n", path, i, t > manual
                    } else if (l ~ /^[[:space:]]*target_os = "nto",?[[:space:]]*$/) {
                        # One predicate per line: add a `target_os = "qnx",` line after it, unless the
                        # next line already is one (from an earlier run).
                        if (line[i + 1] ~ /^[[:space:]]*target_os = "qnx",?[[:space:]]*$/) { print l; continue }
                        match(l, /^[[:space:]]*/); indent = substr(l, 1, RLENGTH)
                        if (l !~ /,[[:space:]]*$/) l = l ","
                        print l
                        l = indent "target_os = \"qnx\","
                    } else {
                        gsub(/target_os = "nto",/, "target_os = \"nto\", target_os = \"qnx\",", l)
                        gsub(/target_os = "nto"\)/, "target_os = \"nto\", target_os = \"qnx\")", l)
                    }
                }
                print l
            }
        }
    ' > "$tmp"

    # awk always ends with a newline; keep the file's own ending.
    if [ -s "$path" ] && [ -n "$(tail -c1 "$path")" ]; then
        truncate -s -1 "$tmp"
    fi

    if cmp -s "$tmp" "$path"; then
        rm "$tmp"
    else
        cat "$tmp" > "$path"   # keeps the file's permissions
        rm "$tmp"
        echo "patched $path"
    fi
    cat "$manual"
    rm "$manual"
done
