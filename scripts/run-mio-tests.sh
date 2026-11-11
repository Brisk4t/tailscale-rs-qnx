#!/usr/bin/env bash
# Build mio's test suite for QNX and run each test binary on the QEMU target.
#
# Usage: scripts/run-mio-tests.sh [extra libtest args...]
#   e.g. scripts/run-mio-tests.sh -Z unstable-options --report-time
#
# Expects a mio git checkout at $MIO_DIR (default ~/Projects/mio) with patches/mio-<ver>.patch
# applied and a QNX .cargo/config.toml (see diagnostics.md). Output:
#   $MIO_DIR/build.log           cargo build output
#   $MIO_DIR/raw/<binary>.txt    full output of each test binary
#   $MIO_DIR/mio-qnx-tests.log   filtered results
set -uo pipefail

here=$(cd "$(dirname "$0")" && pwd)
MIO_DIR=${MIO_DIR:-$HOME/Projects/mio}
TIMEOUT=${TIMEOUT:-120}

source ~/qnx800/qnxsdp-env.sh > /dev/null
cd "$MIO_DIR" || exit 1

RUN=$here/qnx-run.sh
TARGET_DIR=${QNX_TARGET_DIR:-$HOME/qnxprojects/targets/qemu-qnx800-x86_64}
IP=${QNX_HOST_IP:-$("$here/qnx-target-ip.sh" "$TARGET_DIR")} || exit 1
SSH=(ssh -n -o BatchMode=yes -o UserKnownHostsFile="$TARGET_DIR/local/ssh-ident" "root@$IP")

echo "### building"
cargo +nightly-2026-11-09 test --release --features os-poll,os-ext,net --no-run 2>&1 \
    | tee build.log \
    | grep -E '^error|Finished'
mapfile -t bins < <(sed -n 's/.*Executable .* (\(.*\))$/\1/p' build.log)
[ ${#bins[@]} -gt 0 ] || { echo "no test binaries built, see build.log"; exit 1; }

mkdir -p raw
{
    for bin in "${bins[@]}"; do
        name=$(basename "$bin")
        echo "=== $name"
        # stdin from /dev/null: under `timeout` ssh runs in a background process group, and
        # reading the terminal would stop it (SIGTTIN) and hang the script.
        timeout -k 5 "$TIMEOUT" "$RUN" "$bin" --test-threads=1 "$@" < /dev/null > "raw/$name.txt" 2>&1
        rc=$?
        grep -E '^test |test result|panicked|error' "raw/$name.txt"
        if [ $rc -eq 124 ] || [ $rc -eq 137 ]; then
            # libtest prints "test <name> ... " and only ends the line when the test finishes,
            # so the unfinished last line names the test that hung.
            echo "TIMEOUT after ${TIMEOUT}s in: $(tail -c 200 "raw/$name.txt" | tr -d '\n' | sed 's/.*\(test [^ ]* \.\.\. \)$/\1/')"
        fi
        # Kill anything left running on the target, then free the space.
        "${SSH[@]}" "slay -f $name 2>/dev/null; rm -f /data/home/root/bin/$name; sync"
    done
} | tee mio-qnx-tests.log

echo "### summary"
grep -E '^===|test result|TIMEOUT' mio-qnx-tests.log
