# tailscale-QNX

Porting [tailscale-rs](https://github.com/tailscale/tailscale-rs) to QNX OS 8.0 (`x86_64-pc-qnx`).

**Status:** tailscale-rs cross-compiles for QNX, and its `tcp_echo` example runs on a QEMU QNX VM. It joins a
tailnet and serves TCP to other devices on it. Plan, open issues and findings: [PORTING_PLAN.md](PORTING_PLAN.md).
Bugs under investigation: [diagnostics.md](diagnostics.md).

## What's here

| Path | What it is |
|---|---|
| `smoke/hello` | Hello world for the QNX target |
| `smoke/std-smoke` | std checks: threads, sync, time, fs, env, panics, TCP/UDP v4+v6, DNS, process, internet |
| `smoke/tokio-smoke` | tokio checks: timers, tasks, UDP echo (32 clients), 1 MiB TCP echo, `tokio::signal`; also a `serve` mode |
| `smoke/random-smoke` | getrandom 0.2/0.3/0.4 and ring checks; `ring-vectors` mode for comparing against the host |
| `smoke/rustix-smoke` | `gethostname` and `rustix::system::uname` against the target's `hostname` and `uname` |
| `smoke/netmon-smoke` | tailscale-rs's polling network monitor (`ts_netmon::PollingMon`) against `ifconfig`, plus change detection |
| `patches/<crate>-<version>.patch` | QNX patches for mio, socket2, tokio, getrandom ×3, ring, signal-hook-registry, errno, rustix, wait-timeout |
| `patches/tailscale-rs/tailscale-rs-0.7.1.patch` | The QNX changes to tailscale-rs itself: TLS via ring, polling network monitor |
| `scripts/setup-qnx-crates.sh` | Downloads the crates from crates.io and applies the patches |
| `scripts/qnx-run.sh` | Cargo runner: copies a binary to the QNX target and runs it over ssh |
| `scripts/qnx-target-ip.sh` | Prints the QEMU target's current IP |
| `scripts/udp-echo-client.py` | Host-side client for `tokio-smoke serve` |
| `smoke/poll-probe` | What QNX's `poll()` reports when a TCP peer closes; `mio` replays mio's `tcp::connect` test; `mio-loop [N]` repeats it N times and counts lost readable events |
| `scripts/run-mio-tests.sh` | Builds mio's test suite for QNX and runs it on the target (see diagnostics.md) |
| `scripts/add-qnx-cfg.py` | Adds `target_os = "qnx"` next to every `target_os = "nto"` cfg (for patching more crates) |

## Prerequisites

- **Host:**
  - Linux x86_64 with `rustup`, `git`, `curl` and `python3`.
  - An ssh key at `~/.ssh/id_ed25519` (create one with `ssh-keygen -t ed25519`).
- **QNX SDP 8.0** at `~/qnx800`. It provides `qcc`, the `ntox86_64-*` binutils and `mkqnximage`.
- **QEMU and libvirt** for the target VM:
  - `qemu-system-x86_64`
  - libvirt's default NAT network, bridge `virbr0`
  - `allow virbr0` in `/etc/qemu/bridge.conf`, and a setuid `qemu-bridge-helper`
  - Optional: membership of the `kvm` group, which makes the VM much faster.

Expected layout:

```
~/Projects/
├── QNX/
│   ├── tailscale-QNX/      this repo
│   └── qnx-crates/         patched crate forks (step 2)
└── tailscale-rs/           upstream checkout at v0.7.1 (step 6)
```

This repo's `Cargo.toml` finds the forks by relative path. tailscale-rs's local config uses absolute paths.

## 1. Toolchain

`x86_64-pc-qnx` is a tier-3 Rust target with no prebuilt std, so std is built from source with nightly
`-Zbuild-std`. `rust-toolchain.toml` pins a nightly that is known to work:

```bash
rustup toolchain install nightly-2026-11-09 --component rust-src --profile minimal
```

Every shell that builds for QNX needs the SDP environment, which puts `qcc` on `PATH`:

```bash
source ~/qnx800/qnxsdp-env.sh
```

`.cargo/config.toml` holds the rest:
- target `x86_64-pc-qnx`
- `qcc` as the linker
- `build-std`
- the `CC`/`CXX`/`AR` settings for C dependencies
- `scripts/qnx-run.sh` as the runner

## 2. Patched crates

Several crates don't know `target_os = "qnx"` yet. Create the patched forks:

```bash
scripts/setup-qnx-crates.sh          # -> ~/Projects/QNX/qnx-crates/<crate>-<version>/
```

Each fork is a git repo: one commit with the pristine crates.io source, and the QNX patch left uncommitted.
`git diff` in a fork shows exactly the change. PORTING_PLAN.md explains why each crate needs a patch.

## 3. Build

```bash
source ~/qnx800/qnxsdp-env.sh
cargo build --workspace              # debug
cargo build --workspace --release    # release (panic=abort)
```

The first build compiles std itself and takes about a minute. Binaries go to
`target/x86_64-pc-qnx/{debug,release}/`.

## 4. Set up the QEMU target (once)

The target is a mkqnximage project at `~/qnxprojects/targets/qemu-qnx800-x86_64`, for example one created by
the QNX VS Code extension. It needs two changes before you can deploy over ssh.

**a. Fix sshd.** The SDP's OpenSSH needs `sshd-auth`, but mkqnximage's image template only installs
`sshd-session`. Without this fix, sshd fails at boot with `/usr/libexec/sshd-auth does not exist`.

```bash
cd ~/qnxprojects/targets/qemu-qnx800-x86_64
cat >> local/snippets/system_files.custom <<'EOF'

# OpenSSH 10 in the SDP needs sshd-auth; the stock template only installs sshd-session.
bin/sshd-auth=usr/libexec/sshd-auth
EOF
cat >> local/snippets/ifs_files.custom <<'EOF'

# sshd looks for sshd-auth at its compiled-in default path.
[type=link] /usr/libexec/sshd-auth=/system/bin/sshd-auth
EOF
```

**b. Install your ssh key for root and rebuild.** Rebuilding the image **wipes the data partition**.

```bash
./mkqnximage-wrapper.sh --ssh-ident=$HOME/.ssh/id_ed25519.pub --build
```

**Using the QNX VS Code extension from another machine?** The extension keeps its target list in your local
VS Code user settings. Add the existing VM to `qnx.targets.list`, with `"vmtype": "qemu"` and
`"path": "/home/<you>/qnxprojects/targets/qemu-qnx800-x86_64"`. Don't use "Create QNX Virtual Target" for it:
that rebuilds the image.

## 5. Run the smoke tests

Boot the VM headless. The serial console goes to `output/qemu.out`.

```bash
cd ~/qnxprojects/targets/qemu-qnx800-x86_64
./mkqnximage-wrapper.sh --run=-h
grep 'Startup complete' output/qemu.out     # booted when this prints
```

Then, from this repo, `cargo run` builds, deploys and runs in one step:

```bash
source ~/qnx800/qnxsdp-env.sh
cargo run -p hello                 # hello from Rust on qnx (x86_64), pid ...
cargo run -p std-smoke             # 13 checks, 0 required failures
cargo run -p tokio-smoke           # 5 checks, 0 failures
cargo run -p random-smoke          # 4 checks, 0 failures
cargo run -p rustix-smoke          # 5 checks, 0 failures
cargo run -p netmon-smoke          # 5 checks, 0 failures (needs tailscale-rs checked out, step 6)
```

Add `--release` to test release builds. Each test exits non-zero if a required check fails. `std-smoke`'s
`net/internet` check is optional and prints `WARN` if the VM has no outbound network.

`netmon-smoke` builds `ts_netmon` from `~/Projects/tailscale-rs`. Its last two checks add and remove the
alias `10.99.0.1/32` on `lo0` to test change detection, so they only run on QNX, as root. On a Linux host it
runs the first three checks only.

**ring on QNX vs the host.** Check that ring produces the same bytes on both:

```bash
cargo run -q --release --target x86_64-unknown-linux-gnu -p random-smoke -- ring-vectors > /tmp/host.txt
cargo run -q --release -p random-smoke -- ring-vectors > /tmp/qnx.txt
diff /tmp/host.txt /tmp/qnx.txt && echo identical
```

**UDP over the real NIC (host ↔ target):**

```bash
IP=$(scripts/qnx-target-ip.sh)
SSH="ssh -o UserKnownHostsFile=$HOME/qnxprojects/targets/qemu-qnx800-x86_64/local/ssh-ident root@$IP"
cargo run -p tokio-smoke                                  # deploys the current build
$SSH 'cd bin && (./tokio-smoke serve 0.0.0.0:9999 > /tmp/serve.log 2>&1 &)'
python3 scripts/udp-echo-client.py $IP 9999 2000          # -> 2000/2000 echoed ..., rtt p50=...
$SSH 'slay -f tokio-smoke'
```

`tokio-smoke` knobs, set on the target:

| Variable | Default | Meaning |
|---|---|---|
| `UDP_CLIENTS` | 32 | concurrent UDP clients |
| `UDP_ROUNDS` | 100 | round trips per client |
| `UDP_RCVBUF` | 1 MiB | the server's `SO_RCVBUF` (QNX's default is small) |

## 6. Build tailscale-rs for QNX

**a. Check out v0.7.1 and apply the QNX patch.** The patch does two things:
- switches TLS from aws-lc-rs (which doesn't build with qcc) to ring on QNX; host builds still use aws-lc-rs
- adds a polling network monitor (`ts_netmon::PollingMon`), used on QNX and other Unixes with no dedicated backend

```bash
cd ~/Projects
git clone https://github.com/tailscale/tailscale-rs.git && cd tailscale-rs
git checkout v0.7.1
git apply ~/Projects/QNX/tailscale-QNX/patches/tailscale-rs/tailscale-rs-0.7.1.patch
```

**b. Add a local cargo config.** It holds the QNX target settings and points the patched crates at the
forks. It's excluded from git, so upstream files stay untouched. Replace `<you>` with your username:

```bash
mkdir -p .cargo && cat > .cargo/config.toml <<'EOF'
[target.x86_64-pc-qnx]
linker = "qcc"
runner = "/home/<you>/Projects/QNX/tailscale-QNX/scripts/qnx-run.sh"

[unstable]
build-std = ["std", "panic_abort"]

[env]
CC_x86_64_pc_qnx = "qcc"
CFLAGS_x86_64_pc_qnx = "-Vgcc_ntox86_64"
CXX_x86_64_pc_qnx = "q++"
CXXFLAGS_x86_64_pc_qnx = "-Vgcc_ntox86_64_cxx"
AR_x86_64_pc_qnx = "ntox86_64-ar"

[patch.crates-io]
mio     = { path = "/home/<you>/Projects/QNX/qnx-crates/mio-1.2.4" }
socket2 = { path = "/home/<you>/Projects/QNX/qnx-crates/socket2-0.6.5" }
tokio   = { path = "/home/<you>/Projects/QNX/qnx-crates/tokio-1.54.1" }
getrandom02 = { package = "getrandom", path = "/home/<you>/Projects/QNX/qnx-crates/getrandom-0.2.17" }
getrandom03 = { package = "getrandom", path = "/home/<you>/Projects/QNX/qnx-crates/getrandom-0.3.4" }
getrandom04 = { package = "getrandom", path = "/home/<you>/Projects/QNX/qnx-crates/getrandom-0.4.3" }
ring    = { path = "/home/<you>/Projects/QNX/qnx-crates/ring-0.17.14" }
signal-hook-registry = { path = "/home/<you>/Projects/QNX/qnx-crates/signal-hook-registry-1.4.8" }
errno   = { path = "/home/<you>/Projects/QNX/qnx-crates/errno-0.3.14" }
rustix  = { path = "/home/<you>/Projects/QNX/qnx-crates/rustix-1.1.6" }
wait-timeout = { path = "/home/<you>/Projects/QNX/qnx-crates/wait-timeout-0.2.1" }
EOF
echo '/.cargo/' >> .git/info/exclude
```

**c. Build.** tailscale-rs pins stable Rust, so pass the nightly and the target explicitly:

```bash
source ~/qnx800/qnxsdp-env.sh
cargo +nightly-2026-11-09 build --release --target x86_64-pc-qnx --example tcp_echo
```

- **Output:** `target/x86_64-pc-qnx/release/examples/tcp_echo`. The release profile uses LTO and strips the
  binary, so the build takes a few minutes.
- **Always use release builds** for the target. Debug examples are about 42 MB even stripped, and the VM's `/data`
  is 61 MB.
- **Cargo.lock changes:** the build rewrites `Cargo.lock` to point at the forks. Don't commit those changes.
- **Ignore `warning: patch ... was not used in the crate graph`.** It's spurious with `-Zbuild-std`.
  `cargo tree --target x86_64-pc-qnx -i mio` shows the fork is used.

Host builds and tests in tailscale-rs work as before, because the config only applies with
`--target x86_64-pc-qnx`.

## 7. Join a tailnet from QNX

**Deploy** the release binary:

```bash
IP=$(~/Projects/QNX/tailscale-QNX/scripts/qnx-target-ip.sh)
SSHOPT="-o UserKnownHostsFile=$HOME/qnxprojects/targets/qemu-qnx800-x86_64/local/ssh-ident"
scp $SSHOPT ~/Projects/tailscale-rs/target/x86_64-pc-qnx/release/examples/tcp_echo root@$IP:/data/home/root/bin/
ssh $SSHOPT root@$IP
```

**Run** it on the target:

```sh
cd /data/home/root/bin
./tcp_echo                          # or: TS_AUTH_KEY=tskey-auth-... ./tcp_echo
```

**Logging in:**
- **Without an auth key,** tcp_echo prints a login URL. Open it on any machine and approve the device.
- **Known issue:** after approval, tcp_echo exits with `410 Gone` / `auth path not found`. The approval still
  worked, so run `./tcp_echo` again.
- **The node's keys are saved** in `tsrs_keys.json` in the current directory. Later runs reuse them and need no
  key or login.

**When it's up,** it logs `listening_addr=100.x.y.z:1234`. From another device on the tailnet:

```bash
nc 100.x.y.z 1234        # type a line and press Enter; it comes back
```

tcp_echo logs `accepted connection` when `nc` connects. For more detail, run it with
`RUST_LOG=info,ts_tunnel=debug,ts_runtime=debug`.

**Expected log noise:**
- **`tailscale ping` gets no reply** while traffic goes through DERP. tailscale-rs only answers pings that arrive
  over a direct UDP path (`ping is from non-udp peer, bail`). Use `nc` instead.
- **`No route to host (os error 265)`** when sending to a peer's IPv6 address is harmless: the QEMU NAT is
  IPv4-only.

### Testing the network monitor

Besides `netmon-smoke` (step 5), the tailscale-rs side has two tests that run on the target. Both need the
`wait-timeout` fork, because tailscale-rs examples and tests pull in `proptest`.

**ts_netmon's unit tests** (9 tests). The runner deploys and runs them. Delete the binary afterwards to save
space on `/data`:

```bash
cd ~/Projects/tailscale-rs
cargo +nightly-2026-11-09 test --release --target x86_64-pc-qnx -p ts_netmon --lib
```

**Live events with the `monitor` example.** Build and copy it:

```bash
cargo +nightly-2026-11-09 build --release --target x86_64-pc-qnx -p ts_netmon --example monitor
scp $SSHOPT target/x86_64-pc-qnx/release/examples/monitor root@$IP:/data/home/root/bin/
```

Run `./monitor` on the target. It prints the current interfaces and addresses. Then, from a second ssh
session:

```sh
ifconfig lo0 inet 10.99.0.1/32 alias      # monitor shows AddrUpsert(..., 10.99.0.1/32) within 10 s
ifconfig lo0 inet 10.99.0.1 -alias        # AddrRemoved within 10 s
```

**In tailscale-rs itself.** Run `tcp_echo` with `RUST_LOG=info,ts_runtime::netmon=debug`. The node's
endpoints in the admin console should include its LAN address (`192.168.122.x:<port>`). An alias added as
above should appear there too.

## How deployment works

`scripts/qnx-run.sh <binary> [args...]` does this:
1. strips the binary
2. copies it to `/data/home/root/bin/` on the target
3. runs it over ssh, passing the arguments through and returning its exit code

It finds the VM's IP with `scripts/qnx-target-ip.sh`, which reads the libvirt DHCP lease, because the IP
changes on every boot. It doesn't forward environment variables; to set any, run the binary over ssh yourself.

| Variable | Default |
|---|---|
| `QNX_HOST_IP` | looked up from `/var/lib/libvirt/dnsmasq/virbr0.status` |
| `QNX_USER` | `root` |
| `QNX_TARGET_DIR` | `~/qnxprojects/targets/qemu-qnx800-x86_64` |
| `QNX_REMOTE_DIR` | `/data/home/$QNX_USER/bin` |

ssh in by hand:

```bash
ssh -o UserKnownHostsFile=~/qnxprojects/targets/qemu-qnx800-x86_64/local/ssh-ident root@$(scripts/qnx-target-ip.sh)
```

## Troubleshooting

| Symptom | Fix |
|---|---|
| `linker 'qcc' not found` | `source ~/qnx800/qnxsdp-env.sh` in this shell |
| `` error[E0463]: can't find crate for `core` `` | `rust-src` is missing: `rustup component add rust-src --toolchain nightly-2026-11-09` |
| `failed to read .../qnx-crates/<crate>/Cargo.toml` | Run `scripts/setup-qnx-crates.sh` |
| `aws-lc-sys` build fails with `--include=` | The tailscale-rs patch isn't applied (step 6a) |
| `ssh: connect ... Connection refused` | sshd isn't running: apply step 4a and rebuild the image |
| `Permission denied (publickey,...)` | Rebuild the image with `--ssh-ident` (step 4b) |
| `no DHCP lease for ...` / `No route to host` | The VM hasn't finished booting (check `output/qemu.out`) or isn't running (`pgrep -a qemu-system`) |
| `No space left on device` during deploy | Clear `/data/home/root/bin` on the target, run `sync` (qnx6 frees space only then), and deploy release builds |
| tcp_echo exits with `410 Gone` after login | Run it again; the node was registered |
| `` cannot find value `SA_RESTART` `` in `wait-timeout` | The `wait-timeout` fork isn't in tailscale-rs's `[patch.crates-io]` (step 6b) |
