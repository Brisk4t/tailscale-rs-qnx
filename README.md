# tailscale-rs-QNX

[tailscale-rs](https://github.com/tailscale/tailscale-rs) on QNX OS 8.0, for x86_64 (`x86_64-pc-qnx`) and ARM64
(`aarch64-unknown-qnx`).

tailscale-rs is a library: a process embeds it and gets TCP/UDP sockets on a tailnet. This repo has the
patches and build setup that make it work on QNX, and `ts-forward`, a QNX program that joins a tailnet and
forwards TCP both ways between tailnet peers and local services such as sshd, a web server or qconn.

| Target | Architecture | Status |
|---|---|---|
| QEMU VM (mkqnximage) | x86_64 | Joins a tailnet; TCP and UDP to peers work |
| Raspberry Pi 5 | ARM64 | QNX boots; ts-forward runs and reaches the control server. The BSP-based image is new |
| Raspberry Pi 4 | ARM64 | Image builds; not yet tested on a board |

Plan, status and open issues: [PORTING_PLAN.md](PORTING_PLAN.md).

---

## Getting Started

### Prerequisites

- A Linux x86_64 host with `rustup`, `git`, `curl` and `unzip`.
- QNX SDP 8.0 installed at `~/qnx800`.
- For a **Raspberry Pi 4 or 5**: the board's BSP, installed into the SDP with QNX Software Center:
  ```bash
  ~/qnx/qnxsoftwarecenter/qnxsoftwarecenter_clt -installPackage com.qnx.qnx800.bsp.hw.raspberrypi_bcm2712_rpi5 -destination ~/qnx800   # Pi 5
  ~/qnx/qnxsoftwarecenter/qnxsoftwarecenter_clt -installPackage com.qnx.qnx800.bsp.hw.raspberrypi_bcm2711_rpi4 -destination ~/qnx800   # Pi 4
  ```
  plus [Raspberry Pi Imager](https://www.raspberrypi.com/software/) and a microSD card. A USB serial adapter or
  Raspberry Pi Debug Probe is useful for seeing the console, but not required.
- For the **QEMU VM**: `qemu-system-x86_64`, libvirt's default NAT network (`virbr0`), `allow virbr0` in
  `/etc/qemu/bridge.conf`, a setuid `qemu-bridge-helper`, and an ssh key at `~/.ssh/id_ed25519`.

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

### 3. Build ts-forward

```bash
source ~/qnx800/qnxsdp-env.sh
cargo build --release                                 # x86_64: target/x86_64-pc-qnx/release/ts-forward
cargo build --release --target aarch64-unknown-qnx    # ARM64:  target/aarch64-unknown-qnx/release/ts-forward
```

Build the one for your target, or both. The first build takes a few minutes.

### 4a. Raspberry Pi 4 or 5

Build the QNX boot image. This follows the board's BSP User's Guide: ts-forward is added to the BSP's
buildfile and the BSP's own `make` builds the image.

```bash
source ~/qnx800/qnxsdp-env.sh
IMAGE_TYPE=rasppi5 scripts/build-image.sh        # Pi 5; IMAGE_TYPE=rasppi for a Pi 4
```

The result is `build/image/rasppi5/bootfs/`: `ifs-rpi5.bin` (the QNX boot image), `config.txt` and a
`ts-forward/` folder. To make the SD card:

1. Write **Raspberry Pi OS** to it with Raspberry Pi Imager. That gives the card a FAT boot partition
   (`bootfs`) with the Pi's firmware and device trees.
2. Copy everything in `bootfs/` onto that partition, replacing its `config.txt`:
   ```bash
   cp -r build/image/rasppi5/bootfs/. /media/$USER/bootfs/
   ```
3. Optional: put an auth key in `ts-forward/authkey` on that partition, so the device joins without a login
   URL (or build with `TS_AUTH_KEY=...`, which puts it in `bootfs/` for you).

Put the card in the Pi, connect Ethernet and power it on. ts-forward starts at boot and the device appears in
your Tailscale admin console as `qnx-tailscale`. Without an auth key, the login URL to approve is in
`ts-forward/ts-forward.log` on the card (or on the serial console). Then, from any device on your tailnet:

```bash
ssh qnxuser@<tailnet IP of the Pi>
```

The BSP image's sshd doesn't allow root logins; `qnxuser` is its default user. QNX documents its default
password as `qnxuser` (and root's as `root`). These are development defaults built into the image's read-only
`/etc/shadow`, so they can't be changed on the device. With ssh forwarded to your tailnet, anyone your tailnet
policy lets reach the Pi can try them: restrict access to it in the policy, or leave ssh out of `args`.

The files in `ts-forward/` on the card control it, and can be edited from any computer:

| File | |
|---|---|
| `args` | ts-forward's forwards (`-p ssh` by default; see [Using ts-forward](#using-ts-forward)) |
| `hostname` | the device's name, also its tailnet name (`qnx-tailscale` by default) |
| `authkey` | optional auth key for the first registration |
| `keys.json` | the device's tailnet identity, created on first start and kept across reboots |
| `ts-forward.log` | the current boot's log (the previous one is `ts-forward.log.old`) |

### 4b. QEMU VM (x86_64)

**Create the VM** (once). This builds a QEMU image with the SDP's `mkqnximage`. The two `sshd-auth` lines are
needed by the SDP's OpenSSH; `--ssh-ident` installs your key for root.

```bash
source ~/qnx800/qnxsdp-env.sh
mkdir -p ~/qnxprojects/targets/qemu-qnx800-x86_64/local/snippets
cd ~/qnxprojects/targets/qemu-qnx800-x86_64
echo 'bin/sshd-auth=usr/libexec/sshd-auth' >> local/snippets/system_files.custom
echo '[type=link] /usr/libexec/sshd-auth=/system/bin/sshd-auth' >> local/snippets/ifs_files.custom
mkqnximage --type=qemu --arch=x86_64 --hostname=qnx-vm --ssh-ident=$HOME/.ssh/id_ed25519.pub
mkqnximage --run=-h                              # boot it, headless
grep 'Startup complete' output/qemu.out          # booted when this prints
```

**Deploy and run ts-forward**, from the repo root:

```bash
source ~/qnx800/qnxsdp-env.sh
IP=$(cd ~/qnxprojects/targets/qemu-qnx800-x86_64 && mkqnximage --getip)
SSHOPT="-o UserKnownHostsFile=$HOME/qnxprojects/targets/qemu-qnx800-x86_64/local/ssh-ident"
scp $SSHOPT target/x86_64-pc-qnx/release/ts-forward root@$IP:/data/home/root/bin/
ssh $SSHOPT root@$IP 'cd /data/home/root/bin && ./ts-forward'
```

It prints a login URL: open it and approve the device (or pass `-k tskey-auth-...`). When it logs `ready`, the
VM's sshd is reachable from any device on your tailnet as `ssh root@<tailnet IP>`.

**Or build a VM image with ts-forward installed** and started at boot:

```bash
scripts/build-image.sh                           # IMAGE_TYPE=qemu is the default
cd build/image/qemu && mkqnximage --run=-h
```

ts-forward's identity is then in `/data/var/ts-forward/keys.json` and its log in `/data/var/log/ts-forward.log`.
Rebuilding this image wipes its data partition, so the device needs approving again.

### Using ts-forward

```sh
ts-forward --list-presets                        # built-in forwards and groups
ts-forward -p qnx-dev -p web                     # inbound: ssh, qconn, http/https/8080/8443
ts-forward -f 2222=22 -f 8443=192.168.1.10:443   # inbound, custom: TAILNET_PORT=[HOST:]PORT
ts-forward --to 2201=laptop:22                   # outbound: 127.0.0.1:2201 -> peer `laptop`, port 22
ts-forward --help                                # everything else
```

- **Inbound** (`-p`, `-f`) makes local services reachable from the tailnet. With no options, it forwards ssh.
- **Outbound** (`--to [BIND:]LOCAL_PORT=PEER:PORT`) lets programs on the device reach a tailnet peer, by
  connecting to `127.0.0.1:LOCAL_PORT`. `PEER` is a tailnet IP or a peer's name.
- **Name and identity:** the device's tailnet name is its hostname (or `-H`). Its identity is kept in the key
  file (`-c`, default `ts-forward-keys.json`), so later runs don't need to log in again.
- **Access** is controlled by your tailnet policy.
- **qconn has no authentication.** Anyone who can reach it can run programs on the device as root, so only
  forward it on a tailnet whose policy restricts access to this device.

### Image settings

Environment variables for `scripts/build-image.sh`:

| Variable | Default | Meaning |
|---|---|---|
| `IMAGE_TYPE` | `qemu` | `rasppi5` (Pi 5), `rasppi` (Pi 4), `qemu`, or another `mkqnximage --type` such as `vmware` |
| `TS_FORWARD_ARGS` | `-p ssh` | ts-forward's forwards, e.g. `"-p qnx-dev -p web"`. Raspberry Pi: written to `bootfs/ts-forward/args` |
| `TS_AUTH_KEY` | none | joins the tailnet on first boot without the login URL. Raspberry Pi: written to `bootfs/ts-forward/authkey`; QEMU: stored in the image. Use a single-use key |
| `IMAGE_HOSTNAME` | `qnx-tailscale` | the QNX hostname, also the device's tailnet name. Raspberry Pi: written to `bootfs/ts-forward/hostname` |
| `SSH_IDENT` | `~/.ssh/id_ed25519.pub` | QEMU: public key for root ssh login |

---

## Development

### Repository layout

| Path | What it is |
|---|---|
| `apps/ts-forward` | The forwarder (`cargo build --release` builds it) |
| `image/raspberrypi/` | The Raspberry Pi image's ts-forward startup script and buildfile additions |
| `patches/<crate>-<version>.patch` | QNX patches for crates that don't know `target_os = "qnx"`: mio, socket2, tokio, getrandom 0.2/0.3/0.4, ring, signal-hook-registry, errno, rustix, wait-timeout |
| `patches/tailscale-rs/*.patch` | This port's changes to tailscale-rs, against `v0.7.1`: TLS via ring (`0001`), polling network monitor (`0002`) |
| `scripts/setup.sh` | Creates `build/`: the crate forks, the patched tailscale-rs checkout and its cargo config |
| `scripts/build-image.sh` | Builds a QNX image (Raspberry Pi 4/5, QEMU, ...) in `build/image/<type>/` with ts-forward installed and started at boot |
| `scripts/qnx-run.sh` | Cargo runner for the QEMU VM: copies a binary to it and runs it over ssh |
| `scripts/add-qnx-cfg.sh` | Adds `target_os = "qnx"` next to every `target_os = "nto"` cfg, for patching more crates |
| `smoke/*` | Smoke tests for the port itself (below) |
| `scripts/udp-echo-client.py` | Host-side client for `tokio-smoke serve` |
| `build/` | Git-ignored. `patched-crates/` and `tailscale-rs/` from `scripts/setup.sh`, `image/<type>/` from `scripts/build-image.sh` |

### How the setup works

`scripts/setup.sh` creates three things, all in `build/`:

1. **Crate forks** in `build/patched-crates/<crate>-<version>/`, one per `patches/<crate>-<version>.patch`. Each
   fork is a git repo: one commit with the pristine crates.io source and the QNX patch left uncommitted, so
   `git diff` in a fork shows exactly the change. This repo's `Cargo.toml` uses them through
   `[patch.crates-io]`. PORTING_PLAN.md explains why each crate needs a patch. None of the patches is
   architecture-specific.
2. **tailscale-rs** cloned at `v0.7.1` into `build/tailscale-rs`, on a local branch `v0.7.1-qnx`, with
   `patches/tailscale-rs/*.patch` applied in order, uncommitted. Upstream has no QNX branch; the patches are
   the whole change. `ts-forward` and `netmon-smoke` depend on crates in this checkout.
3. **tailscale-rs's `.cargo/config.toml`:** the QNX target settings for both architectures, plus a
   `[patch.crates-io]` entry for every fork, with absolute paths. It's excluded from tailscale-rs's git and only
   applies with `--target x86_64-pc-qnx` or `aarch64-unknown-qnx`, so host builds there are unaffected.

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

- `x86_64-pc-qnx` and `aarch64-unknown-qnx` are tier-3 Rust targets with no prebuilt std, so std is built from
  source (`-Zbuild-std`) with the nightly pinned in `rust-toolchain.toml`.
- `.cargo/config.toml` configures both: `qcc` as linker, build-std, and the `CC`/`CXX`/`AR` settings for C
  dependencies (`-Vgcc_ntox86_64` / `-Vgcc_ntoaarch64le`). `x86_64-pc-qnx` is the default target, with
  `scripts/qnx-run.sh` as its runner; pass `--target aarch64-unknown-qnx` for ARM64.
- Every shell that builds for QNX needs `source ~/qnx800/qnxsdp-env.sh`, which puts `qcc` on `PATH`.
- `cargo build --release` builds `apps/` only; add `--workspace` to include the smoke tests. Release builds use
  `panic=abort`.
- Cargo prints `warning: patch ... was not used in the crate graph` for every fork. It's spurious with
  `-Zbuild-std`: `cargo tree --target x86_64-pc-qnx -i mio` shows the fork is used.
- Deploy release builds. Debug builds of tailscale-rs programs are about 42 MB even stripped, and the VM's
  `/data` is 61 MB.

### Deployment runner (QEMU VM)

`cargo run` for `x86_64-pc-qnx` uses `scripts/qnx-run.sh <binary> [args...]`, which:
1. strips the binary
2. copies it to `/data/home/root/bin/` on the VM
3. runs it over ssh, passing the arguments through and returning its exit code

It finds the VM's IP with `mkqnximage --getip` in the target folder (the IP changes on every boot), so it needs
the SDP environment sourced. It doesn't forward environment variables; to set any, run the binary over ssh
yourself. ARM64 has no runner: copy binaries to the board (e.g. with `scp` to its LAN address) and run them
there.

| Variable | Default |
|---|---|
| `QNX_HOST_IP` | looked up with `mkqnximage --getip` in `QNX_TARGET_DIR` |
| `QNX_USER` | `root` |
| `QNX_TARGET_DIR` | `~/qnxprojects/targets/qemu-qnx800-x86_64` |
| `QNX_REMOTE_DIR` | `/data/home/$QNX_USER/bin` |

### Smoke tests

The smoke tests check the port itself: the toolchain, std and the crate forks on QNX. Run them after changing
the pinned nightly, a crate fork or the QNX SDP, or on new hardware. On the VM, with it running:

```bash
source ~/qnx800/qnxsdp-env.sh
cargo run -p std-smoke       # 13 checks: threads, sync, time, fs, env, panics, TCP/UDP v4+v6, DNS, process, internet
cargo run -p tokio-smoke     # 5 checks: timers, tasks, UDP echo, 1 MiB TCP echo, tokio::signal
cargo run -p random-smoke    # 4 checks: getrandom 0.2/0.3/0.4, ring
cargo run -p rustix-smoke    # 5 checks: gethostname and uname against the target's own commands
cargo run -p netmon-smoke    # 5 checks: ts_netmon's PollingMon against ifconfig, plus change detection
```

For ARM64, build them with `cargo build --release --target aarch64-unknown-qnx -p <name>` and run the binaries
from `target/aarch64-unknown-qnx/release/` on the board.

Each exits non-zero if a required check fails; add `--release` to test release builds. `std-smoke`'s
`net/internet` check is optional (`WARN` without outbound network). `netmon-smoke`'s change-detection checks add
and remove `10.99.0.1/32` on `lo0`, so they only run on QNX, as root.

**ring on QNX vs the host** should produce identical bytes:

```bash
cargo run -q --release --target x86_64-unknown-linux-gnu -p random-smoke -- ring-vectors > /tmp/host.txt
cargo run -q --release -p random-smoke -- ring-vectors > /tmp/qnx.txt
diff /tmp/host.txt /tmp/qnx.txt && echo identical
```

**UDP over the real network card** (host to VM; the client needs `python3`):

```bash
IP=$(cd ~/qnxprojects/targets/qemu-qnx800-x86_64 && mkqnximage --getip)
SSH="ssh -o UserKnownHostsFile=$HOME/qnxprojects/targets/qemu-qnx800-x86_64/local/ssh-ident root@$IP"
cargo run -p tokio-smoke                                  # deploys the current build
$SSH 'cd bin && (./tokio-smoke serve 0.0.0.0:9999 > /tmp/serve.log 2>&1 &)'
python3 scripts/udp-echo-client.py $IP 9999 2000          # -> 2000/2000 echoed ..., rtt p50=...
$SSH 'slay -f tokio-smoke'
```

`tokio-smoke` reads `UDP_CLIENTS` (default 32), `UDP_ROUNDS` (100) and `UDP_RCVBUF` (1 MiB, the server's
`SO_RCVBUF`) on the target.

### tailscale-rs examples and tests

Built inside `build/tailscale-rs`. tailscale-rs pins stable Rust, so pass the nightly and the target
(`x86_64-pc-qnx` or `aarch64-unknown-qnx`):

```bash
source ~/qnx800/qnxsdp-env.sh
cd build/tailscale-rs
cargo +nightly-2026-11-09 build --release --target x86_64-pc-qnx --example tcp_echo --example peer_ping
```

The binaries are in `build/tailscale-rs/target/<target>/release/examples/`. The build rewrites tailscale-rs's
`Cargo.lock` to point at the forks.

- **`tcp_echo`:** run `./tcp_echo` on the target and approve the login URL. It logs
  `listening_addr=100.x.y.z:1234`; from another tailnet device, `nc 100.x.y.z 1234` echoes each line back.
- **`peer_ping`:** `./peer_ping -c peer_ping_keys.json -p <peer tailnet IP>:5678` sends `hello` over UDP once a
  second; receive it on the peer with `nc -u -l 5678`.
- More logging: `RUST_LOG=info,ts_tunnel=debug,ts_runtime=debug`.

**Network monitor.** ts_netmon's unit tests run on the VM through the runner, and the `monitor` example
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
2. Run `scripts/add-qnx-cfg.sh` on its files that mention `target_os = "nto"`, and review the
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
| `error: no Raspberry Pi rpi5 BSP in .../bsp` | Install the board's BSP (Getting Started, Prerequisites) |
| Pi boots Raspberry Pi OS instead of QNX | `config.txt` on the card wasn't replaced: copy `bootfs/config.txt` over it |
| Pi: device registers as `node` | It registered before `hostname` was set: rename it in the admin console, or delete `ts-forward/keys.json` on the card to register again |
| Pi: no screen output | Expected: QNX's console is the serial UART (on the Pi 5, the 3-pin connector between the micro-HDMI ports; 115200 8N1) |
| VM: sshd fails at boot with `/usr/libexec/sshd-auth does not exist`, or `ssh: connect ... Connection refused` | The `sshd-auth` lines from step 4b are missing: add them to `local/snippets/` and run `mkqnximage --build` (wipes the data partition) |
| VM: `Permission denied (publickey,...)` | The image was built without your key: rerun `mkqnximage --ssh-ident=$HOME/.ssh/id_ed25519.pub` in the target folder |
| `qnx-run: can't find the target's IP` / `No route to host` | The VM hasn't finished booting (check `output/qemu.out`) or isn't running (`pgrep -a qemu-system`). The runner also needs `source ~/qnx800/qnxsdp-env.sh` |
| `No space left on device` during deploy | Clear `/data/home/root/bin` on the VM, run `sync` (qnx6 frees space only then), and deploy release builds |
| Exits with `410 Gone` / `auth path not found` right after the login URL is approved | Known tailscale-rs issue. The approval worked: run it again (the images restart it automatically) |
| `tailscale ping` from a peer gets no reply | Expected while traffic goes through DERP: tailscale-rs only answers pings that arrive over a direct path. Test with a TCP connection instead |
| `No route to host (os error 265)` sending to a peer's IPv6 address | Harmless when the network is IPv4-only, as the QEMU NAT is |
