# Porting Tailscale to QNX OS 8.0 (via tailscale-rs)

Target: a QNX 8.0 program on the tailnet, x86_64 first (QEMU target
`~/qnxprojects/targets/qemu-qnx800-x86_64`), aarch64 after.
Source: `~/Projects/tailscale-rs`, started @ `4d66c5a` (v0.6.1); now tracking v0.7.1 (`a2a56c4` + local
commits). Rust 2024, pinned 1.98.1.

## Ecosystem status for `target_os = "qnx"` (checked against the locked crate versions)

| Crate (locked) | Knows `nto` (QNX 7.x) | Knows `qnx` (8.0) | Action |
|---|---|---|---|
| `libc` 0.2.191 | yes | **yes** | none |
| `cc` 1.8.0 | – | **yes** | none |
| `mio` 1.2.4 | yes (poll(2) backend) | no | patch: treat `qnx` like `nto` (**critical**, it's tokio's event loop) |
| `tokio` 1.54.1 | yes (ucred, read_dir, process) | no | patch |
| `socket2` 0.6.5 | yes | no | patch |
| `getrandom` 0.2.17 / 0.3.4 / 0.4.3 | yes | no | patch all three (three versions in the tree) |
| `rustix` 1.1.5 | yes (30 files) | no | patch, or check whether it can be dropped from the qnx build |
| `errno` 0.3.14 | yes | no | patch |
| `gethostname` 1.1.0 | – | – | probably generic unix; verify |
| `aws-lc-sys` 0.46.0 | – | – | C/CMake cross-build for QNX is the **biggest unknown**; avoid by switching rustls to `ring` |
| `rtnetlink`, `windows` | – | – | already cfg-gated to linux/windows |

Most patches are mechanical (`target_os = "nto"` → `any(target_os = "nto", target_os = "qnx")`).
Where a crate branches on `target_env = "nto71_iosock"`, QNX 8.0 should take the **io-sock**
branch, because 8.0 only has io-sock. Each one is a small upstream PR, likely welcome since `libc`
already supports `qnx`.

---

## Phase 0: Rust toolchain for QNX 8.0 (1–2 days) ✅ done 2026-11-09

- [x] `rustup toolchain install nightly --component rust-src`, then **pin a dated nightly** once it works
      (tier-3 targets can break from one nightly to the next). The repo's `rust-toolchain.toml` pins 1.98.1,
      so use `cargo +nightly-YYYY-MM-DD` or a local override.
      → Pinned **`nightly-2026-11-09`** (rustc 1.102.0-nightly `5b7b37aef`). This repo's `rust-toolchain.toml`
      selects it; in tailscale-rs use `cargo +nightly-2026-11-09`.
- [x] `~/Projects/tailscale-rs/.cargo/config.toml` (local, not committed upstream; listed in `.git/info/exclude`).
      Same as this repo's `.cargo/config.toml` minus `[build] target`:
      ```toml
      [target.x86_64-pc-qnx]
      linker = "qcc"
      runner = "/home/atharva/Projects/QNX/tailscale-QNX/scripts/qnx-run.sh"

      [unstable]
      build-std = ["std", "panic_abort"]

      [env]
      CC_x86_64_pc_qnx = "qcc"
      CFLAGS_x86_64_pc_qnx = "-Vgcc_ntox86_64"
      CXX_x86_64_pc_qnx = "q++"
      CXXFLAGS_x86_64_pc_qnx = "-Vgcc_ntox86_64_cxx"
      AR_x86_64_pc_qnx = "ntox86_64-ar"
      ```
      (Always `source ~/qnx800/qnxsdp-env.sh` first.) No `-C link-arg=-Vgcc_ntox86_64` rustflag: the rustc
      target spec already passes `-Vgcc_ntox86_64_cxx` to qcc. Sanity check: `ts_bart` cross-builds.
- [x] Hello world → copy to the QEMU target → runs (`cargo run -p hello`).
- [x] std smoke test on target (`smoke/std-smoke`): threads, Mutex/Condvar/mpsc, `Instant`/`SystemTime`/sleep,
      fs (write/append/rename/chmod/symlink/read_dir/canonicalize), env, `catch_unwind` + thread panics,
      TCP/UDP loopback on v4 **and v6**, non-blocking `WouldBlock`, DNS, `Command`, and outbound
      DNS + TCP to `controlplane.tailscale.com:443`. 13/13 pass in debug and release (`panic=abort`).
- [x] tokio smoke test (`smoke/tokio-smoke`, multi-thread `#[tokio::main]`): timers/interval/timeout, 1000 tasks,
      `spawn_blocking`, mpsc, UDP echo server with 32 concurrent clients × 100 round trips, 1 MiB TCP echo.
      4/4 pass in debug and release. `tokio-smoke serve 0.0.0.0:9999` + `scripts/udp-echo-client.py` from the
      host: 2000/2000 datagrams over virbr0, RTT p50 0.83 ms / p99 2.7 ms.
      Needed patched **mio, socket2 and tokio** (pulled forward from Phase 1, see below).

Exit: an async Rust binary built on the host runs on QNX. ✅

### Phase 0 notes / findings

- **Workflow**: `source ~/qnx800/qnxsdp-env.sh && cargo run -p <smoke-crate>` in this repo. The cargo runner
  `scripts/qnx-run.sh` strips the binary, scp's it to `/data/home/root/bin/` and runs it over ssh
  (exit code and args are forwarded; `QNX_HOST_IP` / `QNX_USER` override). `scripts/qnx-target-ip.sh` reads the
  VM's current address from the libvirt DHCP lease (it changes on every boot).
- **Target image fixes** (in `~/qnxprojects/targets/qemu-qnx800-x86_64/local/`):
  - sshd didn't start (`/usr/libexec/sshd-auth does not exist`): the SDP's OpenSSH needs `sshd-auth` but
    mkqnximage's `system.build` template only ships `sshd-session`. Added `bin/sshd-auth=usr/libexec/sshd-auth`
    to `snippets/system_files.custom` and an IFS link `/usr/libexec/sshd-auth=/system/bin/sshd-auth` to
    `snippets/ifs_files.custom`.
  - `--ssh-ident=~/.ssh/id_ed25519.pub` for root key login.
  - Boot headless: `./mkqnximage-wrapper.sh --run=-h` (console goes to `output/qemu.out`).
  - The data partition is only 61 MB, so deploy stripped binaries.
- **libc 0.2.190+ models `target_os = "qnx"` as `nto` + the io-sock branch** (it shares `src/unix/nto`), so the
  crate rule is: add `qnx` wherever `nto` appears, and send `qnx` down the `nto71_iosock` side of any
  `nto70`/`nto71` split. `scripts/add-qnx-cfg.py` does the mechanical part and flags `all(target_os = "nto", …)`
  sites for manual review.
- **UDP receive buffers are small on io-sock**: `net.inet.udp.recvspace = 42080` (FreeBSD default; Linux uses
  208 KiB). Each queued datagram is charged a full mbuf cluster, so a socket holds only about 20 before
  `netstat -s -p udp` shows "dropped due to full socket buffers". 32 concurrent UDP clients lost packets until
  the server set `SO_RCVBUF = 1 MiB`. The effective cap is just under `kern.ipc.maxsockbuf` (2 MiB → `ENOBUFS`).
  **Open item for Phase 3**: at 128+ concurrent loopback clients there are still about 3 drops per run, even
  with 1 MiB on server *and* clients and with 256 KiB vs 1 MiB making no difference, so some other io-sock limit
  is involved. mio isn't losing wakeups (every loss shows up in the kernel counter). tailscale-rs's
  magicsock/WireGuard socket should set a large `SO_RCVBUF` on QNX; check what it does today.

## Phase 1: Ecosystem patches (3–5 days)

- [ ] Fork the crates above into `~/Projects/QNX/qnx-crates/` at the **exact locked versions** and wire them up
      with `[patch.crates-io]`.
      → Started in Phase 0: `mio-1.2.4`, `socket2-0.6.5`, `tokio-1.54.1` (each a git repo: pristine crates.io
      commit + uncommitted QNX diff, `git diff` = the upstream patch). Wired into this repo's `Cargo.toml`;
      tailscale-rs: `[patch.crates-io]` (absolute paths) is in its local `.cargo/config.toml`, so `Cargo.toml`
      stays clean (`Cargo.lock` does change, don't commit it). With `-Zbuild-std`, cargo prints
      `warning: patch ... was not used in the crate graph` for every config-level patch. It's spurious: cargo
      also resolves the std library graph, applies config patches there too, and std doesn't use them.
      `cargo tree -i mio` and `cargo build -v` confirm the forks are what's compiled.
      First `-p tailscale --examples` build stops at **`aws-lc-sys`** (expected) and **`signal-hook-registry`**
      (`libc::SA_RESTART` missing for qnx; pulled in by tokio's `signal` feature).
- [ ] Discovery loop:
      `cargo +nightly build --target x86_64-pc-qnx -p tailscale --examples`. Fix the next failing crate, repeat.
- [ ] **mio**: add `qnx` to every `nto` cfg (poll selector, `net.rs` socket flags). Run mio's tests on target.
      → cfgs done (7 files, +23/−4), exercised by `tokio-smoke`; mio's own test suite not yet run on target.
- [ ] **tokio**: `ucred` (peer creds), `read_dir`, `process`. Only the `net`, `time`, `rt`, `sync` features matter for us.
      → done (3 files), now at **1.54.1** (what tailscale-rs v0.7.1 locks; the 1.53.3 fork was dropped). `qnx`
      uses `impl_nto` (`getpeereid`) for ucred. 1.53.3 also compiled `impl_netbsd` (NetBSD `LOCAL_PEEREID`, absent
      from QNX 8's FreeBSD-style headers) for `nto`; 1.54.1 limits it to netbsd, so the patch is purely additive.
      With it, `ts_tls_util` (ring provider) builds for QNX in tailscale-rs.
- [ ] **socket2**: → cfgs done (4 files); the two `all(target_os = "nto", any(nto70, nto71))` `TCP_KEEPALIVE`
      sites correctly stay io-pkt-only, so `qnx` gets `TCP_KEEPIDLE`.
- [x] **getrandom** 0.2.17 / 0.3.4 / 0.4.3, all three in the QNX graph: 0.2 ← ring, 0.3 ← rand 0.9,
      0.4 ← rand 0.10 + x25519-dalek (WireGuard/Noise keys). QNX 8's libc has **no** `getrandom`,
      `getentropy` or `arc4random`, so `qnx` takes the `nto` path: the `use_file` backend reading
      `/dev/urandom` (served by QNX's `random` resmgr, seeded from `/data/var/random/rnd-seed`), with
      `__get_errno_ptr` for errno. 0.3/0.4 also need `qnx` in the `Cargo.toml` target table that pulls in
      `libc` (the `.rs`-only script misses manifests). Validated on target by `smoke/random-smoke`: 10k unique
      32-byte draws, bit balance, byte-histogram χ², from 4 threads, per version.
- [x] **signal-hook-registry 1.4.8** (via tokio's `signal` feature, which tokio pulls in for hyper-util →
      ts_http_util → ts_control/ts_derp/ts_netcheck, so it can't be dropped): failed on `libc::SA_RESTART`.
      QNX 8's `<signal.h>` has `/* #define SA_RESTART 0x0040 (not supported yet) */`, so the OS has no
      `SA_RESTART` and adding it to `libc` would be wrong. The crate's existing `nto` branch installs the
      handler with `flags = 0`; `qnx` now takes it too (2 lines).
      **Consequence:** with no `SA_RESTART`, a blocking syscall interrupted by a handled signal returns `EINTR`
      instead of restarting. std's `read_exact`/`write_all` and tokio/mio already retry on `Interrupted`, but
      any raw blocking call in tailscale-rs (or FFI users) must handle `EINTR`. Watch for it in Phase 3.
- [x] **errno 0.3.14** (dependency of signal-hook-registry): the `extern "C"` errno accessor is linked as
      `__get_errno_ptr` only on `nto`; on `qnx` it linked as `errno_location`, which doesn't exist (link-time
      failure only, so it built fine until a binary was linked). 1 line.
      Validated by `tokio-smoke`'s `signals` check on target: 5× SIGUSR1 + SIGHUP delivered via
      `tokio::signal`, delivered to the right stream, runtime still healthy, debug and release.
- [ ] **rustix**: same pattern (not hit yet).
- [ ] **TLS crypto**: add a `ring` feature to `ts_tls_util` (`tokio-rustls` `ring` feature;
      `insecure.rs` uses `rustls::crypto::aws_lc_rs::default_provider()` → make the provider selectable).
      `ring` 0.17 builds through `cc`, which already knows `qnx`. Later, optionally get `aws-lc-sys`
      cross-building via a CMake toolchain file for qcc.
      → Done in tailscale-rs (uncommitted, `crates/utils/ts_tls_util`): `tokio-rustls` gets `aws-lc-rs` on
      `cfg(not(target_os = "qnx"))` and `ring` on `cfg(target_os = "qnx")` (target-specific deps; resolver 3
      doesn't unify features across targets). A `CRYPTO_PROVIDER` static picks the matching provider and is
      passed via `ClientConfig::builder_with_provider`, in `connect_alpn` and `connect_insecure`. Host
      behaviour is unchanged: still aws-lc, same protocol versions; clippy `--all-features -D warnings` and
      nightly fmt are clean. `cargo tree --target x86_64-pc-qnx -i aws-lc-sys` is now empty.
      Why aws-lc failed: its build passes `--include=<prefix header>`, which qcc rejects (`qcc -include X`
      and `ntox86_64-gcc --include=X` both work).
      **ring 0.17.14 needs a fork** (2 lines), for two reasons:
      1. `src/rand.rs` only implements `SecureRandom for SystemRandom` on an OS allowlist without `qnx`
         (rustls then fails to compile). Added `qnx`; it then draws from getrandom 0.2 → `/dev/urandom`.
      2. `build.rs` only builds its perlasm for OSes in `LINUX_ABI`. Without `qnx` there, ring's C part
         builds but **binaries fail to link** (undefined `ring_core_0_17_14__sha512_block_data_order_nohw`,
         `aesni_gcm_decrypt`, ...): on x86_64 ring has no pure-C fallback; its Rust code calls the asm
         directly. (An earlier note here claiming a slower-but-working C fallback was wrong; libraries
         compiled, but nothing had been linked yet.) Added `qnx`: QNX x86_64 is SysV ELF, so the `elf`
         perlasm flavour applies.
      Validated on target by `smoke/random-smoke`: round trips + tamper rejection (AES-256-GCM,
      ChaCha20-Poly1305), SHA-256 KAT, X25519, Ed25519; plus `random-smoke ring-vectors`, a deterministic
      run over SHA-2/HMAC/AES-GCM/ChaCha20-Poly1305/Ed25519 at 19 lengths (0 B–64 KiB), whose digests match
      the Linux host exactly in debug and release (QEMU `--cpu host` → same AVX2/AES-NI paths).
      ring's own test suite wasn't run: the crates.io package omits its test vectors.
      With these, `ring`, `rustls`, `rand` 0.9/0.10 and `x25519-dalek` build for QNX inside tailscale-rs.
      Next blocker for `tokio-rustls`/`ts_tls_util`: tokio 1.54.1 (needs re-fork; tailscale-rs moved to v0.7.1).
      `signal-hook-registry` is fixed (see below).
- [ ] Verify ABI-sensitive bits against QNX 8.0 headers in `~/qnx800/target/qnx/usr/include`
      (`sockaddr` layouts, `msghdr`, `IP_PKTINFO`/`IP_RECVDSTADDR`, `SO_REUSEPORT`, `MSG_*` flags) wherever the crates' `nto` code
      assumed 7.x/io-pkt.

Exit: `tailscale` and its examples cross-compile for `x86_64-pc-qnx`.

## Phase 2: tailscale-rs changes for QNX (3–5 days)

- [ ] **`ts_netmon`**: the non-supported-OS fallback `NoopMon` uses `unimplemented!()`. First make sure
      that path is never called on qnx, or give it a polling implementation (re-read `getifaddrs` every N seconds).
      Then a real backend: reuse `src/bsd/` (PF_ROUTE routing socket, currently `cfg(macos)`).
      QNX io-sock is FreeBSD-derived, so adapt `bsd/net_table/{interface_hdr,route_hdr}.rs` to the FreeBSD
      `if_msghdr`/`rt_msghdr` layouts in `~/qnx800/target/qnx/usr/include/net/{if,route}.h`.
- [ ] Add `target_os = "qnx"` to `netmon.rs`/`lib.rs` cfgs and the `ts_netmon` `Cargo.toml` target deps
      (`libc`, `socket2`, `nom`, `zerocopy`… the macOS set).
- [ ] State / key file location: default to a writable QNX path (e.g. `/data/var/tailscale/`), since the root may be
      read-only/union in mkqnximage builds.
- [ ] OS reporting to control: `std::env::consts::OS == "qnx"`; check the admin console accepts and shows it.
- [ ] Gate anything else that misbehaves with a `qnx` cfg; keep the diff minimal and upstreamable.

## Phase 3: Validation on the QEMU target (2–3 days)

- [ ] Run `examples/tcp_echo` and `examples/peer_ping` on QNX with an auth key; peer them with a normal
      Go Tailscale client on your laptop.
- [ ] Device appears in the admin console; ping works both ways over DERP.
- [ ] Direct path: STUN/netcheck works through the QEMU NAT (virbr0 → Proxmox → LAN). Expect DERP fallback
      more often (upstream NAT traversal is still WIP, and there's double NAT here).
      Optionally test with `OPT_NETWORK=bridge` to put the QNX VM straight on the LAN.
- [ ] Throughput and latency vs a Linux build of the same example on the same box.
- [ ] 24h soak: key/DERP reconnects, netmon events (unplug/replug the virtual NIC), memory growth.
- [ ] Run the workspace unit tests on target where feasible (a `runner` in `.cargo/config.toml` that scp's + ssh-runs the binary).

Exit: a QNX process holds a stable tailnet connection and serves TCP to peers.

## Phase 4: QNX developer integration (2–4 days)

- [ ] Build `ts_ffi` for QNX as a `staticlib` (and `cdylib`); C sample built with `qcc`.
- [ ] Hook it into a QNX VS Code extension project (CMake + `ts_ffi.h`), so a C/C++ QNX app joins the tailnet.
- [ ] Package the binary in the mkqnximage target (`local/snippets/system_files.custom`), auto-start via
      `post_start.custom`; auth key / state on the data partition.
- [ ] aarch64: same steps with `aarch64-unknown-qnx` and `-Vgcc_ntoaarch64le`.

## Phase 5: Stretch goals

- [ ] **System-wide VPN for all QNX apps**: a small daemon that reads/writes IP packets on io-sock's
      FreeBSD-style `/dev/tun` (`net/if_tun.h`: `TUNSIFHEAD`, `TUNSIFMODE`) and feeds them to tailscale-rs's
      dataplane instead of smoltcp. Needs a `tun-rs` QNX backend (FreeBSD backend as template) plus
      route setup via `route`/PF_ROUTE. Depends on tailscale-rs exposing a raw-packet path; check upstream first.
- [ ] `ssh` feature (`russh` with aws-lc → needs `aws-lc-sys` on QNX, or russh with ring).
- [ ] Upstream: crate patches → mio/tokio/socket2/getrandom/rustix/errno; QNX in tailscale-rs's platform list
      (talk to maintainers about CI: cross-build + QEMU QNX runner).

---

## Risks

| Risk | Mitigation |
|---|---|
| Tier-3 target breaks on nightly | Pin a dated nightly; std smoke tests in Phase 0 |
| A crate's `nto` path assumes QNX 7.x / io-pkt ABI | Diff against QNX 8.0 headers; prefer `nto71_iosock` branches; test on target |
| `aws-lc-sys` won't cross-build | Use `ring` provider (Phase 1) |
| tailscale-rs is preview: unaudited crypto, API churn, NAT traversal WIP | Fine for dev/prototyping; track upstream monthly releases; don't ship in safety-critical paths yet |
| Library-only, no system-wide VPN | Phase 5 tun daemon, or put a Linux subnet router in front of the QNX network |

## Rough effort

| Phase | Effort |
|---|---|
| 0 Toolchain | 1–2 days |
| 1 Crate patches | 3–5 days |
| 2 tailscale-rs changes | 3–5 days |
| 3 Validation | 2–3 days |
| 4 Integration | 2–4 days |

**First QNX binary on the tailnet: about 1–2 weeks** (vs about 2–3 months for the Go route).

## Alternative considered: Go tailscale

Would need a full Go port (`GOOS=qnx`): a libc-calling runtime like solaris/aix, a poll-based netpoller
(QNX has no userland kqueue/epoll), external linking through qcc for QNX's ELF layout
(`/usr/lib/ldqnx-64.so.2` + QNX `.note`), and `x/sys/unix` for QNX. Benefit: the full `tailscaled`
feature set (tun, subnet routing, SSH, MagicDNS). Revisit only if Phase 5's system-wide VPN proves
insufficient.

The Go client would first need a Go runtime port to QNX (`GOOS=qnx` doesn't exist):
months of work. tailscale-rs is Tailscale's official Rust reimplementation.
Rust already has a QNX 8.0 target, and the code is almost OS-agnostic:

- Rust target **`x86_64-pc-qnx`** / `aarch64-unknown-qnx` (`target_os = "qnx"`, QNX SDP 8.0+).
  It's tier 3: no prebuilt std, so build with nightly `-Zbuild-std`. Linking goes through `qcc`.
- The only OS-specific code in tailscale-rs is **`ts_netmon`** (Linux netlink / macOS PF_ROUTE / Windows),
  plus an optional `tun-rs` device in `ts_netstack_smoltcp`. Everything else is portable Rust:
  Noise control protocol, WireGuard (`ts_tunnel`), DERP, disco, STUN, smoltcp netstack.
- Root CAs come from bundled `webpki-roots`, so a system cert store on QNX isn't needed.
- No runtime experiment gate in this repo.

**What you get:** like `tsnet`, tailscale-rs is a **library**. A QNX process embeds it and opens
TCP/UDP sockets on the tailnet through a userspace netstack. It doesn't provide a system-wide
VPN (`tailscaled` + tun + routes); that's a stretch goal (Phase 5). There are C bindings (`ts_ffi`),
so QNX C/C++ apps can use it too.