#!/bin/bash
# Build a QNX 8.0 boot image with ts-forward installed and started at boot.
#
# Usage:
#   cargo build --release && scripts/build-image.sh                                     # QEMU VM (x86_64)
#   cargo build --release --target aarch64-unknown-qnx && IMAGE_TYPE=rasppi5 scripts/build-image.sh   # Pi 5
#   cargo build --release --target aarch64-unknown-qnx && IMAGE_TYPE=rasppi  scripts/build-image.sh   # Pi 4
#
# QEMU (and other mkqnximage types): a full disk image from mkqnximage, in build/image/<type>/output/.
#   /system/bin/ts-forward              the release binary, stripped
#   /data/var/ts-forward/keys.json      its tailnet identity (created on first start, kept across reboots)
#   /data/var/log/ts-forward.log        its log, including the login URL on first start
#
# Raspberry Pi 4/5: the documented BSP flow (QNX BSP User's Guide). The board's BSP, installed in the SDP
# with QNX Software Center (com.qnx.qnx800.bsp.hw.raspberrypi_bcm2711_rpi4 / ..._bcm2712_rpi5), is
# extracted to build/image/<type>/bsp, ts-forward is added to its buildfile, and its `make` builds the IFS.
# The result is build/image/<type>/bootfs/: the files to copy onto the boot (FAT) partition of an SD card
# prepared with Raspberry Pi Imager (Raspberry Pi OS), which already holds the Pi's firmware.
#   /usr/bin/ts-forward                 the release binary, stripped
#   bootfs/ts-forward/keys.json         its tailnet identity, on the SD card (kept across reboots)
#   bootfs/ts-forward/ts-forward.log    its log, readable by putting the card in any computer
#   bootfs/ts-forward/authkey           optional auth key (TS_AUTH_KEY, or copy one there yourself)
#   bootfs/ts-forward/args              ts-forward's arguments (TS_FORWARD_ARGS); edit it on the card to change them
# The startup script and buildfile additions are in image/raspberrypi/.
#
# In both, ts-forward runs in a loop at boot, so it's restarted if it exits.
#
# Env:
#   IMAGE_TYPE        qemu (default), rasppi5 (Pi 5), rasppi (Pi 4), or another mkqnximage --type (e.g. vmware)
#   TS_FORWARD_ARGS   forwards to run (default: "-p ssh")
#   TS_AUTH_KEY       optional auth key so the device joins without the login URL. QEMU: stored in the image's
#                     data partition (root-only). Raspberry Pi: written to bootfs/ts-forward/authkey. Anyone
#                     with the image or card can read it: prefer a single-use key.
#   IMAGE_HOSTNAME    QNX hostname, which ts-forward also uses as its tailnet name (default: qnx-tailscale).
#                     Raspberry Pi: written to bootfs/ts-forward/hostname; edit it on the card to change it.
#   SSH_IDENT         QEMU only: public key installed for root login (default: ~/.ssh/id_ed25519.pub)
set -euo pipefail

repo=$(cd "$(dirname "$0")/.." && pwd)
type=${IMAGE_TYPE:-qemu}
img=$repo/build/image/$type
args=${TS_FORWARD_ARGS:--p ssh}

case "$type" in
    rasppi)  board=rpi4 ;;
    rasppi5) board=rpi5 ;;
    *)       board= ;;
esac
if [ -n "$board" ]; then
    rust_target=aarch64-unknown-qnx; strip=ntoaarch64-strip
else
    rust_target=x86_64-pc-qnx;       strip=ntox86_64-strip
fi
bin=$repo/target/$rust_target/release/ts-forward

command -v "$strip" >/dev/null || { echo "error: $strip not found; run: source ~/qnx800/qnxsdp-env.sh" >&2; exit 1; }
if [ ! -f "$bin" ]; then
    hint="cargo build --release"; [ -n "$board" ] && hint="$hint --target $rust_target"
    echo "error: $bin not found; run: $hint" >&2; exit 1
fi
case "$args" in *"'"*|*'"'*|*'{'*|*'}'*)
    echo "error: TS_FORWARD_ARGS can't contain quotes or braces" >&2; exit 1 ;;
esac

mkdir -p "$img"
cd "$img"

# ===================================================================================================
# Raspberry Pi: BSP buildfile + make
# ===================================================================================================
if [ -n "$board" ]; then
    command -v unzip >/dev/null || { echo "error: unzip not found (needed to extract the BSP)" >&2; exit 1; }
    sdp=${QNX_TARGET%/target/qnx}
    zip=$(ls -1 "$sdp"/bsp/BSP_raspberrypi-*-"$board"_*.zip 2>/dev/null | sort | tail -n1 || true)
    [ -n "$zip" ] || { echo "error: no Raspberry Pi $board BSP in $sdp/bsp; install it with QNX Software Center" >&2; exit 1; }

    # A fresh copy of the BSP each run, so the buildfile edit below never accumulates.
    echo "== extracting $(basename "$zip")"
    rm -rf bsp bootfs
    mkdir -p bsp bootfs/ts-forward
    unzip -q "$zip" -d bsp

    "$strip" -o bsp/ts-forward "$bin"

    # Add ts-forward and its startup script (image/raspberrypi/) to the BSP's buildfile. mkifs expands
    # the variables in host paths.
    cat "$repo/image/raspberrypi/ts-forward.build" >> "bsp/images/$board.build"
    export TS_FORWARD_BIN=$img/bsp/ts-forward
    export TS_FORWARD_STARTUP=$repo/image/raspberrypi/board_startup.sh

    echo "== building the BSP (make)"
    rm -f "bsp/images/ifs-$board.bin"
    make -C bsp > make.log 2>&1 || { tail -30 make.log >&2; echo "error: BSP build failed; full log: $img/make.log" >&2; exit 1; }
    [ -f "bsp/images/ifs-$board.bin" ] || { echo "error: make produced no bsp/images/ifs-$board.bin; see $img/make.log" >&2; exit 1; }

    # The files for the SD card's boot partition (from the BSP User's Guide).
    cp "bsp/images/ifs-$board.bin" bootfs/
    if [ "$board" = rpi5 ]; then
        printf '%s\n' arm_64bit=1 enable_uart=1 force_turbo=1 "kernel=ifs-$board.bin" > bootfs/config.txt
    else
        printf '%s\n' arm_64bit=1 enable_uart=1 force_turbo=1 gpu_mem=16 max_framebuffers=2 "kernel=ifs-$board.bin" > bootfs/config.txt
    fi
    printf '%s\n' "$args" > bootfs/ts-forward/args
    printf '%s\n' "${IMAGE_HOSTNAME:-qnx-tailscale}" > bootfs/ts-forward/hostname
    if [ -n "${TS_AUTH_KEY:-}" ]; then
        (umask 077; printf '%s\n' "$TS_AUTH_KEY" > bootfs/ts-forward/authkey)
    fi

    echo
    echo "== Built $img/bootfs/"
    (cd bootfs && find . -type f | sort | sed 's|^\./|     |')
    echo "   ts-forward args at boot: $args"
    [ -f bootfs/ts-forward/authkey ] && echo "   auth key: included (bootfs/ts-forward/authkey)" \
                                     || echo "   auth key: none (the login URL will be in ts-forward/ts-forward.log on the card)"
    echo "   Write Raspberry Pi OS to the SD card with Raspberry Pi Imager, then copy everything in bootfs/"
    echo "   onto its boot partition (replacing config.txt), e.g.:  cp -r $img/bootfs/. /media/\$USER/bootfs/"
    exit 0
fi

# ===================================================================================================
# QEMU and other mkqnximage types
# ===================================================================================================
command -v mkqnximage >/dev/null || { echo "error: mkqnximage not found; run: source ~/qnx800/qnxsdp-env.sh" >&2; exit 1; }
hostname=${IMAGE_HOSTNAME:-qnx-tailscale}
ident=${SSH_IDENT:-$HOME/.ssh/id_ed25519.pub}
mkdir -p local/snippets local/files

"$strip" -o local/files/ts-forward "$bin"

if [ -n "${TS_AUTH_KEY:-}" ]; then
    (umask 077; printf '%s\n' "$TS_AUTH_KEY" > local/files/authkey)
else
    rm -f local/files/authkey
fi

# Snippets. These files are owned by this script: rewritten on every run.
cat > local/snippets/system_files.custom <<EOF
# Generated by tailscale-QNX scripts/build-image.sh
bin/sshd-auth=usr/libexec/sshd-auth
[perms=555] bin/ts-forward=$img/local/files/ts-forward
EOF

cat > local/snippets/ifs_files.custom <<'EOF'
# Generated by tailscale-QNX scripts/build-image.sh
[type=link] /usr/libexec/sshd-auth=/system/bin/sshd-auth
EOF

{
    echo "# Generated by tailscale-QNX scripts/build-image.sh"
    [ -f local/files/authkey ] && echo "[uid=0 gid=0 perms=600] var/ts-forward/authkey=$img/local/files/authkey"
    true
} > local/snippets/data_files.custom

cat > local/snippets/post_start.custom <<EOF
# Generated by tailscale-QNX scripts/build-image.sh: start ts-forward, restarting it if it exits.
mkdir -p /data/var/ts-forward /data/var/log
(
    # The DHCP client works in the background: wait (up to 2 minutes) for a default route.
    i=0
    while ! route -n get default >/dev/null 2>&1 && [ \$i -lt 120 ]; do sleep 1; i=\$((i + 1)); done
    echo "network: default route after \$i s"
    cd /data/var/ts-forward
    if [ -r authkey ]; then TS_AUTH_KEY=\$(cat authkey); export TS_AUTH_KEY; fi
    while true; do
        echo "ts-forward starting: \$(date)"
        /system/bin/ts-forward -c /data/var/ts-forward/keys.json $args
        echo "ts-forward exited with status \$?; restarting in 10 s"
        sleep 10
    done
) >> /data/var/log/ts-forward.log 2>&1 &
EOF

opts=(--type="$type" --arch=x86_64 --hostname="$hostname" --noprompt --build)
if [ -f "$ident" ]; then
    opts+=(--ssh-ident="$ident")
else
    opts+=(--ssh-ident=none)
    echo "note: $ident not found, so the image has no root ssh login (set SSH_IDENT)" >&2
fi

echo "== mkqnximage ${opts[*]}  (in $img)"
mkqnximage "${opts[@]}"

disk=$(ls -1t "$img"/output/disk-* 2>/dev/null | grep -v '\.vmdk$' | head -n1 || true)
echo
echo "== Image built: ${disk:-$img/output}"
echo "   ts-forward args at boot: $args"
[ -f local/files/authkey ] && echo "   auth key: included (joins the tailnet on first boot)" \
                           || echo "   auth key: none (the login URL will be in /data/var/log/ts-forward.log)"
[ "$type" = qemu ] && echo "   Boot it with:  cd $img && mkqnximage --run=-h"
true
