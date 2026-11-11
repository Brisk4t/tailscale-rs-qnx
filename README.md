# tailscale-QNX

[tailscale-rs](https://github.com/tailscale/tailscale-rs) on QNX OS 8.0 (`x86_64-pc-qnx`).

tailscale-rs is a library: a process embeds it and gets TCP/UDP sockets on a tailnet. This repo has the
patches and build setup that make it work on QNX, and `ts-forward`, a QNX program that joins a tailnet and
forwards TCP both ways between tailnet peers and local services such as sshd, a web server or qconn.

Plan, status and open issues: [PORTING_PLAN.md](PORTING_PLAN.md).

---

## Getting Started

### Prerequisites

- Linux x86_64 host with `rustup`, `git`, `curl` and `python3`, and an ssh key at `~/.ssh/id_ed25519`
  (`ssh-keygen -t ed25519` if you don't have one).
- QNX SDP 8.0 installed at `~/qnx800`.
- QEMU and libvirt, for the QNX VM: `qemu-system-x86_64`, libvirt's default NAT network (`virbr0`),
  `allow virbr0` in `/etc/qemu/bridge.conf` and a setuid `qemu-bridge-helper`.

### 1. Install the toolchain

```bash
rustup toolchain install nightly-2026-11-09 --component rust-src --profile minimal
```

### 2. Set up the dependencies

From the repo root:

```bash
scripts/setup.sh
```

This creates `build/` (git-ignored) with the patched crates and a patched tailscale-rs checkout.

### 3. Build

```bash
source ~/qnx800/qnxsdp-env.sh
cargo build --release
```

The binary is `target/x86_64-pc-qnx/release/ts-forward`. The first build takes a few minutes.

### 4. Create the QNX VM

Once. This builds a QEMU image with the SDP's `mkqnximage`, in the folder the deploy scripts expect. The two
`sshd-auth` lines are needed by the SDP's OpenSSH; `--ssh-ident` installs your key for root.

```bash
source ~/qnx800/qnxsdp-env.sh
mkdir -p ~/qnxprojects/targets/qemu-qnx800-x86_64/local/snippets
cd ~/qnxprojects/targets/qemu-qnx800-x86_64
echo 'bin/sshd-auth=usr/libexec/sshd-auth' >> local/snippets/system_files.custom
echo '[type=link] /usr/libexec/sshd-auth=/system/bin/sshd-auth' >> local/snippets/ifs_files.custom
mkqnximage --type=qemu --arch=x86_64 --hostname=qnx-vm --ssh-ident=$HOME/.ssh/id_ed25519.pub
```

Start it, and wait until `Startup complete` appears:

```bash
mkqnximage --run=-h
grep 'Startup complete' output/qemu.out
```

### 5. Deploy and run ts-forward

From the repo root:

```bash
IP=$(scripts/qnx-target-ip.sh)
SSHOPT="-o UserKnownHostsFile=$HOME/qnxprojects/targets/qemu-qnx800-x86_64/local/ssh-ident"
scp $SSHOPT target/x86_64-pc-qnx/release/ts-forward root@$IP:/data/home/root/bin/
ssh $SSHOPT root@$IP
```

On the target:

```sh
cd /data/home/root/bin
./ts-forward
```

It prints a login URL. Open it in a browser and approve the device (or pass an auth key with
`-k tskey-auth-...`). When it logs `ready`, the target's sshd is reachable from any device on your tailnet:

```bash
ssh root@<tailnet IP of the target>
```

### Using ts-forward

```sh
./ts-forward --list-presets                        # built-in forwards and groups
./ts-forward -p qnx-dev -p web                     # inbound: ssh, qconn, http/https/8080/8443
./ts-forward -f 2222=22 -f 8443=192.168.1.10:443   # inbound, custom: TAILNET_PORT=[HOST:]PORT
./ts-forward --to 2201=laptop:22                   # outbound: 127.0.0.1:2201 -> peer `laptop`, port 22
./ts-forward --help                                # everything else
```

- **Inbound** (`-p`, `-f`) makes local services reachable from the tailnet. With no options, it forwards ssh.
- **Outbound** (`--to [BIND:]LOCAL_PORT=PEER:PORT`) lets programs on the target reach a tailnet peer, by
  connecting to `127.0.0.1:LOCAL_PORT`. `PEER` is a tailnet IP or a peer's name.
- **Access** is controlled by your tailnet policy. The device's identity is kept in `ts-forward-keys.json`,
  so later runs don't need to log in again.
- **qconn has no authentication.** Anyone who can reach it can run programs on the target as root, so only
  forward it on a tailnet whose policy restricts access to this device.

---

## Development

### Repository layout

| Path | What it is |
|---|---|
| `apps/ts-forward` | The forwarder (`cargo build --release` builds it) |
| `patches/<crate>-<version>.patch` | QNX patches for crates that don't know `target_os = "qnx"`: mio, socket2, tokio, getrandom 0.2/0.3/0.4, ring, signal-hook-registry, errno, rustix, wait-timeout |
| `patches/tailscale-rs/*.patch` | This port's changes to tailscale-rs, against `v0.7.1`: TLS via ring (`0001`), polling network monitor (`0002`) |
| `scripts/setup.sh` | Creates `build/`: the crate forks, the patched tailscale-rs checkout and its cargo config |
| `scripts/qnx-run.sh` | Cargo runner: copies a binary to the target and runs it over ssh |
| `scripts/qnx-target-ip.sh` | Prints the QEMU VM's current IP |
| `scripts/add-qnx-cfg.py` | Adds `target_os = "qnx"` next to every `target_os = "nto"` cfg, for patching more crates |
| `smoke/*` | Smoke tests for the port itself (below) |
| `scripts/udp-echo-client.py` | Host-side client for `tokio-smoke serve` |
| `build/` | Created by `scripts/setup.sh`, git-ignored: `patched-crates/` and `tailscale-rs/` |

### How the setup works

`scripts/setup.sh` creates three things, all in `build/`:

1. **Crate forks** in `build/patched-crates/<crate>-<version>/`, one per `patches/<crate>-<version>.patch`. Each
   fork is a git repo: one commit with the pristine crates.io source and the QNX patch left uncommitted, so
   `git diff` in a fork shows exactly the change. This repo's `Cargo.toml` uses them through
   `[patch.crates-io]`. PORTING_PLAN.md explains why each crate needs a patch.
2. **tailscale-rs** cloned at `v0.7.1` into `build/tailscale-rs`, on a local branch `v0.7.1-qnx`, with
   `patches/tailscale-rs/*.patch` applied in order, uncommitted. Upstream has no QNX branch; the patches are
   the whole change. `ts-forward` and `netmon-smoke` depend on crates in this checkout.
3. **tailscale-rs's `.cargo/config.toml`:** the QNX target settings plus a `[patch.crates-io]` entry for every
   fork, with absolute paths. It's excluded from tailscale-rs's git and only applies with
   `--target x86_64-pc-qnx`, so host builds there are unaffected.

It's safe to rerun: existing forks and the checkout are left alone, applied patches are detected, and a cargo
config it didn't write is never overwritten (it writes `config.toml.setup-new` next to it instead). To start
over, delete `build/` and rerun. `TAILSCALE_RS_REF` selects another tailscale-rs tag, if the patches still apply.

To update the tailscale-rs patches after changing the checkout:

```bash
cd build/tailscale-rs
git diff v0.7.1 -- crates/utils/ts_tls_util > ../../patches/tailscale-rs/0001-tls-ring-on-qnx.patch
git diff v0.7.1 -- crates/net/underlay/ts_netmon > ../../patches/tailscale-rs/0002-polling-netmon.patch
```

### Build notes

- `x86_64-pc-qnx` is a tier-3 Rust target with no prebuilt std, so std is built from source (`-Zbuild-std`)
  with the nightly pinned in `rust-toolchain.toml`. `.cargo/config.toml` sets the target, `qcc` as linker,
  build-std, the `CC`/`CXX`/`AR` settings for C dependencies, and `scripts/qnx-run.sh` as the runner.
- Every shell that builds for QNX needs `source ~/qnx800/qnxsdp-env.sh`, which puts `qcc` on `PATH`.
- `cargo build --release` builds `apps/` only; add `--workspace` to include the smoke tests. Release builds use
  `panic=abort`.
- Cargo prints `warning: patch ... was not used in the crate graph` for every fork. It's spurious with
  `-Zbuild-std`: `cargo tree --target x86_64-pc-qnx -i mio` shows the fork is used.
- Deploy release builds. Debug builds of tailscale-rs programs are about 42 MB even stripped, and the VM's
  `/data` is 61 MB.

### Deployment runner

`cargo run` uses `scripts/qnx-run.sh <binary> [args...]`, which:
1. strips the binary
2. copies it to `/data/home/root/bin/` on the target
3. runs it over ssh, passing the arguments through and returning its exit code

It finds the VM's IP with `scripts/qnx-target-ip.sh`, from libvirt's DHCP leases (the IP changes on every boot).
It doesn't forward environment variables; to set any, run the binary over ssh yourself.

| Variable | Default |
|---|---|
| `QNX_HOST_IP` | looked up from `/var/lib/libvirt/dnsmasq/virbr0.status` |
| `QNX_USER` | `root` |
| `QNX_TARGET_DIR` | `~/qnxprojects/targets/qemu-qnx800-x86_64` |
| `QNX_REMOTE_DIR` | `/data/home/$QNX_USER/bin` |

### Smoke tests

The smoke tests check the port itself: the toolchain, std and the crate forks on QNX. Run them after changing
the pinned nightly, a crate fork or the QNX SDP, or on new hardware. With the VM running:

```bash
source ~/qnx800/qnxsdp-env.sh
cargo run -p std-smoke       # 13 checks: threads, sync, time, fs, env, panics, TCP/UDP v4+v6, DNS, process, internet
cargo run -p tokio-smoke     # 5 checks: timers, tasks, UDP echo, 1 MiB TCP echo, tokio::signal
cargo run -p random-smoke    # 4 checks: getrandom 0.2/0.3/0.4, ring
cargo run -p rustix-smoke    # 5 checks: gethostname and uname against the target's own commands
cargo run -p netmon-smoke    # 5 checks: ts_netmon's PollingMon against ifconfig, plus change detection
```

Each exits non-zero if a required check fails; add `--release` to test release builds. `std-smoke`'s
`net/internet` check is optional (`WARN` without outbound network). `netmon-smoke`'s change-detection checks add
and remove `10.99.0.1/32` on `lo0`, so they only run on QNX, as root.

**ring on QNX vs the host** should produce identical bytes:

```bash
cargo run -q --release --target x86_64-unknown-linux-gnu -p random-smoke -- ring-vectors > /tmp/host.txt
cargo run -q --release -p random-smoke -- ring-vectors > /tmp/qnx.txt
diff /tmp/host.txt /tmp/qnx.txt && echo identical
```

**UDP over the real network card** (host to target):

```bash
IP=$(scripts/qnx-target-ip.sh)
SSH="ssh -o UserKnownHostsFile=$HOME/qnxprojects/targets/qemu-qnx800-x86_64/local/ssh-ident root@$IP"
cargo run -p tokio-smoke                                  # deploys the current build
$SSH 'cd bin && (./tokio-smoke serve 0.0.0.0:9999 > /tmp/serve.log 2>&1 &)'
python3 scripts/udp-echo-client.py $IP 9999 2000          # -> 2000/2000 echoed ..., rtt p50=...
$SSH 'slay -f tokio-smoke'
```

`tokio-smoke` reads `UDP_CLIENTS` (default 32), `UDP_ROUNDS` (100) and `UDP_RCVBUF` (1 MiB, the server's
`SO_RCVBUF`) on the target.

### tailscale-rs examples and tests

Built inside `build/tailscale-rs`. tailscale-rs pins stable Rust, so pass the nightly and the target:

```bash
source ~/qnx800/qnxsdp-env.sh
cd build/tailscale-rs
cargo +nightly-2026-11-09 build --release --target x86_64-pc-qnx --example tcp_echo --example peer_ping
```

The binaries are in `build/tailscale-rs/target/x86_64-pc-qnx/release/examples/`. The build rewrites
tailscale-rs's `Cargo.lock` to point at the forks.

- **`tcp_echo`:** run `./tcp_echo` on the target and approve the login URL. It logs
  `listening_addr=100.x.y.z:1234`; from another tailnet device, `nc 100.x.y.z 1234` echoes each line back.
- **`peer_ping`:** `./peer_ping -c peer_ping_keys.json -p <peer tailnet IP>:5678` sends `hello` over UDP once a
  second; receive it on the peer with `nc -u -l 5678`.
- More logging: `RUST_LOG=info,ts_tunnel=debug,ts_runtime=debug`.

**Network monitor.** ts_netmon's unit tests run on the target through the runner, and the `monitor` example
prints interface and address events live:

```bash
cd build/tailscale-rs
cargo +nightly-2026-11-09 test --release --target x86_64-pc-qnx -p ts_netmon --lib
cargo +nightly-2026-11-09 build --release --target x86_64-pc-qnx -p ts_netmon --example monitor
```

With `./monitor` running on the target, `ifconfig lo0 inet 10.99.0.1/32 alias` from a second ssh session shows
up as `AddrUpsert` within 10 s, and `ifconfig lo0 inet 10.99.0.1 -alias` as `AddrRemoved`.

### Patching another crate

If another crate fails to build for `target_os = "qnx"`:
1. Add a fork in `build/patched-crates/<crate>-<version>/`: pristine crates.io source committed, as
   `scripts/setup.sh` does.
2. Run `scripts/add-qnx-cfg.py` on its files that mention `target_os = "nto"`, and review the
   `all(target_os = "nto", ...)` sites it flags. Check its `Cargo.toml` target tables too.
3. Save `git diff` as `patches/<crate>-<version>.patch`, and add the fork to this repo's `Cargo.toml`
   `[patch.crates-io]`. Rerun `scripts/setup.sh` to update tailscale-rs's config.

### QNX VS Code extension

A target created by the extension works the same way. Its `mkqnximage-wrapper.sh` just sources the SDP
environment and runs `mkqnximage`, so `./mkqnximage-wrapper.sh --run=-h` is equivalent to the commands above.
If the target was created without the `sshd-auth` lines, add them to its `local/snippets/` and rebuild it with
`mkqnximage --build` (this wipes its data partition).

The extension keeps its target list in your local VS Code user settings. To use the existing VM from another
machine, add it to `qnx.targets.list` with `"vmtype": "qemu"` and
`"path": "/home/<you>/qnxprojects/targets/qemu-qnx800-x86_64"`. Don't use "Create QNX Virtual Target" for it:
that rebuilds the image.

---

## Troubleshooting

| Symptom | Fix |
|---|---|
| `linker 'qcc' not found` | `source ~/qnx800/qnxsdp-env.sh` in this shell |
| `` error[E0463]: can't find crate for `core` `` | `rust-src` is missing: `rustup component add rust-src --toolchain nightly-2026-11-09` |
| `failed to read .../build/patched-crates/...` or `.../build/tailscale-rs/...` | Run `scripts/setup.sh` |
| `aws-lc-sys` build fails with `--include=` | The tailscale-rs patches aren't applied: run `scripts/setup.sh` |
| `` cannot find value `SA_RESTART` `` in `wait-timeout` | tailscale-rs's `.cargo/config.toml` is missing the `wait-timeout` fork: rerun `scripts/setup.sh`, or merge its `config.toml.setup-new` |
| sshd fails at boot with `/usr/libexec/sshd-auth does not exist`, or `ssh: connect ... Connection refused` | The `sshd-auth` lines from Getting Started step 4 are missing: add them to `local/snippets/` and run `mkqnximage --build` (wipes the data partition) |
| `Permission denied (publickey,...)` | The image was built without your key: rerun `mkqnximage --ssh-ident=$HOME/.ssh/id_ed25519.pub` in the target folder |
| `no DHCP lease for ...` / `No route to host` | The VM hasn't finished booting (check `output/qemu.out`) or isn't running (`pgrep -a qemu-system`) |
| `No space left on device` during deploy | Clear `/data/home/root/bin` on the target, run `sync` (qnx6 frees space only then), and deploy release builds |
| Exits with `410 Gone` / `auth path not found` right after the login URL is approved | Known tailscale-rs issue. The approval worked: run it again |
| `tailscale ping` from a peer gets no reply | Expected while traffic goes through DERP: tailscale-rs only answers pings that arrive over a direct path. Test with a TCP connection instead |
| `No route to host (os error 265)` sending to a peer's IPv6 address | Harmless: the QEMU NAT network is IPv4-only |
