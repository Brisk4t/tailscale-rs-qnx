#!/bin/bash
# Print the current DHCP address of the QEMU QNX target (virbr0 / libvirt dnsmasq).
# Usage: qnx-target-ip.sh [target-dir]
set -euo pipefail
TARGET_DIR=${1:-${QNX_TARGET_DIR:-$HOME/qnxprojects/targets/qemu-qnx800-x86_64}}
mac=$(sed -n "s/^OPT_MACADDR='\(.*\)'/\1/p" "$TARGET_DIR/local/options")
status=/var/lib/libvirt/dnsmasq/virbr0.status
# The lease file lists entries oldest first; the last one for our MAC is current.
python3 -I -c '
import json, sys
mac, path = sys.argv[1].lower(), sys.argv[2]
leases = [l for l in json.load(open(path)) if l["mac-address"].lower() == mac]
if not leases:
    sys.exit("no DHCP lease for " + mac)
print(max(leases, key=lambda l: l["expiry-time"])["ip-address"])
' "$mac" "$status"
