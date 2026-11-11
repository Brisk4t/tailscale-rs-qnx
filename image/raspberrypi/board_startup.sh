#!/bin/ksh
# Raspberry Pi QNX image: start ts-forward at boot, and restart it if it exits.
#
# Installed as /scripts/board_startup.sh, which the BSP's customize_startup.sh runs after networking has
# been started. ts-forward's files live on the SD card's FAT boot partition, in ts-forward/:
#   keys.json        its tailnet identity (created on first start)
#   ts-forward.log   its log (the previous boot's is ts-forward.log.old)
#   authkey          optional auth key, used for the first registration
#   args             optional ts-forward arguments (default: -p ssh)
#   hostname         optional QNX hostname, which ts-forward also uses as its tailnet name
#                    (default: qnx-tailscale)

if ! waitfor /dev/sd0t12 10; then
    echo "ts-forward: no FAT boot partition (/dev/sd0t12) on the SD card, not starting"
    exit 0
fi
mkdir -p /bootfs
mount -t dos /dev/sd0t12 /bootfs
mkdir -p /bootfs/ts-forward
cd /bootfs/ts-forward

# The BSP image doesn't set a hostname ("localhost"); ts-forward uses it as the device's tailnet name.
name=qnx-tailscale
[ -r hostname ] && name=$(cat hostname)
hostname "$name"

[ -f ts-forward.log ] && mv -f ts-forward.log ts-forward.log.old
echo "Starting ts-forward (log: /bootfs/ts-forward/ts-forward.log)"

(
    # The DHCP client works in the background: wait for a default route.
    i=0
    while ! route -n get default >/dev/null 2>&1 && [ $i -lt 120 ]; do sleep 1; i=$((i + 1)); done
    echo "network: default route after $i s"

    # The Pi has no battery-backed clock, and TLS fails while it says 1970: set it.
    n=0
    while ! ntpdate -u pool.ntp.org >/dev/null 2>&1 && [ $n -lt 30 ]; do sleep 2; n=$((n + 1)); done
    echo "clock: $(date)"

    if [ -r authkey ]; then TS_AUTH_KEY=$(cat authkey); export TS_AUTH_KEY; fi
    args="-p ssh"
    [ -r args ] && args=$(cat args)

    while true; do
        echo "ts-forward starting: $(date), args: $args"
        /usr/bin/ts-forward -c /bootfs/ts-forward/keys.json $args
        echo "ts-forward exited with status $?; restarting in 10 s"
        sleep 10
    done
) >> /bootfs/ts-forward/ts-forward.log 2>&1 &
