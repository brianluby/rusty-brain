/// Top-level CLI arguments parsed by clap.
#[derive(clap::Parser)]
#[command(name = "rusty-brain", about = "Memory hooks for Claude Code")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

/// Subcommands corresponding to Claude Code hook events.
#[derive(clap::Subcommand)]
pub enum Command {
    /// Initialize memory and inject context at session start
    SessionStart,
    /// Capture tool observations after tool execution
    PostToolUse,
    /// Generate session summary and gracefully shut down
    Stop,
    /// Track installation version state
    SmartInstall,
}
