#!/usr/bin/env python3
"""Receive UDP datagrams (e.g. from tailscale-rs's peer_ping) and report their timing.

Usage: udp-recv-timing.py [port] [expected-interval-seconds]     (defaults: 5678 1.0)

Prints one line per datagram with the gap since the previous one, and a summary on Ctrl+C:
gap percentiles, how many arrived late (> 1.5 x interval) or bunched (< 0.5 x interval).
"""

import socket
import statistics
import sys
import time

port = int(sys.argv[1]) if len(sys.argv) > 1 else 5678
interval = float(sys.argv[2]) if len(sys.argv) > 2 else 1.0

sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
sock.bind(("0.0.0.0", port))
print(f"listening on UDP {port}, expecting one datagram every {interval:g} s (Ctrl+C for summary)")

gaps = []
last = None
start = None
try:
    while True:
        data, addr = sock.recvfrom(2048)
        now = time.monotonic()
        if start is None:
            start = now
        if last is None:
            print(f"{now - start:9.3f}s  from {addr[0]}:{addr[1]}  {data!r}")
        else:
            gap = now - last
            gaps.append(gap)
            mark = "  LATE" if gap > 1.5 * interval else "  BUNCHED" if gap < 0.5 * interval else ""
            print(f"{now - start:9.3f}s  gap {gap * 1000:7.1f} ms{mark}")
        last = now
except KeyboardInterrupt:
    pass

if len(gaps) < 2:
    print("\nnot enough datagrams for a summary")
    sys.exit(0)

gaps.sort()


def pct(p):
    return gaps[min(len(gaps) - 1, int(p / 100 * len(gaps)))] * 1000


late = sum(g > 1.5 * interval for g in gaps)
bunched = sum(g < 0.5 * interval for g in gaps)
print(f"\n{len(gaps) + 1} datagrams, {len(gaps)} gaps")
print(f"gap ms: min {gaps[0] * 1000:.1f}  p50 {pct(50):.1f}  p90 {pct(90):.1f}  p99 {pct(99):.1f}  "
      f"max {gaps[-1] * 1000:.1f}  stdev {statistics.stdev(gaps) * 1000:.1f}")
print(f"late (> {1.5 * interval:g} s): {late}   bunched (< {0.5 * interval:g} s): {bunched}")
