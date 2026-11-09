fn main() {
    println!(
        "hello from Rust on {} ({}), pid {}",
        std::env::consts::OS,
        std::env::consts::ARCH,
        std::process::id()
    );
}
