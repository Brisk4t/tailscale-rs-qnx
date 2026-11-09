#!/usr/bin/env python3
"""Host-side client for `tokio-smoke serve`: send N datagrams, expect each echoed back."""
import socket
import sys
import time

host, port = sys.argv[1], int(sys.argv[2])
count = int(sys.argv[3]) if len(sys.argv) > 3 else 1000
s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
s.settimeout(2)
lost, rtts = 0, []
for i in range(count):
    msg = f"host msg {i}".encode()
    t = time.perf_counter()
    s.sendto(msg, (host, port))
    try:
        data, _ = s.recvfrom(2048)
        if data != msg:
            sys.exit(f"mismatch at {i}: {data!r}")
        rtts.append(time.perf_counter() - t)
    except socket.timeout:
        lost += 1
rtts.sort()
p = lambda q: rtts[int(q * (len(rtts) - 1))] * 1e3 if rtts else float("nan")
print(f"{count - lost}/{count} echoed from {host}:{port}, rtt p50={p(.5):.3f}ms p99={p(.99):.3f}ms")
sys.exit(1 if lost else 0)
