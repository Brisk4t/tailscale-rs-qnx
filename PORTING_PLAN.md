# Porting tailscale-rs to QNX 8.0

**Goal:** a QNX 8.0 program on a tailnet. x86_64 first, on the QEMU target
`~/qnxprojects/targets/qemu-qnx800-x86_64`; aarch64 later.

**Source:** `~/Projects/tailscale-rs` at v0.7.1 (`a2a56c4`) plus local commits.

**What you get:** tailscale-rs is a library, like Go's `tsnet`. A QNX process embeds it and opens TCP/UDP
sockets on the tailnet through a userspace network stack (smoltcp); other processes on the device aren't on
the tailnet. C/C++ apps can use it through `ts_ffi`.

How to build and run everything: [README.md](README.md).

---

## Status

| Phase | Estimate | Status |
|---|---|---|
| 0. Rust toolchain for QNX | 1–2 days | ✅ Done 2026-11-09 |
| 1. Patch the crate ecosystem | 3–5 days | ✅ Exit reached 2026-11-09 (2 follow-ups open) |
| 2. tailscale-rs changes for QNX | 3–5 days | Not started |
| 3. Validate on the QEMU target | 2–3 days | **In progress.** First end-to-end echo over the tailnet on 2026-11-10 |
| 4. QNX developer integration | 2–4 days | Not started |
| 5. Stretch goals | – | Not started |

### Works on QNX today

- Release `examples/tcp_echo` on the QEMU VM:
  - logs in to `controlplane.tailscale.com`
  - gets a tailnet address (100.77.109.107)
  - echoes `nc` input from a peer running the regular Tailscale client, over DERP
- That exercises TLS (ring), the control channel, DERP, WireGuard (`ts_tunnel`) and the smoltcp stack.
- All six smoke tests pass on the target: `hello`, `std-smoke`, `tokio-smoke`, `random-smoke`,
  `rustix-smoke` and `netmon-smoke`.

---

## Open issues

| # | Issue | Impact | Cause | Next step |
|---|---|---|---|---|
| 1 | **Interactive login exits with `410 Gone`** (`auth path not found`) after the URL is approved | First run dies; a rerun with the saved `tsrs_keys.json` works | Probably upstream: the follow-up long-poll fails | Repeat on the Linux host build. If it fails there too, report upstream |
| 2 | **One `nc` attempt dropped** with `ts_tunnel: session not found` | Peer's TCP connection hangs; a later attempt worked | Unknown: stale session after a restart, or a replaced handshake | Watch for it with `RUST_LOG=ts_tunnel=debug` |
| 3 | **Network monitor is polling-only** | Changes noticed up to 10 s late; no routes reported | New `ts_netmon::PollingMon` (Go's `pollingMon` equivalent) is the fallback; there's no PF_ROUTE backend for QNX | Good enough for now. Run a tailnet test with it; PF_ROUTE backend only if needed |
| 4 | **`tailscale ping` gets no reply over DERP** | Can't ping-check the node until there's a direct path | Upstream: `ts_runtime::disco` only answers pings that arrive over UDP | None needed. Use `nc` |
| 5 | **UDP drops under load** | Possible WireGuard packet loss at high rates | io-sock's default UDP receive buffer is 42 KB. 1 MiB fixed 32 clients, but 128+ still drop about 3 per run | Check what tailscale-rs's UDP sockets set. Measure throughput |
| 6 | **No `SA_RESTART`** on QNX 8 | Blocking syscalls interrupted by a signal return `EINTR` | OS limitation | std and tokio retry; audit raw calls and FFI users |
| 7 | **ABI not reviewed** | Wrong constants or struct layouts would compile and fail at runtime | The forks reuse code written for QNX 7 (`nto`) | Check `sockaddr`, `msghdr`, `IP_PKTINFO`, `SO_REUSEPORT` and `MSG_*` against `~/qnx800/target/qnx/usr/include` |
| 8 | **mio drops a readable event** after a writable one, in about 1 in 20 connects | mio's `connect` and `shutdown_read` tests hang or fail. tokio and tailscale-rs: probably none, because the next read re-arms it | QNX sometimes reports a just-connected socket as readable (`RDNORM\|OUT`); mio then stops waiting on `RDNORM`. mio's suite: 142 passed, 4 failed (2 from this, 2 harmless) | Optional: report to QNX with a C reproducer. See [diagnostics.md](diagnostics.md) |
| 9 | **Debug binaries are too big** | Debug examples are 319 MB, 42 MB stripped, against a 61 MB `/data` | – | Deploy release builds |

---

## Phase 0: Rust toolchain ✅

- [x] Pin a nightly that works: **`nightly-2026-11-09`** (rustc 1.102.0-nightly `5b7b37aef`).
- [x] Cargo config for `x86_64-pc-qnx`:
  - linker: qcc
  - `-Zbuild-std`
  - `CC`/`CXX`/`AR` for C dependencies
  - runner: `scripts/qnx-run.sh`
- [x] Fix the target image so sshd starts and accepts root key login (see README §4).
- [x] Smoke tests pass on the target:
  - [x] `hello`
  - [x] `std-smoke` (13 checks)
  - [x] `tokio-smoke` (5 checks)

**Exit:** an async Rust binary built on the host runs on QNX ✅

## Phase 1: Crate ecosystem ✅

- [x] Fork each crate that doesn't know `target_os = "qnx"` into `~/Projects/QNX/qnx-crates/`, at the locked
      versions (see [Crate patches](#crate-patches)).
- [x] Discovery loop: build `-p tailscale --examples` and fix the next failing crate, until the whole build
      succeeds.
- [x] Switch TLS from aws-lc-rs to ring on QNX (see [tailscale-rs changes](#tailscale-rs-changes)).
- [ ] Review the ABI against QNX 8 headers (open issue 7).
- [x] Run mio's test suite on the target: 142 passed, 4 failed. Two come from a QNX `poll()` quirk (open issue 8), two are harmless test expectations.

**Exit:** `tailscale` and its examples cross-compile for `x86_64-pc-qnx` ✅

## Phase 2: tailscale-rs changes for QNX

- [ ] **Network monitor:**
  - [x] `ts_netmon::PollingMon`, the equivalent of Go's `pollingMon`:
    - re-reads `getifaddrs` every 10 s and diffs against the last snapshot
    - the `platform_mon()` fallback on every Unix without a dedicated backend (previously `None`)
    - validated on QNX by `smoke/netmon-smoke`
    - in `patches/tailscale-rs/0002-polling-netmon.patch` (developed as `df19edc` on the local `qnx` branch)
  - [x] Run `tcp_echo` with it against the tailnet; check the runtime advertises the LAN address.
  - [ ] Optional real backend: reuse `ts_netmon/src/bsd/` (PF_ROUTE, currently macOS-only). io-sock is
        FreeBSD-derived, so adapt `bsd/net_table/{interface_hdr,route_hdr}.rs` to the layouts in
        `~/qnx800/target/qnx/usr/include/net/{if,route}.h`.

## Phase 3: Validation on the QEMU target

- [x] `tcp_echo` joins the tailnet and serves TCP to a Go-client peer over DERP.
- [x] The device appears in the admin console.
- [x] `peer_ping` example.
- [ ] Throughput and latency compared with the same example on Linux.
- [ ] 24-hour soak test: DERP and key reconnects, unplugging and replugging the virtual network card, memory growth.
- [ ] Workspace unit tests on the target, where feasible.

**Exit:** a QNX process holds a stable tailnet connection and serves TCP to peers.

## Phase 4: QNX developer integration

- [ ] Build `ts_ffi` as a `staticlib`/`cdylib`, plus a C sample built with qcc.
- [ ] Use it from a QNX VS Code extension project (CMake + `ts_ffi.h`).
- [ ] Package the binary into the mkqnximage target, auto-started, with state on `/data`.
- [ ] Direct (non-DERP) path, on a real QNX device on the LAN running the packaged image. The QEMU VM sits behind
      two NATs, so it isn't tested there. Pass: `tailscale ping` from a peer replies `via <LAN ip:port>`, and the
      console lists the device's LAN endpoint.
- [ ] aarch64: `aarch64-unknown-qnx` with `-Vgcc_ntoaarch64le`.

## Phase 5: Stretch goals

- [ ] `ssh` feature (needs aws-lc-sys on QNX, or russh with ring).
- [ ] Upstream the crate patches, and get QNX into tailscale-rs's platform list with CI.

---

## Crate patches

libc models QNX 8 (`target_os = "qnx"`) as QNX 7's `nto` code plus the io-sock network stack. Most crates
only check for `nto`, so the patching rule is:

> Add `qnx` wherever `nto` appears. Where a crate splits `nto70`/`nto71`, send `qnx` down the **io-sock**
> (`nto71_iosock`) side.

`scripts/add-qnx-cfg.py` does the mechanical part. Some crates also gate dependencies in `Cargo.toml`, which
the script doesn't touch. Each fork is a pristine crates.io commit plus an uncommitted diff, saved as
`patches/<crate>-<version>.patch`.

| Crate | Why it needs a patch | Change | Validated by |
|---|---|---|---|
| `mio` 1.2.4 | tokio's event loop; its poll(2) backend only enabled for `nto` | 7 files | `tokio-smoke` |
| `socket2` 0.6.5 | socket options gated on `nto` | 4 files | `tokio-smoke` |
| `tokio` 1.54.1 | peer credentials, `read_dir`, process uid/gid | 3 files | `tokio-smoke` |
| `getrandom` 0.2.17 / 0.3.4 / 0.4.3 | QNX 8 has no `getrandom`/`getentropy`/`arc4random`; uses `/dev/urandom` like `nto` | 2–3 files each, plus `Cargo.toml` for 0.3/0.4 | `random-smoke` |
| `ring` 0.17.14 | `SystemRandom` OS allowlist; assembly only built for OSes in `LINUX_ABI` (link fails otherwise, no C fallback on x86_64) | 2 lines | `random-smoke`, plus digests identical to the host |
| `signal-hook-registry` 1.4.8 | uses `SA_RESTART`, which QNX 8 doesn't have; takes `nto`'s `flags = 0` path | 2 lines | `tokio-smoke` signals check |
| `errno` 0.3.14 | errno accessor must link as `__get_errno_ptr` | 1 line | `tokio-smoke` signals check |
| `rustix` 1.1.6 | without it QNX falls into rustix's Linux code (used by `gethostname`) | 30 files, all mechanical | `rustix-smoke` |
| `wait-timeout` 0.2.1 | dev-dependency only (`proptest` → `rusty-fork`), needed to build tailscale-rs examples and tests; uses `SA_RESTART`. Also drops `#![deny(warnings)]`, since path dependencies aren't lint-capped | 2 files | `ts_netmon` unit tests on the target (9/9) |

No patch needed: `libc` 0.2.191, `cc` 1.8.0, `gethostname` 1.1.0. `rtnetlink` and `windows` are already gated
to their OSes.

### Wiring the forks in

- **This repo:** `[patch.crates-io]` in `Cargo.toml`.
- **tailscale-rs:** `[patch.crates-io]` with absolute paths, in a local `.cargo/config.toml`.
  - The config is excluded via `.git/info/exclude`, so upstream files stay clean.
  - Don't commit the `Cargo.lock` changes.
- **The "patch not used" warning is spurious.** With `-Zbuild-std`, cargo prints
  `warning: patch ... was not used in the crate graph` for every config-level patch, because it also resolves
  std's dependency graph. `cargo tree -i <crate>` confirms the fork is used.

## tailscale-rs changes

| Where | Change | Why |
|---|---|---|
| `crates/utils/ts_tls_util/Cargo.toml` | `tokio-rustls` uses `aws-lc-rs` except on QNX, where it uses `ring` (target-specific dependencies) | aws-lc-sys's build passes `--include=<header>`, which qcc rejects |
| `crates/utils/ts_tls_util/src/lib.rs`, `insecure.rs` | A `CRYPTO_PROVIDER` static, passed through `ClientConfig::builder_with_provider` | The provider choice doesn't depend on which rustls features other crates enable. Host behaviour is unchanged |
| `crates/net/underlay/ts_netmon/src/polling.rs` + cfg wiring | `PollingMon`: polls `getifaddrs` every 10 s; the `platform_mon()` fallback on Unix without a dedicated backend | QNX has no netmon backend (open issue 3). Validated by `netmon-smoke`, the `monitor` example, and ts_netmon's unit tests on the target |
| `.cargo/config.toml` (local only) | QNX target config and `[patch.crates-io]` | See above |

Developed on a local tailscale-rs branch `qnx` (upstream has no QNX branch) and exported, source only, without
`Cargo.lock`, as `patches/tailscale-rs/0001-tls-ring-on-qnx.patch` and `0002-polling-netmon.patch`. They apply in
order to tag `v0.7.1`; `scripts/setup.sh` applies them to a fresh clone.

---

## QNX 8 platform facts

| Fact | Consequence |
|---|---|
| No `getrandom`, `getentropy` or `arc4random` | Randomness comes from `/dev/urandom`, served by the `random` resource manager |
| No `SA_RESTART` (commented out in `<signal.h>`) | Interrupted syscalls return `EINTR` (open issue 6) |
| io-sock is FreeBSD-derived | PF_ROUTE and `if_msghdr`/`rt_msghdr` layouts as on FreeBSD |
| UDP receive buffer defaults to 42 KB (`net.inet.udp.recvspace`); the maximum is just under 2 MiB (`kern.ipc.maxsockbuf`) | Raise `SO_RCVBUF` on busy UDP sockets (open issue 5) |
| x86_64 uses the SysV ELF ABI | ring's `elf` assembly applies as is |
| errno 265 is `EHOSTUNREACH` | Seen when sending to IPv6 peers; the QEMU NAT is IPv4-only |
| The qnx6 filesystem frees deleted space only after `sync` | Run `sync` before checking free space on `/data` |
| The QEMU image's `/data` is 61 MB | Deploy release (or stripped) binaries |

## Risks

| Risk | Mitigation |
|---|---|
| A nightly update breaks the tier-3 target | Pinned nightly; smoke tests |
| A crate's `nto` code assumes the QNX 7 / io-pkt ABI | ABI review (open issue 7); test on the target |
| tailscale-rs is preview software: unaudited crypto, API churn, NAT traversal still in progress | Fine for development; track upstream releases; keep it out of safety-critical paths |

## Why tailscale-rs and not the Go client

The Go client would first need a Go runtime port to QNX. `GOOS=qnx` doesn't exist, and a port needs:
- a libc-calling runtime
- a poll-based network poller (QNX has no epoll or kqueue)
- qcc linking
- `x/sys/unix` support

That's months of work. It would bring full `tailscaled` (tun, subnet routing, SSH, MagicDNS); revisit only if
a system-wide tailnet interface on QNX becomes a requirement.

tailscale-rs is Tailscale's official Rust implementation, and Rust already has a QNX 8 target. Its only
OS-specific code is the network monitor (`ts_netmon`). The rest is portable Rust:
- the control protocol
- WireGuard
- DERP
- disco
- STUN
- the smoltcp network stack

Root certificates come from the bundled `webpki-roots`, so no system certificate store is needed.
