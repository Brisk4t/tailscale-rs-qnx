//! Repeat mio's `connect` / `shutdown_read` pattern (writable first, then expect readable)
//! many times. On a miss, print the poll(2) trace of that iteration.

use std::{
    io::Write,
    net,
    sync::{Mutex, mpsc::channel},
    thread,
    time::Duration,
};

use mio::{Events, Interest, Poll, Token, net::TcpStream};

static TRACE: Mutex<Vec<String>> = Mutex::new(Vec::new());

struct BufLog;
impl log::Log for BufLog {
    fn enabled(&self, _: &log::Metadata) -> bool {
        true
    }
    fn log(&self, r: &log::Record) {
        if r.target().starts_with("mio") {
            TRACE.lock().unwrap().push(format!("{}", r.args()));
        }
    }
    fn flush(&self) {}
}

/// One iteration. `echo`: peer echoes our write (shutdown_read); else peer closes (connect).
fn once(echo: bool) -> bool {
    let listener = net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = channel::<()>();
    let server = thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        if echo {
            let mut buf = [0u8; 16];
            let n = std::io::Read::read(&mut s, &mut buf).unwrap();
            s.write_all(&buf[..n]).unwrap();
            rx.recv().unwrap();
        } else {
            rx.recv().unwrap();
            drop(s);
        }
    });

    let mut poll = Poll::new().unwrap();
    let mut stream = TcpStream::connect(addr).unwrap();
    poll.registry().register(&mut stream, Token(1), Interest::READABLE | Interest::WRITABLE).unwrap();
    let mut events = Events::with_capacity(8);

    // Wait for the writable event, like the tests do.
    let mut writable = false;
    while !writable {
        poll.poll(&mut events, Some(Duration::from_secs(2))).unwrap();
        if events.is_empty() {
            break;
        }
        writable = events.iter().any(|e| e.is_writable());
    }

    if echo {
        stream.write_all(b"hello").unwrap();
    } else {
        tx.send(()).unwrap();
    }

    poll.poll(&mut events, Some(Duration::from_secs(2))).unwrap();
    let ok = events.iter().any(|e| e.is_readable());
    if echo {
        let _ = tx.send(());
    }
    server.join().unwrap();
    ok
}

pub fn run(n: usize) {
    log::set_boxed_logger(Box::new(BufLog)).unwrap();
    log::set_max_level(log::LevelFilter::Trace);
    for (name, echo) in [("connect (peer closes)", false), ("shutdown_read (peer echoes)", true)] {
        let mut misses = 0;
        for i in 0..n {
            TRACE.lock().unwrap().clear();
            if !once(echo) {
                misses += 1;
                if misses <= 2 {
                    println!("{name}: MISS on iteration {i}, poll trace:");
                    for l in TRACE.lock().unwrap().iter().filter(|l| l.contains("Poll finished")) {
                        println!("    {l}");
                    }
                }
            }
        }
        println!("{name}: {misses}/{n} iterations missed the readable event");
    }
}
