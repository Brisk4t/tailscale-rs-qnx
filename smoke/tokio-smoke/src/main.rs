//! tokio smoke test for x86_64-pc-qnx (exercises the patched mio poll(2) backend).
//!
//!   tokio-smoke                 run in-process checks over loopback, exit 1 on failure
//!   tokio-smoke serve <addr>    run a UDP echo server on <addr> (e.g. 0.0.0.0:9999) until killed
//!
//! Env knobs for the udp-echo check: UDP_CLIENTS (32), UDP_ROUNDS (100), UDP_RCVBUF (1 MiB).

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::time::{interval, sleep, timeout};

type R = Result<String, String>;

fn e<E: std::fmt::Display>(err: E) -> String {
    err.to_string()
}

/// Echo every datagram back to its sender, forever.
async fn udp_echo_server(sock: Arc<UdpSocket>) -> std::io::Result<()> {
    let mut buf = vec![0u8; 65536];
    loop {
        let (n, from) = sock.recv_from(&mut buf).await?;
        sock.send_to(&buf[..n], from).await?;
    }
}

/// Bind a UDP socket with a 1 MiB receive buffer. QNX io-sock defaults to FreeBSD's
/// net.inet.udp.recvspace=42080, which only holds ~20 queued datagrams (each is charged
/// a 2 KiB mbuf cluster), so a busy server drops packets without this.
fn bind_udp_server(addr: SocketAddr) -> std::io::Result<(UdpSocket, usize, usize)> {
    use socket2::{Domain, Protocol, Socket, Type};
    let sock = Socket::new(Domain::for_address(addr), Type::DGRAM, Some(Protocol::UDP))?;
    let before = sock.recv_buffer_size()?;
    sock.set_recv_buffer_size(std::env::var("UDP_RCVBUF").ok().and_then(|v| v.parse().ok()).unwrap_or(1 << 20))?;
    let after = sock.recv_buffer_size()?;
    sock.set_nonblocking(true)?;
    sock.bind(&addr.into())?;
    Ok((UdpSocket::from_std(sock.into())?, before, after))
}

async fn udp_echo() -> R {
    let (server, rcvbuf_before, rcvbuf_after) =
        bind_udp_server((Ipv4Addr::LOCALHOST, 0).into()).map_err(e)?;
    let server = Arc::new(server);
    let addr = server.local_addr().map_err(e)?;
    let srv = tokio::spawn(udp_echo_server(server));

    // N concurrent clients x M round trips each (env UDP_CLIENTS / UDP_ROUNDS).
    let env_n = |k: &str, d: u32| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
    let (nclients, rounds) = (env_n("UDP_CLIENTS", 32), env_n("UDP_ROUNDS", 100));
    let start = Instant::now();
    let clients: Vec<_> = (0..nclients)
        .map(|c| {
            tokio::spawn(async move {
                let sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await?;
                sock.connect(addr).await?;
                let mut buf = [0u8; 64];
                for i in 0..rounds {
                    let msg = format!("client {c} msg {i}");
                    sock.send(msg.as_bytes()).await?;
                    let n = timeout(Duration::from_secs(5), sock.recv(&mut buf))
                        .await
                        .map_err(|_| std::io::Error::other(format!("client {c} recv {i} timed out")))??;
                    if &buf[..n] != msg.as_bytes() {
                        return Err(std::io::Error::other("echo mismatch"));
                    }
                }
                Ok::<_, std::io::Error>(())
            })
        })
        .collect();
    for c in clients {
        c.await.map_err(e)?.map_err(e)?;
    }
    let elapsed = start.elapsed();
    srv.abort();
    Ok(format!(
        "{nclients}x{rounds} round trips via {addr} in {elapsed:?} (SO_RCVBUF {rcvbuf_before} -> {rcvbuf_after})"
    ))
}

async fn tcp_echo() -> R {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.map_err(e)?;
    let addr = listener.local_addr().map_err(e)?;
    let srv = tokio::spawn(async move {
        loop {
            let (mut s, _) = listener.accept().await?;
            tokio::spawn(async move {
                let (mut r, mut w) = s.split();
                tokio::io::copy(&mut r, &mut w).await
            });
        }
        #[allow(unreachable_code)]
        Ok::<_, std::io::Error>(())
    });
    let payload: Vec<u8> = (0..1_048_576u32).map(|i| (i % 251) as u8).collect();
    let mut s = TcpStream::connect(addr).await.map_err(e)?;
    s.set_nodelay(true).map_err(e)?;
    let (mut r, mut w) = s.split();
    let start = Instant::now();
    let (wr, rd) = tokio::join!(
        async {
            w.write_all(&payload).await?;
            w.shutdown().await
        },
        async {
            let mut got = Vec::with_capacity(payload.len());
            r.read_to_end(&mut got).await.map(|_| got)
        }
    );
    wr.map_err(e)?;
    let got = rd.map_err(e)?;
    srv.abort();
    if got != payload {
        return Err(format!("echoed {} bytes, mismatch", got.len()));
    }
    Ok(format!("1 MiB echoed via {addr} in {:?}", start.elapsed()))
}

async fn timers() -> R {
    let t = Instant::now();
    sleep(Duration::from_millis(100)).await;
    let slept = t.elapsed();
    if slept < Duration::from_millis(100) || slept > Duration::from_millis(600) {
        return Err(format!("sleep(100ms) took {slept:?}"));
    }
    let mut iv = interval(Duration::from_millis(20));
    let t = Instant::now();
    for _ in 0..6 {
        iv.tick().await; // first tick is immediate
    }
    let ticks = t.elapsed();
    if ticks < Duration::from_millis(100) {
        return Err(format!("6 ticks @20ms took {ticks:?}"));
    }
    match timeout(Duration::from_millis(50), sleep(Duration::from_secs(10))).await {
        Err(_) => {}
        Ok(()) => return Err("timeout did not fire".into()),
    }
    Ok(format!("sleep(100ms)={slept:?}, 6 ticks={ticks:?}, timeout ok"))
}

async fn tasks() -> R {
    let blocking = tokio::task::spawn_blocking(|| {
        std::thread::sleep(Duration::from_millis(20));
        std::thread::current().name().map(str::to_owned)
    })
    .await
    .map_err(e)?;
    let handles: Vec<_> = (0..1000u64).map(|i| tokio::spawn(async move { i * 2 })).collect();
    let mut sum = 0;
    for h in handles {
        sum += h.await.map_err(e)?;
    }
    if sum != 999_000 {
        return Err(format!("sum {sum}"));
    }
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    tokio::spawn(async move {
        for i in 0..100 {
            tx.send(i).await.unwrap();
        }
    });
    let mut n = 0;
    while let Some(_) = rx.recv().await {
        n += 1;
    }
    Ok(format!("1000 tasks, spawn_blocking on {blocking:?}, mpsc {n} msgs"))
}

async fn run_checks() -> i32 {
    let mut failed = 0;
    macro_rules! check {
        ($name:expr, $fut:expr) => {{
            let t = Instant::now();
            match timeout(Duration::from_secs(30), $fut).await {
                Ok(Ok(msg)) => println!("PASS {:<9} {msg} ({:?})", $name, t.elapsed()),
                Ok(Err(err)) => {
                    failed += 1;
                    println!("FAIL {:<9} {err}", $name)
                }
                Err(_) => {
                    failed += 1;
                    println!("FAIL {:<9} timed out after 30s", $name)
                }
            }
        }};
    }
    check!("timers", timers());
    check!("tasks", tasks());
    check!("udp-echo", udp_echo());
    check!("tcp-echo", tcp_echo());
    println!("4 checks, {failed} failures");
    failed
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    println!(
        "tokio-smoke on {}/{} ({} workers)",
        std::env::consts::OS,
        std::env::consts::ARCH,
        tokio::runtime::Handle::current().metrics().num_workers()
    );
    match args.get(1).map(String::as_str) {
        Some("serve") => {
            let addr: SocketAddr = args
                .get(2)
                .map(|a| a.parse().expect("bad address"))
                .unwrap_or_else(|| "0.0.0.0:9999".parse().unwrap());
            let (sock, _, rcvbuf) = bind_udp_server(addr).expect("bind");
            let sock = Arc::new(sock);
            println!(
                "UDP echo server listening on {} (SO_RCVBUF {rcvbuf})",
                sock.local_addr().unwrap()
            );
            udp_echo_server(sock).await.expect("echo server");
        }
        None => std::process::exit(run_checks().await),
        Some(other) => {
            eprintln!("unknown command {other:?}; usage: tokio-smoke [serve <addr>]");
            std::process::exit(2);
        }
    }
}
