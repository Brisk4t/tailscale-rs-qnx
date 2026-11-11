//! mio's `tests/tcp.rs::connect`, step by step, with mio's trace log on stderr.

use std::{io, net, sync::mpsc::channel, thread, time::Duration};

use mio::{Events, Interest, Poll, Token, net::TcpStream};

struct StderrLog;
impl log::Log for StderrLog {
    fn enabled(&self, _: &log::Metadata) -> bool {
        true
    }
    fn log(&self, r: &log::Record) {
        if r.target().starts_with("mio") {
            eprintln!("  [{}] {}", r.level(), r.args());
        }
    }
    fn flush(&self) {}
}

pub fn run() -> io::Result<()> {
    log::set_boxed_logger(Box::new(StderrLog)).unwrap();
    log::set_max_level(log::LevelFilter::Trace);

    let listener = net::TcpListener::bind("127.0.0.1:0")?;
    let addr = listener.local_addr()?;
    let (tx, rx) = channel::<()>();
    let (tx2, rx2) = channel::<()>();
    let server = thread::spawn(move || {
        let stream = listener.accept().unwrap();
        rx.recv().unwrap();
        drop(stream);
        tx2.send(()).unwrap();
    });

    let mut poll = Poll::new()?;
    let mut stream = TcpStream::connect(addr)?;
    poll.registry().register(&mut stream, Token(1), Interest::READABLE | Interest::WRITABLE)?;
    let mut events = Events::with_capacity(128);

    println!("mio step 1: poll, expect writable");
    poll.poll(&mut events, Some(Duration::from_secs(3)))?;
    for e in &events {
        println!("  event {:?} readable={} writable={} read_closed={}", e.token(), e.is_readable(), e.is_writable(), e.is_read_closed());
    }

    tx.send(()).unwrap();
    rx2.recv().unwrap();

    let infinite = std::env::args().nth(2).as_deref() == Some("infinite");
    println!("mio step 2: peer closed, poll (timeout {}), expect readable", if infinite { "none" } else { "3 s" });
    if infinite {
        // Watchdog: the real test blocks forever here if no event arrives.
        thread::spawn(|| {
            thread::sleep(Duration::from_secs(5));
            println!("  HANG: no event after 5 s with an infinite timeout");
            std::process::exit(2);
        });
    }
    poll.poll(&mut events, if infinite { None } else { Some(Duration::from_secs(3)) })?;
    if events.is_empty() {
        println!("  TIMEOUT: no event");
    }
    for e in &events {
        println!("  event {:?} readable={} writable={} read_closed={}", e.token(), e.is_readable(), e.is_writable(), e.is_read_closed());
    }
    server.join().unwrap();
    Ok(())
}
