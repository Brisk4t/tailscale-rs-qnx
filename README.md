# tailscale-QNX

Porting [tailscale-rs](https://github.com/tailscale/tailscale-rs) to QNX OS 8.0 (`x86_64-pc-qnx`).
See [PORTING_PLAN.md](PORTING_PLAN.md) for the plan, status and findings.

What's here so far (Phase 0):

| Path | What it is |
|---|---|
| `smoke/hello` | Hello world for the QNX target |
| `smoke/std-smoke` | std checks: threads, sync, time, fs, env, panics, TCP/UDP v4+v6, DNS, process, internet |
| `smoke/tokio-smoke` | tokio checks: timers, tasks, UDP echo (32 clients), 1 MiB TCP echo; also a `serve` mode |
| `patches/` | QNX patches for `mio` 1.2.4, `socket2` 0.6.5, `tokio` 1.53.3 (the versions tailscale-rs locks) |
| `scripts/setup-qnx-crates.sh` | Downloads those crates from crates.io and applies the patches |
| `scripts/qnx-run.sh` | Cargo runner: copies a binary to the QNX target and runs it over ssh |
| `scripts/qnx-target-ip.sh` | Prints the QEMU target's current IP (from the libvirt DHCP lease) |
| `scripts/udp-echo-client.py` | Host-side client for `tokio-smoke serve` |
| `scripts/add-qnx-cfg.py` | Adds `target_os = "qnx"` next to every `target_os = "nto"` cfg (for patching more crates) |

## Prerequisites

- Linux x86_64 host with `rustup`, `git`, `curl`, `python3`.
- **QNX SDP 8.0** installed at `~/qnx800` (provides `qcc`, `ntox86_64-*` binutils, `mkqnximage`).
- **QEMU + libvirt** for the target VM: `qemu-system-x86_64`, the `virbr0` bridge (libvirt's default NAT
  network), `allow virbr0` in `/etc/qemu/bridge.conf`, and a setuid `qemu-bridge-helper`.
  Membership of the `kvm` group makes the VM much faster.
- An ssh key at `~/.ssh/id_ed25519` (`ssh-keygen -t ed25519` if you don't have one).

Expected layout (the workspace refers to the crate forks by relative path):

```
~/Projects/
├── QNX/
│   ├── tailscale-QNX/      this repo
│   └── qnx-crates/         patched crate forks (created in step 2)
└── tailscale-rs/           upstream checkout @ 4d66c5a (only needed for step 6)
```

## 1. Toolchain

`x86_64-pc-qnx` is a tier-3 Rust target with no prebuilt std, so std is built from source with nightly
`-Zbuild-std`. This repo's `rust-toolchain.toml` pins a nightly that is known to work:

```bash
rustup toolchain install nightly-2026-11-09 --component rust-src --profile minimal
```

Every shell that builds for QNX needs the SDP environment, which puts `qcc` on `PATH`:

```bash
source ~/qnx800/qnxsdp-env.sh
```

`.cargo/config.toml` already holds the rest: target `x86_64-pc-qnx`, `qcc` as linker, `build-std`, the
`CC`/`CXX`/`AR` settings for C dependencies, and `scripts/qnx-run.sh` as the runner.

## 2. Patched crates

mio, socket2 and tokio don't know `target_os = "qnx"` yet. Create the patched forks next to this repo:

```bash
scripts/setup-qnx-crates.sh          # -> ~/Projects/QNX/qnx-crates/{mio-1.2.4,socket2-0.6.5,tokio-1.53.3}
```

Each fork is a git repo: one commit with the pristine crates.io source, and the QNX patch left uncommitted,
so `git diff` in a fork shows exactly the change. `Cargo.toml` uses the forks through `[patch.crates-io]`.

## 3. Build

```bash
source ~/qnx800/qnxsdp-env.sh
cargo build --workspace              # debug
cargo build --workspace --release    # release (panic=abort)
```

The first build compiles std itself and takes about a minute. Binaries are written to
`target/x86_64-pc-qnx/{debug,release}/`:

```bash
$ file target/x86_64-pc-qnx/debug/hello
... ELF 64-bit LSB pie executable, x86-64, ... interpreter /usr/lib/ldqnx-64.so.2 ...
```

## 4. Set up the QEMU target (once)

The target is a mkqnximage project at `~/qnxprojects/targets/qemu-qnx800-x86_64` (for example, created by
the QNX VS Code extension). Two changes are needed before you can deploy over ssh.

**a. Fix sshd.** The SDP's OpenSSH needs `sshd-auth`, but mkqnximage's image template only installs
`sshd-session`. Without the fix, sshd fails at boot with `/usr/libexec/sshd-auth does not exist`. Append to the
target's snippets:

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

## 5. Run on the target

Boot the VM headless (the serial console goes to `output/qemu.out`):

```bash
cd ~/qnxprojects/targets/qemu-qnx800-x86_64
./mkqnximage-wrapper.sh --run=-h
grep 'Startup complete' output/qemu.out     # booted when this prints
```

Then, from this repo, `cargo run` builds, deploys and runs in one step:

```bash
source ~/qnx800/qnxsdp-env.sh
cargo run -p hello
cargo run -p std-smoke
cargo run -p tokio-smoke
cargo run --release -p tokio-smoke
```

Expected output:

```
hello from Rust on qnx (x86_64), pid ...
...
13 checks, 0 required failures       # std-smoke
4 checks, 0 failures                 # tokio-smoke
```

`std-smoke` and `tokio-smoke` exit non-zero if a required check fails. `net/internet` (DNS + TCP to
`controlplane.tailscale.com:443`) is optional; it prints `WARN` when the VM has no outbound network.

### How deployment works

`scripts/qnx-run.sh <binary> [args...]` strips the binary (the data partition is only about 60 MB),
copies it to `/data/home/root/bin/` on the target, and runs it over ssh. Arguments and the exit code are
passed through. It finds the VM's IP with `scripts/qnx-target-ip.sh`, because the IP changes on every boot.
Environment overrides:

| Variable | Default |
|---|---|
| `QNX_HOST_IP` | looked up from `/var/lib/libvirt/dnsmasq/virbr0.status` |
| `QNX_USER` | `root` |
| `QNX_TARGET_DIR` | `~/qnxprojects/targets/qemu-qnx800-x86_64` |
| `QNX_REMOTE_DIR` | `/data/home/$QNX_USER/bin` |

To ssh in by hand:

```bash
ssh -o UserKnownHostsFile=~/qnxprojects/targets/qemu-qnx800-x86_64/local/ssh-ident root@$(scripts/qnx-target-ip.sh)
```

The runner doesn't forward environment variables. To pass any, run the deployed binary over ssh, for example
`ssh ... root@$IP 'cd bin && UDP_CLIENTS=64 ./tokio-smoke'`.

### UDP over the real NIC (host ↔ target)

```bash
IP=$(scripts/qnx-target-ip.sh)
SSH="ssh -o UserKnownHostsFile=$HOME/qnxprojects/targets/qemu-qnx800-x86_64/local/ssh-ident root@$IP"
cargo run -p tokio-smoke                                  # deploys the current build
$SSH 'cd bin && (./tokio-smoke serve 0.0.0.0:9999 > /tmp/serve.log 2>&1 &)'
python3 scripts/udp-echo-client.py $IP 9999 2000          # -> 2000/2000 echoed ..., rtt p50=...
$SSH 'slay -f tokio-smoke'
```

### tokio-smoke knobs

Set these on the target (see above) to stress the `udp-echo` check: `UDP_CLIENTS` (default 32),
`UDP_ROUNDS` (100), and `UDP_RCVBUF` (1 MiB, the server's `SO_RCVBUF`). QNX's default UDP receive buffer
is small, which is why the server raises it. See "Phase 0 notes" in the plan.

## 6. Building tailscale-rs crates for QNX

tailscale-rs pins stable 1.98.1, so pass the nightly explicitly. Put a local cargo config in the checkout,
which keeps upstream files untouched:

```bash
cd ~/Projects/tailscale-rs
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
EOF
echo '/.cargo/' >> .git/info/exclude

source ~/qnx800/qnxsdp-env.sh
cargo +nightly-2026-11-09 build --target x86_64-pc-qnx -p ts_bart
```

Unlike this repo, it doesn't set `[build] target`, so host builds and tests in tailscale-rs work as before.
Crates that depend on tokio, mio or socket2 also need the patched forks. That's Phase 1 work: add a
`[patch.crates-io]` table to the same config file.

## Troubleshooting

| Symptom | Fix |
|---|---|
| `linker 'qcc' not found` | `source ~/qnx800/qnxsdp-env.sh` in this shell |
| `` error[E0463]: can't find crate for `core` `` | `rust-src` missing: `rustup component add rust-src --toolchain nightly-2026-11-09` |
| `failed to read .../qnx-crates/mio-1.2.4/Cargo.toml` | run `scripts/setup-qnx-crates.sh` |
| `ssh: connect ... Connection refused` | sshd not running: apply step 4a and rebuild the image |
| `Permission denied (publickey,...)` | rebuild the image with `--ssh-ident` (step 4b) |
| `no DHCP lease for ...` / `No route to host` | VM not booted yet (check `output/qemu.out`) or not running (`pgrep -a qemu-system`) |
| `No space left on device` during deploy | clear `/data/home/root/bin` on the target; deploy release builds |
