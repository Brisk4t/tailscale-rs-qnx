//! rustix smoke test for x86_64-pc-qnx: the one path tailscale-rs uses (gethostname ->
//! rustix::system::uname), compared against the target's own `hostname` / `uname` commands.

use std::process::Command;

fn cmd(args: &[&str]) -> Result<String, String> {
    let out = Command::new(args[0]).args(&args[1..]).output().map_err(|e| format!("{args:?}: {e}"))?;
    if !out.status.success() {
        return Err(format!("{args:?} exited {}", out.status));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn main() {
    let mut failed = 0;
    let mut check = |name: &str, got: String, want: Result<String, String>| match want {
        Ok(want) if got == want => println!("PASS {name:<16} {got:?}"),
        Ok(want) => {
            failed += 1;
            println!("FAIL {name:<16} got {got:?}, system says {want:?}");
        }
        Err(e) => {
            failed += 1;
            println!("FAIL {name:<16} couldn't run reference command: {e}");
        }
    };

    let host = gethostname::gethostname().into_string().unwrap_or_else(|s| format!("<non-utf8 {s:?}>"));
    check("gethostname", host, cmd(&["hostname"]));

    let u = rustix::system::uname();
    let s = |c: &std::ffi::CStr| c.to_string_lossy().into_owned();
    check("uname sysname", s(u.sysname()), cmd(&["uname", "-s"]));
    check("uname nodename", s(u.nodename()), cmd(&["uname", "-n"]));
    check("uname release", s(u.release()), cmd(&["uname", "-r"]));
    check("uname machine", s(u.machine()), cmd(&["uname", "-m"]));

    println!("5 checks, {failed} failures");
    std::process::exit(if failed == 0 { 0 } else { 1 });
}
