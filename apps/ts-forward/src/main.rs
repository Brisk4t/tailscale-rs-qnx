//! ts-forward: connect local TCP programs and tailnet peers, both ways.
//!
//! Joins the tailnet as its own device (tailscale-rs, userspace network stack).
//!
//! - **Inbound** (`--preset`, `--forward`): listens on tailnet ports and forwards each connection to a
//!   local TCP address, so local services (sshd, web servers, qconn) are reachable from the tailnet.
//! - **Outbound** (`--to`): listens on a local port and forwards each connection to a tailnet peer,
//!   so local programs can reach peers. Other processes can't reach `100.x` addresses directly: the
//!   tailnet only exists inside this process.
//!
//! ```text
//! ts-forward                                  # inbound ssh only (the default)
//! ts-forward -p qnx-dev -p https              # inbound ssh + qconn + https
//! ts-forward -f 2222=22 -f 8443=192.168.1.10:443
//! ts-forward --to 2201=laptop:22              # local 127.0.0.1:2201 -> peer `laptop` port 22
//! ```
//!
//! Inbound access control is the tailnet's: any peer the tailnet policy lets reach this device can
//! connect. The local service sees connections from 127.0.0.1, not from the peer's tailnet address.

mod presets;

use std::{
    collections::BTreeMap,
    error::Error,
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    process::ExitCode,
    sync::Arc,
    time::{Duration, Instant},
};

use clap::Parser;
use tailscale::{AuthState, Config, Device, netstack::TcpListener};
use tokio::{net::TcpStream, sync::Semaphore};
use tracing_subscriber::filter::LevelFilter;

#[derive(Parser)]
#[command(
    version,
    about = "Expose local TCP services (ssh, https, qconn, ...) on a tailnet, and reach tailnet peers from local programs"
)]
struct Args {
    /// Key file holding this device's tailnet identity. Created if missing.
    #[arg(short = 'c', long, default_value = "ts-forward-keys.json")]
    key_file: PathBuf,

    /// Auth key. Not needed once the key file is authorized; without one, a login URL is printed.
    #[arg(short = 'k', long, env = "TS_AUTH_KEY")]
    auth_key: Option<String>,

    /// Name to request on the tailnet. Defaults to the OS hostname (unless that's empty or
    /// "localhost", in which case the control server picks one).
    #[arg(short = 'H', long)]
    hostname: Option<String>,

    /// Control server URL. Defaults to Tailscale's.
    #[arg(long, env = "TS_CONTROL_URL")]
    control_url: Option<url::Url>,

    /// Built-in forward or group to enable (repeatable). See --list-presets.
    #[arg(short = 'p', long = "preset", value_name = "NAME")]
    presets: Vec<String>,

    /// Custom forward (repeatable): `PORT`, `TAILNET_PORT=PORT` or `TAILNET_PORT=HOST:PORT`.
    /// Local targets default to 127.0.0.1.
    #[arg(short = 'f', long = "forward", value_name = "SPEC")]
    forwards: Vec<String>,

    /// Outbound forward (repeatable): `[BIND:]LOCAL_PORT=PEER:PORT`. Local programs connect to
    /// BIND:LOCAL_PORT (default bind 127.0.0.1) and reach PEER:PORT on the tailnet. PEER is a
    /// tailnet IP or a peer's name.
    #[arg(short = 't', long = "to", value_name = "SPEC")]
    outbound: Vec<String>,

    /// Print the built-in presets and groups, then exit.
    #[arg(long)]
    list_presets: bool,

    /// Maximum simultaneous forwarded connections, across all forwards.
    #[arg(long, default_value_t = 64)]
    max_connections: usize,

    /// Seconds to wait when connecting to a local target (inbound) or a tailnet peer (outbound).
    #[arg(long, default_value_t = 10)]
    connect_timeout: u64,

    /// Don't listen on the device's tailnet IPv6 address.
    #[arg(long)]
    no_ipv6: bool,

    /// Seconds to wait for the Tailscale control server at startup before exiting with an error.
    /// tailscale-rs doesn't retry a failed first connection (e.g. DNS not ready yet), so exiting
    /// lets a supervisor restart it. Waiting for a login URL to be approved isn't limited by this.
    #[arg(long, default_value_t = 60)]
    startup_timeout: u64,
}

/// One tailnet port forwarded to one local target.
#[derive(Clone)]
struct Forward {
    name: String,
    tailnet_port: u16,
    target: String,
}

fn parse_port(s: &str) -> Result<u16, String> {
    match s.parse::<u16>() {
        Ok(0) | Err(_) => Err(format!("invalid port `{s}`")),
        Ok(p) => Ok(p),
    }
}

/// `PORT`, `TAILNET_PORT=PORT` or `TAILNET_PORT=HOST:PORT`.
fn parse_forward(spec: &str) -> Result<Forward, String> {
    let (tailnet, target) = match spec.split_once('=') {
        None => (spec, spec),
        Some((t, target)) => (t, target),
    };
    let tailnet_port = parse_port(tailnet)?;
    let target = if target.contains(':') {
        let (_, port) = target.rsplit_once(':').unwrap();
        parse_port(port)?;
        target.to_owned()
    } else {
        format!("127.0.0.1:{}", parse_port(target)?)
    };
    Ok(Forward { name: format!("custom:{tailnet_port}"), tailnet_port, target })
}

/// A tailnet peer, by address or by name (resolved per connection, so it follows network map changes).
#[derive(Clone, Debug, PartialEq)]
enum PeerRef {
    Ip(IpAddr),
    Name(String),
}

impl std::fmt::Display for PeerRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PeerRef::Ip(ip) => write!(f, "{ip}"),
            PeerRef::Name(n) => f.write_str(n),
        }
    }
}

/// One local listening address forwarded to one tailnet peer port.
#[derive(Clone)]
struct Outbound {
    name: String,
    bind: SocketAddr,
    peer: PeerRef,
    port: u16,
}

/// `[BIND:]LOCAL_PORT=PEER:PORT`, e.g. `2201=laptop:22`, `0.0.0.0:2201=100.81.78.97:22`,
/// `2201=[fd7a:115c:a1e0::1]:22`.
fn parse_outbound(spec: &str) -> Result<Outbound, String> {
    let (local, remote) = spec
        .split_once('=')
        .ok_or_else(|| format!("`{spec}`: expected [BIND:]LOCAL_PORT=PEER:PORT"))?;

    let bind = if local.contains(':') {
        local.parse::<SocketAddr>().map_err(|_| format!("invalid bind address `{local}`"))?
    } else {
        SocketAddr::from(([127, 0, 0, 1], parse_port(local)?))
    };
    if bind.port() == 0 {
        return Err(format!("invalid port in `{local}`"));
    }

    let (peer, port) = if remote.starts_with('[') {
        let sa = remote.parse::<SocketAddr>().map_err(|_| format!("invalid peer address `{remote}`"))?;
        (PeerRef::Ip(sa.ip()), sa.port())
    } else {
        let (host, port) = remote
            .rsplit_once(':')
            .ok_or_else(|| format!("`{remote}`: expected PEER:PORT"))?;
        if host.is_empty() {
            return Err(format!("`{remote}`: missing peer"));
        }
        let peer = match host.parse::<IpAddr>() {
            Ok(ip) => PeerRef::Ip(ip),
            Err(_) => PeerRef::Name(host.to_owned()),
        };
        (peer, parse_port(port)?)
    };
    if port == 0 {
        return Err(format!("invalid port in `{remote}`"));
    }

    Ok(Outbound { name: format!("to:{peer}:{port}"), bind, peer, port })
}

fn build_outbound(args: &Args) -> Result<Vec<Outbound>, String> {
    let mut out: Vec<Outbound> = Vec::new();
    for spec in &args.outbound {
        let o = parse_outbound(spec)?;
        if let Some(old) = out.iter().find(|x| x.bind == o.bind) {
            return Err(format!("local address {} is used by both `{}` and `{}`", o.bind, old.name, o.name));
        }
        out.push(o);
    }
    Ok(out)
}

fn build_forwards(args: &Args) -> Result<Vec<Forward>, String> {
    let names: Vec<&str> = if args.presets.is_empty() && args.forwards.is_empty() && args.outbound.is_empty() {
        presets::DEFAULT.to_vec()
    } else {
        args.presets.iter().map(String::as_str).collect()
    };

    // Keyed by tailnet port: a preset listed twice (directly and via a group) is fine;
    // two different targets on one port are not.
    let mut by_port: BTreeMap<u16, Forward> = BTreeMap::new();
    let mut add = |f: Forward| -> Result<(), String> {
        match by_port.get(&f.tailnet_port) {
            Some(old) if old.target != f.target => Err(format!(
                "tailnet port {} is used by both `{}` ({}) and `{}` ({})",
                f.tailnet_port, old.name, old.target, f.name, f.target
            )),
            Some(_) => Ok(()),
            None => {
                by_port.insert(f.tailnet_port, f);
                Ok(())
            }
        }
    };

    for name in names {
        for p in presets::expand(name)? {
            add(Forward { name: p.name.to_owned(), tailnet_port: p.port, target: format!("127.0.0.1:{}", p.port) })?;
        }
    }
    for spec in &args.forwards {
        add(parse_forward(spec)?)?;
    }
    Ok(by_port.into_values().collect())
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> ExitCode {
    let args = Args::parse();
    if args.list_presets {
        presets::print_list();
        return ExitCode::SUCCESS;
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::builder()
                .with_default_directive(LevelFilter::INFO.into())
                .from_env_lossy(),
        )
        .init();

    let (forwards, outbound) = match (build_forwards(&args), build_outbound(&args)) {
        (Ok(f), Ok(o)) => (f, o),
        (Err(e), _) | (_, Err(e)) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };

    match run(args, forwards, outbound).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!(error = %e, "ts-forward stopped");
            ExitCode::FAILURE
        }
    }
}

async fn run(args: Args, forwards: Vec<Forward>, outbound: Vec<Outbound>) -> Result<(), Box<dyn Error>> {
    let mut config = Config::default_with_key_file(&args.key_file).await?;
    config.requested_hostname = args.hostname.clone().or_else(os_hostname);
    tracing::info!(hostname = config.requested_hostname.as_deref().unwrap_or("(none)"), "requesting tailnet name");
    if let Some(url) = args.control_url.clone() {
        config.control_server_url = url;
    }

    let startup = Duration::from_secs(args.startup_timeout);
    let no_control = || format!("no response from the control server within {} s (network or DNS not ready?)", startup.as_secs());

    let dev = Arc::new(
        tokio::time::timeout(startup, Device::new(&config, args.auth_key.clone()))
            .await
            .map_err(|_| no_control())??,
    );

    let auth = tokio::time::timeout(startup, dev.is_authorized()).await.map_err(|_| no_control())??;
    if let AuthState::NotAuthorized(url) = auth {
        tracing::warn!(%url, "device not authorized: open the URL to approve it (if this exits afterwards, run it again)");
    }
    let ipv4 = wait_for_ipv4(&dev).await?;
    let ipv6 = if args.no_ipv6 { None } else { dev.ipv6_addr().await.ok() };

    let limit = Arc::new(Semaphore::new(args.max_connections));
    let connect_timeout = Duration::from_secs(args.connect_timeout);

    for f in &forwards {
        warn_if_target_down(f).await;
        for ip in [Some(IpAddr::from(ipv4)), ipv6.map(IpAddr::from)].into_iter().flatten() {
            let addr = SocketAddr::new(ip, f.tailnet_port);
            let listener = dev.tcp_listen(addr).await?;
            tracing::info!(forward = %f.name, listen = %addr, target = %f.target, "forwarding");
            tokio::spawn(serve(listener, f.clone(), dev.clone(), limit.clone(), connect_timeout));
        }
    }
    for o in &outbound {
        if let Err(e) = resolve(&dev, &o.peer).await {
            tracing::warn!(forward = %o.name, error = %e, "peer not found (yet); will retry per connection");
        }
        let listener = tokio::net::TcpListener::bind(o.bind).await
            .map_err(|e| format!("binding {} for `{}`: {e}", o.bind, o.name))?;
        tracing::info!(forward = %o.name, listen = %o.bind, peer = %o.peer, port = o.port, "forwarding to tailnet");
        tokio::spawn(serve_outbound(listener, o.clone(), dev.clone(), limit.clone(), connect_timeout));
    }
    tracing::info!(inbound = forwards.len(), outbound = outbound.len(), tailnet_ip = %ipv4, "ready");

    shutdown_signal().await;
    tracing::info!("shutting down");
    if let Ok(dev) = Arc::try_unwrap(dev) {
        dev.shutdown(Some(Duration::from_secs(3))).await;
    }
    Ok(())
}

/// The OS hostname, if it's set to something meaningful.
///
/// tailscale-rs's `Config` passes `requested_hostname` through as-is, so leaving it `None` registers
/// the device without a name (the control server then calls it "node").
fn os_hostname() -> Option<String> {
    meaningful_hostname(&gethostname::gethostname().into_string().ok()?)
}

fn meaningful_hostname(name: &str) -> Option<String> {
    let name = name.trim();
    (!name.is_empty() && !name.eq_ignore_ascii_case("localhost")).then(|| name.to_owned())
}

/// The tailnet address appears once the device is registered and has its first network map.
async fn wait_for_ipv4(dev: &Device) -> Result<std::net::Ipv4Addr, Box<dyn Error>> {
    let start = Instant::now();
    loop {
        match dev.ipv4_addr().await {
            Ok(ip) => return Ok(ip),
            Err(e) if start.elapsed() < Duration::from_secs(300) => {
                tracing::debug!(error = %e, "no tailnet address yet");
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            Err(e) => return Err(format!("no tailnet address after 5 minutes: {e}").into()),
        }
    }
}

/// A forward to a port nothing listens on is allowed (the service may start later), but say so.
async fn warn_if_target_down(f: &Forward) {
    match tokio::time::timeout(Duration::from_secs(1), TcpStream::connect(&f.target)).await {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => tracing::warn!(forward = %f.name, target = %f.target, error = %e, "local target not reachable (yet)"),
        Err(_) => tracing::warn!(forward = %f.name, target = %f.target, "local target not answering (yet)"),
    }
}

async fn serve(
    listener: TcpListener,
    f: Forward,
    dev: Arc<Device>,
    limit: Arc<Semaphore>,
    connect_timeout: Duration,
) {
    loop {
        let conn = match listener.accept().await {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(forward = %f.name, error = %e, "accept failed, closing this listener");
                return;
            }
        };
        let Ok(permit) = limit.clone().try_acquire_owned() else {
            tracing::warn!(forward = %f.name, peer = %conn.remote_addr(), "connection limit reached, rejecting");
            continue;
        };
        let (f, dev) = (f.clone(), dev.clone());
        tokio::spawn(async move {
            let _permit = permit;
            handle(conn, f, dev, connect_timeout).await;
        });
    }
}

async fn handle(mut conn: tailscale::netstack::TcpStream, f: Forward, dev: Arc<Device>, connect_timeout: Duration) {
    let peer = conn.remote_addr();
    let who = match dev.peer_by_tailnet_ip(peer.ip()).await {
        Ok(Some(node)) => node.hostname,
        _ => "?".to_owned(),
    };

    let mut local = match tokio::time::timeout(connect_timeout, TcpStream::connect(&f.target)).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            tracing::warn!(forward = %f.name, %peer, %who, target = %f.target, error = %e, "local connect failed");
            return;
        }
        Err(_) => {
            tracing::warn!(forward = %f.name, %peer, %who, target = %f.target, "local connect timed out");
            return;
        }
    };
    let _ = local.set_nodelay(true);
    tracing::info!(forward = %f.name, %peer, %who, target = %f.target, "connection opened");

    let start = Instant::now();
    match tokio::io::copy_bidirectional(&mut conn, &mut local).await {
        Ok((to_local, to_peer)) => tracing::info!(
            forward = %f.name, %peer, %who, to_local, to_peer, secs = start.elapsed().as_secs(), "connection closed"
        ),
        Err(e) => tracing::info!(
            forward = %f.name, %peer, %who, error = %e, secs = start.elapsed().as_secs(), "connection ended"
        ),
    }
}

/// The peer's tailnet IPv4 address (or the given IP).
async fn resolve(dev: &Device, peer: &PeerRef) -> Result<IpAddr, String> {
    match peer {
        PeerRef::Ip(ip) => Ok(*ip),
        PeerRef::Name(name) => match dev.peer_by_name(name).await {
            Ok(Some(node)) => Ok(IpAddr::V4(node.tailnet_address.ipv4.addr())),
            Ok(None) => Err(format!("no tailnet peer named `{name}`")),
            Err(e) => Err(format!("looking up `{name}`: {e}")),
        },
    }
}

async fn serve_outbound(
    listener: tokio::net::TcpListener,
    o: Outbound,
    dev: Arc<Device>,
    limit: Arc<Semaphore>,
    connect_timeout: Duration,
) {
    loop {
        let (local, client) = match listener.accept().await {
            Ok(c) => c,
            Err(e) => {
                // Usually transient (e.g. out of file descriptors): back off and keep listening.
                tracing::warn!(forward = %o.name, error = %e, "local accept failed");
                tokio::time::sleep(Duration::from_millis(200)).await;
                continue;
            }
        };
        let Ok(permit) = limit.clone().try_acquire_owned() else {
            tracing::warn!(forward = %o.name, %client, "connection limit reached, rejecting");
            continue;
        };
        let (o, dev) = (o.clone(), dev.clone());
        tokio::spawn(async move {
            let _permit = permit;
            handle_outbound(local, client, o, dev, connect_timeout).await;
        });
    }
}

async fn handle_outbound(
    mut local: TcpStream,
    client: SocketAddr,
    o: Outbound,
    dev: Arc<Device>,
    connect_timeout: Duration,
) {
    let ip = match resolve(&dev, &o.peer).await {
        Ok(ip) => ip,
        Err(e) => {
            tracing::warn!(forward = %o.name, %client, error = %e, "can't resolve peer");
            return;
        }
    };
    let remote = SocketAddr::new(ip, o.port);
    let mut conn = match tokio::time::timeout(connect_timeout, dev.tcp_connect(remote)).await {
        Ok(Ok(c)) => c,
        Ok(Err(e)) => {
            tracing::warn!(forward = %o.name, %client, %remote, error = %e, "tailnet connect failed");
            return;
        }
        Err(_) => {
            tracing::warn!(forward = %o.name, %client, %remote, "tailnet connect timed out");
            return;
        }
    };
    let _ = local.set_nodelay(true);
    tracing::info!(forward = %o.name, %client, %remote, "connection opened");

    let start = Instant::now();
    match tokio::io::copy_bidirectional(&mut local, &mut conn).await {
        Ok((to_peer, to_local)) => tracing::info!(
            forward = %o.name, %client, %remote, to_peer, to_local, secs = start.elapsed().as_secs(), "connection closed"
        ),
        Err(e) => tracing::info!(
            forward = %o.name, %client, %remote, error = %e, secs = start.elapsed().as_secs(), "connection ended"
        ),
    }
}

async fn shutdown_signal() {
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("installing SIGTERM handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forward_specs() {
        let f = parse_forward("22").unwrap();
        assert_eq!((f.tailnet_port, f.target.as_str()), (22, "127.0.0.1:22"));
        let f = parse_forward("2222=22").unwrap();
        assert_eq!((f.tailnet_port, f.target.as_str()), (2222, "127.0.0.1:22"));
        let f = parse_forward("8443=192.168.1.10:443").unwrap();
        assert_eq!((f.tailnet_port, f.target.as_str()), (8443, "192.168.1.10:443"));
        assert!(parse_forward("0").is_err());
        assert!(parse_forward("22=host:x").is_err());
        assert!(parse_forward("abc").is_err());
    }

    #[test]
    fn outbound_specs() {
        let o = parse_outbound("2201=laptop:22").unwrap();
        assert_eq!(o.bind, "127.0.0.1:2201".parse().unwrap());
        assert_eq!((o.peer.clone(), o.port), (PeerRef::Name("laptop".into()), 22));
        let o = parse_outbound("0.0.0.0:2201=100.81.78.97:22").unwrap();
        assert_eq!(o.bind, "0.0.0.0:2201".parse().unwrap());
        assert_eq!(o.peer, PeerRef::Ip("100.81.78.97".parse().unwrap()));
        let o = parse_outbound("2201=[fd7a:115c:a1e0::1]:22").unwrap();
        assert_eq!(o.peer, PeerRef::Ip("fd7a:115c:a1e0::1".parse().unwrap()));
        let o = parse_outbound("2201=laptop.tail1234.ts.net.:22").unwrap();
        assert_eq!(o.peer, PeerRef::Name("laptop.tail1234.ts.net.".into()));
        for bad in ["2201", "2201=laptop", "2201=:22", "0=laptop:22", "2201=laptop:0", "x=laptop:22", "1.2.3.4:0=laptop:22"] {
            assert!(parse_outbound(bad).is_err(), "{bad} should be rejected");
        }
    }

    #[test]
    fn hostname_fallback() {
        assert_eq!(meaningful_hostname("qnx-tailscale").as_deref(), Some("qnx-tailscale"));
        assert_eq!(meaningful_hostname(" pi5\n").as_deref(), Some("pi5"));
        assert_eq!(meaningful_hostname("localhost"), None);
        assert_eq!(meaningful_hostname("LOCALHOST"), None);
        assert_eq!(meaningful_hostname(""), None);
    }

    #[test]
    fn presets_expand() {
        assert_eq!(presets::expand("qnx-dev").unwrap().len(), 2);
        assert_eq!(presets::expand("all").unwrap().len(), presets::PRESETS.len());
        assert!(presets::expand("nope").is_err());
    }
}
