//! Checks ts_netmon's `PollingMon` (the network monitor tailscale-rs falls back to on QNX)
//! against what the system reports.
//!
//! 1. `platform_mon()` is the polling monitor on this target.
//! 2. The initial snapshot has an up loopback interface with 127.0.0.1/8.
//! 3. The initial snapshot matches `ifconfig`: same interfaces, same up state, same addresses
//!    and prefix lengths (IPv4 and IPv6).
//! 4. Adding an alias address to the loopback interface is reported as `AddrUpsert`.
//! 5. Removing it again is reported as `AddrRemoved`.
//!
//! Checks 4–5 change the system's configuration (needs root), and are only run on QNX.

use std::{
    collections::{BTreeMap, BTreeSet},
    process::{Command, ExitCode},
    time::Duration,
};

use futures_util::StreamExt;
use ipnet::IpNet;
use ts_netmon::{BoxStream, Event, InterfaceId, MonType, Netmon, PollingMon};

/// Poll fast so change detection doesn't take 10 s per check.
const INTERVAL: Duration = Duration::from_secs(1);
/// Address added to (and removed from) the loopback interface for checks 4–5.
const ALIAS: &str = "10.99.0.1/32";

#[derive(Default)]
struct Report {
    checks: usize,
    failures: usize,
}

impl Report {
    fn check(&mut self, name: &str, result: Result<String, String>) {
        self.checks += 1;
        match result {
            Ok(detail) => println!("PASS {name}: {detail}"),
            Err(detail) => {
                self.failures += 1;
                println!("FAIL {name}: {detail}");
            }
        }
    }
}

/// Interface state as seen by the monitor: id -> (name, up, addresses).
type State = BTreeMap<InterfaceId, (String, bool, BTreeSet<IpNet>)>;

fn apply(state: &mut State, event: &Event) {
    match event {
        Event::InterfaceUpsert(iface) => {
            let entry = state.entry(iface.id.clone()).or_default();
            entry.0 = iface.name.clone();
            entry.1 = iface.up;
        }
        Event::InterfaceRemoved(id) => {
            state.remove(id);
        }
        Event::AddrUpsert(id, net) => {
            state.entry(id.clone()).or_default().2.insert(*net);
        }
        Event::AddrRemoved(id, net) => {
            if let Some(entry) = state.get_mut(id) {
                entry.2.remove(net);
            }
        }
        _ => {}
    }
}

/// Apply events until none arrive for `quiet`; returns the events seen.
async fn drain(
    stream: &mut BoxStream<std::io::Result<Event>>,
    state: &mut State,
    quiet: Duration,
) -> Vec<Event> {
    let mut seen = vec![];
    while let Ok(Some(event)) = tokio::time::timeout(quiet, stream.next()).await {
        match event {
            Ok(event) => {
                apply(state, &event);
                seen.push(event);
            }
            Err(e) => println!("     monitor error: {e}"),
        }
    }
    seen
}

/// Wait up to `within` for an event matching `pred`.
async fn wait_for(
    stream: &mut BoxStream<std::io::Result<Event>>,
    state: &mut State,
    within: Duration,
    pred: impl Fn(&Event) -> bool,
) -> Result<Duration, String> {
    let start = tokio::time::Instant::now();
    let deadline = start + within;
    loop {
        match tokio::time::timeout_at(deadline, stream.next()).await {
            Ok(Some(Ok(event))) => {
                apply(state, &event);
                if pred(&event) {
                    return Ok(start.elapsed());
                }
            }
            Ok(Some(Err(e))) => return Err(format!("monitor error: {e}")),
            Ok(None) => return Err("stream ended".into()),
            Err(_) => return Err(format!("no matching event within {within:?}")),
        }
    }
}

/// Parse FreeBSD-style `ifconfig` output (QNX io-sock, FreeBSD, macOS) into
/// name -> (up, addresses). IPv6 zone suffixes (`%vtnet0`) are dropped.
fn parse_ifconfig(out: &str) -> BTreeMap<String, (bool, BTreeSet<IpNet>)> {
    let mut result = BTreeMap::<String, (bool, BTreeSet<IpNet>)>::new();
    let mut current = None;

    for line in out.lines() {
        if !line.starts_with(char::is_whitespace) {
            // "vtnet0: flags=8843<UP,BROADCAST,...> metric 0 mtu 1500"
            let Some((name, rest)) = line.split_once(':') else {
                continue;
            };
            let up = rest
                .split_once('<')
                .and_then(|(_, f)| f.split_once('>'))
                .is_some_and(|(flags, _)| flags.split(',').any(|f| f == "UP"));
            result.insert(name.to_owned(), (up, BTreeSet::new()));
            current = Some(name.to_owned());
            continue;
        }

        let Some(name) = &current else { continue };
        let words: Vec<&str> = line.split_whitespace().collect();
        let net = match words.as_slice() {
            // "inet 192.168.122.229 netmask 0xffffff00 broadcast ..." (BSD: hex mask)
            // "inet 192.168.1.245  netmask 255.255.255.0  broadcast ..." (Linux: dotted)
            ["inet", addr, "netmask", mask, ..] => {
                let mask = match mask.strip_prefix("0x") {
                    Some(hex) => u32::from_str_radix(hex, 16).ok(),
                    None => mask.parse::<std::net::Ipv4Addr>().ok().map(u32::from),
                };
                mask.and_then(|m| format!("{addr}/{}", m.leading_ones()).parse().ok())
            }
            // "inet6 fe80::1%lo0 prefixlen 64 scopeid 0x2"
            ["inet6", addr, "prefixlen", len, ..] => {
                let addr = addr.split('%').next().unwrap_or(addr);
                format!("{addr}/{len}").parse().ok()
            }
            _ => None,
        };
        if let Some(net) = net {
            result.get_mut(name).unwrap().1.insert(net);
        }
    }

    result
}

fn ifconfig(args: &[&str]) -> Result<String, String> {
    let out = Command::new("ifconfig")
        .args(args)
        .output()
        .map_err(|e| format!("ifconfig {args:?}: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "ifconfig {args:?}: {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn compare_with_ifconfig(state: &State) -> Result<String, String> {
    let expected = parse_ifconfig(&ifconfig(&[])?);
    let actual: BTreeMap<String, (bool, BTreeSet<IpNet>)> = state
        .values()
        .map(|(name, up, addrs)| (name.clone(), (*up, addrs.clone())))
        .collect();

    if expected == actual {
        let n_addrs: usize = actual.values().map(|(_, a)| a.len()).sum();
        return Ok(format!(
            "{} interfaces, {n_addrs} addresses match",
            actual.len()
        ));
    }

    let mut diffs = vec![];
    for name in expected.keys().chain(actual.keys()).collect::<BTreeSet<_>>() {
        let (e, a) = (expected.get(name), actual.get(name));
        if e != a {
            diffs.push(format!("{name}: ifconfig={e:?} monitor={a:?}"));
        }
    }
    Err(diffs.join("; "))
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let mut report = Report::default();

    // 1. The fallback is what tailscale-rs gets on this platform.
    let platform = ts_netmon::platform_mon().map(|m| m.ty());
    report.check(
        "platform-mon",
        match &platform {
            Some(ty) if *ty == MonType::POLLING => Ok(format!("platform_mon() is {ty}")),
            Some(ty) if cfg!(target_os = "qnx") => Err(format!("expected polling, got {ty}")),
            Some(ty) => Ok(format!("platform_mon() is {ty} (not QNX: own backend)")),
            None => Err("platform_mon() is None".into()),
        },
    );

    let mon = PollingMon::new(INTERVAL);
    let mut stream = match mon.event_stream() {
        Ok(s) => s,
        Err(e) => {
            println!("FAIL event-stream: {e}");
            return ExitCode::FAILURE;
        }
    };

    let mut state = State::new();
    let initial = drain(&mut stream, &mut state, Duration::from_millis(500)).await;
    for (name, up, addrs) in state.values() {
        let addrs: Vec<String> = addrs.iter().map(ToString::to_string).collect();
        println!(
            "     {name}: {} [{}]",
            if *up { "up" } else { "down" },
            addrs.join(", ")
        );
    }

    // 2. Loopback.
    let loopback = state
        .iter()
        .find(|(_, (_, _, addrs))| addrs.contains(&"127.0.0.1/8".parse().unwrap()))
        .map(|(id, (name, up, _))| (id.clone(), name.clone(), *up));
    report.check(
        "loopback",
        match &loopback {
            Some((_, name, true)) => Ok(format!(
                "{name} is up with 127.0.0.1/8 ({} initial events)",
                initial.len()
            )),
            Some((_, name, false)) => Err(format!("{name} has 127.0.0.1/8 but is down")),
            None => Err("no interface with 127.0.0.1/8".into()),
        },
    );

    // 3. Same view as ifconfig.
    report.check("matches-ifconfig", compare_with_ifconfig(&state));

    // 4-5. Change detection, on QNX only (modifies the system).
    if cfg!(target_os = "qnx")
        && let Some((lo_id, lo_name, _)) = &loopback
    {
        let alias: IpNet = ALIAS.parse().unwrap();
        let wait = INTERVAL * 3;

        let added = ifconfig(&[lo_name, "inet", ALIAS, "alias"]);
        report.check(
            "detects-addr-added",
            match added {
                Ok(_) => wait_for(&mut stream, &mut state, wait, |e| {
                    *e == Event::AddrUpsert(lo_id.clone(), alias)
                })
                .await
                .map(|t| format!("AddrUpsert({lo_name}, {alias}) after {t:.1?}")),
                Err(e) => Err(e),
            },
        );

        let removed = ifconfig(&[lo_name, "inet", &alias.addr().to_string(), "-alias"]);
        report.check(
            "detects-addr-removed",
            match removed {
                Ok(_) => wait_for(&mut stream, &mut state, wait, |e| {
                    *e == Event::AddrRemoved(lo_id.clone(), alias)
                })
                .await
                .map(|t| format!("AddrRemoved({lo_name}, {alias}) after {t:.1?}")),
                Err(e) => Err(e),
            },
        );
    }

    println!("{} checks, {} failures", report.checks, report.failures);
    if report.failures == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
