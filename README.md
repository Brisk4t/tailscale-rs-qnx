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
| `apps/ts-forward` | Exposes local TCP services (ssh, https, qconn, ...) on the tailnet, and lets local programs reach tailnet peers; see step 7 |
| `patches/<crate>-<version>.patch` | QNX patches for mio, socket2, tokio, getrandom ×3, ring, signal-hook-registry, errno, rustix, wait-timeout |
| `patches/tailscale-rs/*.patch` | This port's changes to tailscale-rs itself, against `v0.7.1`: TLS via ring, polling network monitor |
| `scripts/setup.sh` | Creates the crate forks, clones and patches tailscale-rs, and writes its QNX cargo config (step 2) |
| `scripts/qnx-run.sh` | Cargo runner: copies a binary to the QNX target and runs it over ssh |
| `scripts/qnx-target-ip.sh` | Prints the QEMU target's current IP |
| `scripts/udp-echo-client.py` | Host-side client for `tokio-smoke serve` |
| `scripts/run-mio-tests.sh` | Builds mio's test suite for QNX and runs it on the target (results: PORTING_PLAN.md open issue 8) |
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
│   └── qnx-crates/         patched crate forks (created by step 2)
└── tailscale-rs/           upstream checkout at v0.7.1, QNX patch applied (created by step 2)
```

This repo's `Cargo.toml` finds the forks by relative path; tailscale-rs's generated config uses absolute paths.

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

## 2. Set up the crate forks and tailscale-rs

One script sets up everything outside this repo:

```bash
scripts/setup.sh
```

1. **Crate forks** in `~/Projects/QNX/qnx-crates/<crate>-<version>/`, for the crates that don't know
   `target_os = "qnx"` yet (`patches/<crate>-<version>.patch`). Each fork is a git repo: one commit with the
   pristine crates.io source, and the QNX patch left uncommitted, so `git diff` in a fork shows exactly the
   change. PORTING_PLAN.md explains why each crate needs a patch.
2. **tailscale-rs** cloned at `v0.7.1` into `~/Projects/tailscale-rs`, on a local branch `v0.7.1-qnx`, with this
   port's patches applied in order, uncommitted. Upstream has no QNX branch; the patches are the whole change:
   - `0001-tls-ring-on-qnx.patch`: switches TLS from aws-lc-rs (which doesn't build with qcc) to ring on QNX.
     Host builds still use aws-lc-rs.
   - `0002-polling-netmon.patch`: adds a polling network monitor (`ts_netmon::PollingMon`), used on QNX and other
     Unixes with no dedicated backend.
3. **tailscale-rs's local `.cargo/config.toml`:** the QNX target settings, plus a `[patch.crates-io]` entry
   for every fork, with absolute paths. It's excluded from git, so upstream files stay untouched, and it only
   applies with `--target x86_64-pc-qnx`, so host builds and tests in tailscale-rs work as before.

This repo's `netmon-smoke` and `ts-forward` build tailscale-rs crates from that checkout, so run the script
before building anything.

The script is safe to rerun:
- existing forks and an existing checkout are left alone
- a patch that's already applied (or committed) is detected
- a `.cargo/config.toml` it didn't write is never overwritten; it writes `config.toml.setup-new` next to it instead

To use other locations, set `QNX_CRATES_DIR`, `TAILSCALE_RS_DIR` or `TAILSCALE_RS_REF`. If you move
`qnx-crates`, also update the relative paths in this repo's `Cargo.toml`.

## 3. Build

Every shell that builds for QNX needs `source ~/qnx800/qnxsdp-env.sh` first. Cargo prints
`warning: patch ... was not used in the crate graph` for the crate forks in both repos: it's spurious with
`-Zbuild-std` (`cargo tree --target x86_64-pc-qnx -i mio` shows the fork is used).

**a. This repo** (smoke tests and `ts-forward`). The target, linker and nightly come from this repo's
`rust-toolchain.toml` and `.cargo/config.toml`, so plain `cargo` works:

```bash
source ~/qnx800/qnxsdp-env.sh
cd ~/Projects/QNX/tailscale-QNX
cargo build --workspace --release    # release (panic=abort); drop --release for debug
```

The first build compiles std itself and takes a few minutes. Binaries go to
`target/x86_64-pc-qnx/{debug,release}/`.

**b. tailscale-rs examples.** tailscale-rs pins stable Rust, so pass the nightly and the target explicitly:

```bash
source ~/qnx800/qnxsdp-env.sh
cd ~/Projects/tailscale-rs
cargo +nightly-2026-11-09 build --release --target x86_64-pc-qnx --example tcp_echo --example peer_ping
```

- **Output:** `target/x86_64-pc-qnx/release/examples/{tcp_echo,peer_ping}`. The release profile uses LTO and
  strips the binaries, so the build takes a few minutes.
- **Cargo.lock changes:** the build rewrites tailscale-rs's `Cargo.lock` to point at the forks. Don't commit
  those changes.

**Which to use:** release builds for anything deployed to the target. Debug builds of the tailscale-rs examples
are about 42 MB even stripped, and the VM's `/data` is 61 MB.

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
cargo run -p netmon-smoke          # 5 checks, 0 failures
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

## 6. Join a tailnet from QNX

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

## 7. Expose local services with ts-forward

`ts-forward` joins the tailnet as its own device and forwards TCP connections both ways, for programs that
don't embed tailscale-rs:
- **Inbound:** tailnet ports to local services, so sshd, a web server or qconn become reachable from the tailnet.
- **Outbound:** local ports to tailnet peers, so local programs can reach peers. They can't reach `100.x`
  addresses directly, because the tailnet only exists inside ts-forward.

Build it with this repo's workspace (step 3a, or `cargo build --release -p ts-forward`) and deploy it:

```bash
IP=$(scripts/qnx-target-ip.sh)
SSHOPT="-o UserKnownHostsFile=$HOME/qnxprojects/targets/qemu-qnx800-x86_64/local/ssh-ident"
scp $SSHOPT target/x86_64-pc-qnx/release/ts-forward root@$IP:/data/home/root/bin/
```

On the target:

```sh
cd /data/home/root/bin
./ts-forward --list-presets              # built-in forwards and groups
./ts-forward                             # default: ssh (tailnet :22 -> 127.0.0.1:22)
./ts-forward -p qnx-dev -p web           # ssh + qconn + http/https/8080/8443
./ts-forward -f 2222=22 -f 8443=192.168.1.10:443   # custom: TAILNET_PORT=[HOST:]PORT
./ts-forward --to 2201=laptop:22         # outbound only: 127.0.0.1:2201 -> peer `laptop`, port 22
ssh -p 2201 user@127.0.0.1               # ...then, on the target, reach the peer's sshd through it
```

- **Outbound specs:** `[BIND:]LOCAL_PORT=PEER:PORT`. `PEER` is a tailnet IP or a peer's name as shown in the
  admin console, looked up in the network map for each connection. `BIND` defaults to `127.0.0.1`; use
  `0.0.0.0:2201=...` only if other hosts on the device's LAN should be able to use the forward.
- **Defaults:** with only `--to` given, no inbound forwards are opened. Add `-p ssh` to get both.
- **Identity:** its own key file, `ts-forward-keys.json` (`-c` to change). The first run prints a login URL, or
  takes `-k`/`TS_AUTH_KEY`. The device name defaults to the OS hostname (`-H` to change).
- **Access control:** your tailnet policy decides which peers can connect. The local service sees connections
  from `127.0.0.1`. ts-forward logs each connection with the peer's tailnet address and name.
- **`qconn` has no authentication:** anyone allowed to reach it can run programs on the target as root.
  Only enable it on a tailnet whose policy restricts access to this device.
- **TCP only.** Multi-connection protocols like FTP don't work through it.
- **Limits:** `--max-connections` (default 64) caps simultaneous connections in both directions; `--connect-timeout`
  (default 10 s) bounds each local or tailnet connect.

From another device on the tailnet: `ssh root@<tailnet IP>`. sshd on the target only accepts the keys
installed with `--ssh-ident` (step 4b).

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
| `failed to read .../qnx-crates/<crate>/Cargo.toml` or `.../tailscale-rs/...` | Run `scripts/setup.sh` |
| `aws-lc-sys` build fails with `--include=` | The tailscale-rs patch isn't applied: run `scripts/setup.sh` |
| `ssh: connect ... Connection refused` | sshd isn't running: apply step 4a and rebuild the image |
| `Permission denied (publickey,...)` | Rebuild the image with `--ssh-ident` (step 4b) |
| `no DHCP lease for ...` / `No route to host` | The VM hasn't finished booting (check `output/qemu.out`) or isn't running (`pgrep -a qemu-system`) |
| `No space left on device` during deploy | Clear `/data/home/root/bin` on the target, run `sync` (qnx6 frees space only then), and deploy release builds |
| tcp_echo exits with `410 Gone` after login | Run it again; the node was registered |
| `` cannot find value `SA_RESTART` `` in `wait-timeout` | tailscale-rs's `.cargo/config.toml` is missing the `wait-timeout` fork: rerun `scripts/setup.sh` (or merge its `config.toml.setup-new`) |
