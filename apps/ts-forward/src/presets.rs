//! Built-in forwards for common services, and named groups of them.

/// A well-known TCP service: forwarded from the same port on the tailnet to `127.0.0.1`.
pub struct Preset {
    pub name: &'static str,
    pub port: u16,
    pub description: &'static str,
}

pub const PRESETS: &[Preset] = &[
    Preset { name: "ssh", port: 22, description: "OpenSSH (sshd)" },
    Preset { name: "telnet", port: 23, description: "telnetd (unencrypted locally; the tailnet hop is encrypted)" },
    Preset { name: "http", port: 80, description: "HTTP server" },
    Preset { name: "https", port: 443, description: "HTTPS server" },
    Preset { name: "modbus", port: 502, description: "Modbus/TCP" },
    Preset { name: "rtsp", port: 554, description: "RTSP video streaming (control channel only)" },
    Preset { name: "mqtt", port: 1883, description: "MQTT broker" },
    Preset { name: "opcua", port: 4840, description: "OPC UA server" },
    Preset { name: "vnc", port: 5900, description: "VNC / RFB screen sharing" },
    Preset { name: "qconn", port: 8000, description: "QNX qconn target agent (VS Code / Momentics deploy and debug). NO AUTHENTICATION" },
    Preset { name: "http-alt", port: 8080, description: "HTTP on the common alternate port" },
    Preset { name: "https-alt", port: 8443, description: "HTTPS on the common alternate port" },
    Preset { name: "mqtts", port: 8883, description: "MQTT over TLS" },
    Preset { name: "prometheus", port: 9100, description: "Prometheus node exporter / metrics endpoint" },
];

/// Named groups of presets.
pub const GROUPS: &[(&str, &[&str], &str)] = &[
    ("web", &["http", "https", "http-alt", "https-alt"], "web servers"),
    ("qnx-dev", &["ssh", "qconn"], "remote shell plus QNX deploy/debug"),
    ("iot", &["mqtt", "mqtts", "opcua", "modbus"], "industrial / IoT protocols"),
    ("all", &[], "every preset"),
];

/// Presets used when no `--preset` or `--forward` is given.
pub const DEFAULT: &[&str] = &["ssh"];

pub fn find(name: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.name == name)
}

/// Expand a preset or group name into presets.
pub fn expand(name: &str) -> Result<Vec<&'static Preset>, String> {
    if let Some(p) = find(name) {
        return Ok(vec![p]);
    }
    match GROUPS.iter().find(|(g, _, _)| *g == name) {
        Some(("all", _, _)) => Ok(PRESETS.iter().collect()),
        Some((_, members, _)) => Ok(members.iter().map(|m| find(m).expect("group member is a preset")).collect()),
        None => Err(format!("unknown preset `{name}` (see --list-presets)")),
    }
}

pub fn print_list() {
    println!("Presets (tailnet port -> 127.0.0.1:same port):");
    for p in PRESETS {
        println!("  {:<11} {:>5}  {}", p.name, p.port, p.description);
    }
    println!("\nGroups:");
    for (g, members, desc) in GROUPS {
        if members.is_empty() {
            println!("  {g:<11} {desc}");
        } else {
            println!("  {g:<11} {}  ({desc})", members.join(", "));
        }
    }
    println!("\nDefault with no --preset/--forward: {}", DEFAULT.join(", "));
}
