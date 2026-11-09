#!/bin/bash
# Recreate the QNX crate forks from crates.io + patches/*.patch.
# Usage: scripts/setup-qnx-crates.sh [dest-dir]   (default: ../qnx-crates, i.e. ~/Projects/QNX/qnx-crates)
#
# Each fork becomes a git repo: one commit with the pristine crates.io source, and the QNX
# patch applied on top as uncommitted changes, so `git diff` shows exactly the upstream patch.
# Existing fork directories are left alone.
set -euo pipefail
repo=$(cd "$(dirname "$0")/.." && pwd)
dest=${1:-$repo/../qnx-crates}
mkdir -p "$dest"
dest=$(cd "$dest" && pwd)

for patch in "$repo"/patches/*.patch; do
    crate_ver=$(basename "$patch" .patch)    # e.g. mio-1.2.4
    name=${crate_ver%-*}
    ver=${crate_ver##*-}
    dir=$dest/$crate_ver
    if [ -e "$dir" ]; then
        echo "skip  $crate_ver (already at $dir)"
        continue
    fi
    tmp=$(mktemp -d)
    curl -fsSL "https://static.crates.io/crates/$name/$crate_ver.crate" | tar -xz -C "$tmp"
    mv "$tmp/$crate_ver" "$dir"
    rmdir "$tmp"
    git -C "$dir" init -q
    git -C "$dir" add -A
    git -C "$dir" -c user.name=qnx-port -c user.email=qnx-port@localhost \
        commit -qm "Pristine $crate_ver from crates.io"
    git -C "$dir" apply "$patch"
    echo "ready $crate_ver -> $dir ($(git -C "$dir" diff --shortstat))"
done
