//! What does poll(2) report for a TCP socket whose peer has closed?
//!
//! Reproduces the sequence of mio's `tcp::connect` test with raw poll(2) calls:
//! a non-blocking connect to a local listener, then the peer accepts and closes.
//! mio's poll(2) backend expects POLLIN (and/or POLLHUP) once the peer has closed.

use std::{
    io::ErrorKind,
    net::{SocketAddr, TcpListener},
    os::fd::AsRawFd,
    thread,
    time::Duration,
};

use socket2::{Domain, Socket, Type};

mod mio_case;
mod mio_loop;

fn flags(r: libc::c_short) -> String {
    let names = [
        (libc::POLLRDNORM, "RDNORM"),
        (libc::POLLRDBAND, "RDBAND"),
        (libc::POLLOUT, "OUT"),
        (libc::POLLPRI, "PRI"),
        (libc::POLLWRBAND, "WRBAND"),
        (libc::POLLERR, "ERR"),
        (libc::POLLHUP, "HUP"),
        (libc::POLLNVAL, "NVAL"),
    ];
    let mut v: Vec<&str> = names.iter().filter(|(b, _)| r & b != 0).map(|(_, n)| *n).collect();
    if v.is_empty() {
        v.push("-");
    }
    format!("{r:#06x} [{}]", v.join("|"))
}

fn poll1(fd: i32, events: libc::c_short, timeout_ms: i32) -> libc::c_short {
    let mut p = libc::pollfd { fd, events, revents: 0 };
    let n = unsafe { libc::poll(&mut p, 1, timeout_ms) };
    assert!(n >= 0, "poll: {}", std::io::Error::last_os_error());
    p.revents
}

/// Non-blocking connect (like mio's `TcpStream::connect`) to a fresh listener.
fn connect() -> (Socket, TcpListener) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    let s = Socket::new(Domain::IPV4, Type::STREAM, None).unwrap();
    s.set_nonblocking(true).unwrap();
    match s.connect(&addr.into()) {
        Ok(()) => {}
        Err(e) if e.raw_os_error() == Some(libc::EINPROGRESS) => {}
        Err(e) if e.kind() == ErrorKind::WouldBlock => {}
        Err(e) => panic!("connect: {e}"),
    }
    (s, listener)
}

fn close_peer(listener: TcpListener) {
    let (peer, _) = listener.accept().unwrap();
    drop(peer);
    thread::sleep(Duration::from_millis(200));
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some("mio-loop") {
        let n = std::env::args().nth(2).and_then(|n| n.parse().ok()).unwrap_or(300);
        mio_loop::run(n);
        return;
    }
    if std::env::args().nth(1).as_deref() == Some("mio") {
        mio_case::run().unwrap();
        return;
    }
    let in_out = libc::POLLIN | libc::POLLOUT;

    println!("POLLIN={:#06x} POLLOUT={:#06x} POLLHUP={:#06x}", libc::POLLIN, libc::POLLOUT, libc::POLLHUP);

    // 1. The mio sequence: register IN|OUT, get the first event, drop the reported bits
    //    (mio's edge-trigger emulation: `events &= !revents`), peer closes, poll again.
    let (s, listener) = connect();
    let fd = s.as_raw_fd();
    let first = poll1(fd, in_out, 2000);
    println!("1a  connect, poll(IN|OUT)          -> {}", flags(first));
    let remaining = in_out & !first;
    println!("    mio keeps events               =  {}", flags(remaining));
    close_peer(listener);
    let second = poll1(fd, remaining, 2000);
    println!("1b  peer closed, poll(remaining)   -> {}  {}", flags(second), if second == 0 { "TIMEOUT: mio hangs here" } else { "" });

    // 2. Peer closed: what does a fresh poll for each interest report?
    let (s, listener) = connect();
    let fd = s.as_raw_fd();
    poll1(fd, libc::POLLOUT, 2000);
    close_peer(listener);
    println!("2a  peer closed, poll(IN)          -> {}", flags(poll1(fd, libc::POLLIN, 2000)));
    println!("2b  peer closed, poll(RDNORM)      -> {}", flags(poll1(fd, libc::POLLRDNORM, 2000)));
    println!("2c  peer closed, poll(IN|OUT)      -> {}", flags(poll1(fd, in_out, 2000)));
    let mut buf = [0u8; 16];
    let r = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
    println!("2d  read()                         -> {r} (0 = EOF)");

    // 3. Blocked in poll(IN) *before* the peer closes: is the waiter woken?
    let (s, listener) = connect();
    let fd = s.as_raw_fd();
    poll1(fd, libc::POLLOUT, 2000);
    let closer = thread::spawn(move || {
        thread::sleep(Duration::from_millis(300));
        close_peer(listener);
    });
    println!("3   poll(IN) while peer closes     -> {}", flags(poll1(fd, libc::POLLIN, 3000)));
    closer.join().unwrap();
}
