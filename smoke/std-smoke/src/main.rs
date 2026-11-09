//! std smoke test for the x86_64-pc-qnx target: exercises the std pieces tailscale-rs
//! and its runtime lean on. Prints PASS/FAIL per check; exits 1 if any required check fails.

use std::io::{Read, Write};
use std::net::{
    Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream, ToSocketAddrs, UdpSocket,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use std::{env, fs, panic, thread};

type R = Result<String, String>;

fn e<E: std::fmt::Display>(err: E) -> String {
    err.to_string()
}

fn threads() -> R {
    let counter = Arc::new(AtomicUsize::new(0));
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let c = counter.clone();
            thread::spawn(move || {
                for _ in 0..10_000 {
                    c.fetch_add(1, Ordering::Relaxed);
                }
            })
        })
        .collect();
    for h in handles {
        h.join().map_err(|_| "join failed")?;
    }
    let n = counter.load(Ordering::Relaxed);
    if n != 80_000 {
        return Err(format!("counter = {n}"));
    }
    let named = thread::Builder::new()
        .name("qnx-worker".into())
        .stack_size(256 * 1024)
        .spawn(|| thread::current().name().map(str::to_owned))
        .map_err(e)?
        .join()
        .map_err(|_| "join failed")?;
    let par = thread::available_parallelism().map_err(e)?;
    Ok(format!("8x10k atomic incs, named={named:?}, available_parallelism={par}"))
}

fn sync_primitives() -> R {
    let pair = Arc::new((Mutex::new(false), Condvar::new()));
    let p2 = pair.clone();
    let t = thread::spawn(move || {
        *p2.0.lock().unwrap() = true;
        p2.1.notify_one();
    });
    let (lock, cv) = &*pair;
    let (guard, timeout) = cv
        .wait_timeout_while(lock.lock().unwrap(), Duration::from_secs(5), |ready| !*ready)
        .unwrap();
    t.join().unwrap();
    if timeout.timed_out() || !*guard {
        return Err("condvar wait timed out".into());
    }
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || tx.send(42).unwrap());
    let v = rx.recv_timeout(Duration::from_secs(5)).map_err(e)?;
    Ok(format!("mutex+condvar ok, mpsc got {v}"))
}

fn time() -> R {
    let start = Instant::now();
    thread::sleep(Duration::from_millis(50));
    let slept = start.elapsed();
    if slept < Duration::from_millis(50) || slept > Duration::from_millis(500) {
        return Err(format!("sleep(50ms) took {slept:?}"));
    }
    // Monotonic: many successive reads never go backwards.
    let mut prev = Instant::now();
    for _ in 0..100_000 {
        let now = Instant::now();
        if now < prev {
            return Err("Instant went backwards".into());
        }
        prev = now;
    }
    let since_epoch = SystemTime::now().duration_since(UNIX_EPOCH).map_err(e)?;
    if since_epoch.as_secs() < 1_700_000_000 {
        return Err(format!("wall clock looks unset: {}s since epoch", since_epoch.as_secs()));
    }
    Ok(format!("sleep(50ms)={slept:?}, unix time={}s", since_epoch.as_secs()))
}

fn filesystem() -> R {
    let dir = env::temp_dir().join(format!("std-smoke-{}", std::process::id()));
    fs::create_dir_all(dir.join("sub")).map_err(e)?;
    let f = dir.join("sub/file.txt");
    fs::write(&f, b"hello qnx\n").map_err(e)?;
    let mut fh = fs::OpenOptions::new().append(true).open(&f).map_err(e)?;
    fh.write_all(b"line 2\n").map_err(e)?;
    fh.sync_all().map_err(e)?;
    drop(fh);
    let content = fs::read_to_string(&f).map_err(e)?;
    if content != "hello qnx\nline 2\n" {
        return Err(format!("read back {content:?}"));
    }
    let meta = fs::metadata(&f).map_err(e)?;
    let renamed = dir.join("sub/renamed.txt");
    fs::rename(&f, &renamed).map_err(e)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{PermissionsExt, symlink};
        fs::set_permissions(&renamed, fs::Permissions::from_mode(0o600)).map_err(e)?;
        let mode = fs::metadata(&renamed).map_err(e)?.permissions().mode() & 0o777;
        if mode != 0o600 {
            return Err(format!("mode {mode:o}"));
        }
        symlink(&renamed, dir.join("link")).map_err(e)?;
        let target = fs::read_link(dir.join("link")).map_err(e)?;
        if target != renamed {
            return Err(format!("readlink {target:?}"));
        }
    }
    let entries: Vec<_> = fs::read_dir(dir.join("sub"))
        .map_err(e)?
        .map(|d| d.map(|d| d.file_name()))
        .collect::<Result<_, _>>()
        .map_err(e)?;
    let canon = fs::canonicalize(&dir).map_err(e)?;
    fs::remove_dir_all(&dir).map_err(e)?;
    if dir.exists() {
        return Err("remove_dir_all left the dir".into());
    }
    Ok(format!(
        "tmp={}, len={}, read_dir={entries:?}, canonical={}",
        env::temp_dir().display(),
        meta.len(),
        canon.display()
    ))
}

fn environment() -> R {
    // SAFETY: single-threaded at this point in practice; set_var is unsafe in edition 2024.
    unsafe { env::set_var("STD_SMOKE_VAR", "qnx") };
    if env::var("STD_SMOKE_VAR").as_deref() != Ok("qnx") {
        return Err("set_var/var mismatch".into());
    }
    unsafe { env::remove_var("STD_SMOKE_VAR") };
    let exe = env::current_exe().map_err(e)?;
    let cwd = env::current_dir().map_err(e)?;
    let home = env::home_dir();
    Ok(format!(
        "OS={} ARCH={} FAMILY={} exe={} cwd={} home={home:?} args={:?}",
        env::consts::OS,
        env::consts::ARCH,
        env::consts::FAMILY,
        exe.display(),
        cwd.display(),
        env::args().collect::<Vec<_>>()
    ))
}

fn panics() -> R {
    if cfg!(panic = "abort") {
        return Ok("skipped: built with panic=abort".into());
    }
    let prev = panic::take_hook();
    panic::set_hook(Box::new(|_| {}));
    let r = panic::catch_unwind(|| panic!("expected test panic"));
    let thread_r = thread::spawn(|| panic!("panic in thread")).join();
    panic::set_hook(prev);
    match (r, thread_r) {
        (Err(p), Err(_)) => Ok(format!(
            "caught: {:?}; thread panic surfaced via join",
            p.downcast_ref::<&str>()
        )),
        _ => Err("panic was not caught".into()),
    }
}

fn tcp_loopback(addr: SocketAddr) -> R {
    let listener = TcpListener::bind(addr).map_err(e)?;
    let local = listener.local_addr().map_err(e)?;
    let server = thread::spawn(move || -> std::io::Result<()> {
        let (mut s, peer) = listener.accept()?;
        let mut buf = [0u8; 64];
        let n = s.read(&mut buf)?;
        s.write_all(&buf[..n])?;
        let _ = peer;
        Ok(())
    });
    let mut c = TcpStream::connect_timeout(&local, Duration::from_secs(5)).map_err(e)?;
    c.set_nodelay(true).map_err(e)?;
    c.set_read_timeout(Some(Duration::from_secs(5))).map_err(e)?;
    c.write_all(b"ping-tcp").map_err(e)?;
    let mut buf = [0u8; 8];
    c.read_exact(&mut buf).map_err(e)?;
    server.join().map_err(|_| "server panicked")?.map_err(e)?;
    if &buf != b"ping-tcp" {
        return Err(format!("echo mismatch {buf:?}"));
    }
    Ok(format!("echo via {local}, nodelay={}", c.nodelay().map_err(e)?))
}

fn udp_loopback(addr: SocketAddr) -> R {
    let a = UdpSocket::bind(addr).map_err(e)?;
    let b = UdpSocket::bind(addr).map_err(e)?;
    a.set_read_timeout(Some(Duration::from_secs(5))).map_err(e)?;
    b.set_read_timeout(Some(Duration::from_secs(5))).map_err(e)?;
    let (aa, ba) = (a.local_addr().map_err(e)?, b.local_addr().map_err(e)?);
    a.send_to(b"ping-udp", ba).map_err(e)?;
    let mut buf = [0u8; 64];
    let (n, from) = b.recv_from(&mut buf).map_err(e)?;
    if &buf[..n] != b"ping-udp" || from != aa {
        return Err(format!("got {:?} from {from}", &buf[..n]));
    }
    b.connect(aa).map_err(e)?;
    b.send(b"pong").map_err(e)?;
    let n = a.recv(&mut buf).map_err(e)?;
    if &buf[..n] != b"pong" {
        return Err("connected send/recv mismatch".into());
    }
    // Non-blocking recv on an empty socket must be WouldBlock (what mio relies on).
    a.set_nonblocking(true).map_err(e)?;
    match a.recv(&mut buf) {
        Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {}
        other => return Err(format!("nonblocking recv: {other:?}")),
    }
    Ok(format!("{aa} <-> {ba}, nonblocking WouldBlock ok"))
}

fn dns_localhost() -> R {
    let addrs: Vec<_> = ("localhost", 80).to_socket_addrs().map_err(e)?.collect();
    if addrs.is_empty() {
        return Err("no addresses".into());
    }
    Ok(format!("localhost -> {addrs:?}"))
}

fn dns_and_tcp_internet() -> R {
    // Needs the target to have outbound network; reported but not fatal.
    let host = "controlplane.tailscale.com";
    let addrs: Vec<_> = (host, 443).to_socket_addrs().map_err(e)?.collect();
    let v4 = addrs
        .iter()
        .find(|a| a.is_ipv4())
        .ok_or_else(|| format!("no IPv4 for {host}: {addrs:?}"))?;
    let t = Instant::now();
    TcpStream::connect_timeout(v4, Duration::from_secs(5)).map_err(e)?;
    Ok(format!("{host} -> {addrs:?}; TCP connect to {v4} in {:?}", t.elapsed()))
}

fn process_spawn() -> R {
    let out = std::process::Command::new("uname").arg("-a").output().map_err(e)?;
    if !out.status.success() {
        return Err(format!("uname exited {}", out.status));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn main() {
    let v4 = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
    let v6 = SocketAddr::from((Ipv6Addr::LOCALHOST, 0));
    let checks: Vec<(&str, bool, Box<dyn Fn() -> R>)> = vec![
        ("threads", true, Box::new(threads)),
        ("sync", true, Box::new(sync_primitives)),
        ("time", true, Box::new(time)),
        ("fs", true, Box::new(filesystem)),
        ("env", true, Box::new(environment)),
        ("panic", true, Box::new(panics)),
        ("tcp/v4", true, Box::new(move || tcp_loopback(v4))),
        ("tcp/v6", false, Box::new(move || tcp_loopback(v6))),
        ("udp/v4", true, Box::new(move || udp_loopback(v4))),
        ("udp/v6", false, Box::new(move || udp_loopback(v6))),
        ("dns/localhost", true, Box::new(dns_localhost)),
        ("process", false, Box::new(process_spawn)),
        ("net/internet", false, Box::new(dns_and_tcp_internet)),
    ];
    let mut failed = 0;
    for (name, required, check) in &checks {
        let t = Instant::now();
        match check() {
            Ok(msg) => println!("PASS {name:<14} {msg} ({:?})", t.elapsed()),
            Err(err) if *required => {
                failed += 1;
                println!("FAIL {name:<14} {err}");
            }
            Err(err) => println!("WARN {name:<14} {err} (optional)"),
        }
    }
    println!("{} checks, {failed} required failures", checks.len());
    std::process::exit(if failed == 0 { 0 } else { 1 });
}
