//! `rusty-brain` binary entry point.
//!
//! Stub for the P1 setup cluster: prints version and a not-yet-implemented
//! notice, then exits success. The full clap CLI (serve/remember/recall/...)
//! is implemented in the daemon and CLI cluster.

fn main() -> std::process::ExitCode {
    println!("rusty-brain {}", env!("CARGO_PKG_VERSION"));
    eprintln!("CLI not yet implemented in this build");
    std::process::ExitCode::SUCCESS
}
