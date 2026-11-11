#!/bin/bash
# Copy a binary to the QNX target and run it there over ssh, forwarding args and exit code.
# Works as a cargo runner: `runner = "scripts/qnx-run.sh"` (cargo calls it with <binary> [args...]).
#
# Env:
#   QNX_HOST_IP     target address (default: `mkqnximage --getip` in QNX_TARGET_DIR)
#   QNX_USER        ssh user (default: root)
#   QNX_TARGET_DIR  mkqnximage target dir (default: ~/qnxprojects/targets/qemu-qnx800-x86_64)
#   QNX_REMOTE_DIR  where binaries go on the target (default: /data/home/$QNX_USER/bin)
#
# Needs the QNX SDP environment (source ~/qnx800/qnxsdp-env.sh) for mkqnximage and ntox86_64-strip.
set -euo pipefail
bin=$1; shift
TARGET_DIR=${QNX_TARGET_DIR:-$HOME/qnxprojects/targets/qemu-qnx800-x86_64}
# The VM's IP changes on every boot; mkqnximage finds it from the VM's MAC address.
ip=${QNX_HOST_IP:-$(cd "$TARGET_DIR" && mkqnximage --getip 2>/dev/null | tail -n1)} || true
if [ -z "$ip" ]; then
    echo "qnx-run: can't find the target's IP (is the VM in $TARGET_DIR running? set QNX_HOST_IP to skip the lookup)" >&2
    exit 1
fi
user=${QNX_USER:-root}
remote_dir=${QNX_REMOTE_DIR:-/data/home/$user/bin}
ssh_opts=(-o BatchMode=yes -o ConnectTimeout=5 -o UserKnownHostsFile="$TARGET_DIR/local/ssh-ident")

# Strip a copy so debug builds fit on the small data partition.
tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
name=$(basename "$bin")
ntox86_64-strip -o "$tmp/$name" "$bin" 2>/dev/null || cp "$bin" "$tmp/$name"

ssh "${ssh_opts[@]}" "$user@$ip" "mkdir -p $remote_dir"
scp -q "${ssh_opts[@]}" "$tmp/$name" "$user@$ip:$remote_dir/$name"
args=; [ $# -gt 0 ] && args=$(printf ' %q' "$@")
exec ssh "${ssh_opts[@]}" "$user@$ip" "cd $remote_dir && ./$name$args"
