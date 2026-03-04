//! CLI scripts (find, ask, stats, timeline) for rusty-brain.

mod args;
mod commands;
mod output;

use std::fmt;
use std::path::PathBuf;

use clap::Parser;

use args::{Cli, Command};
use rusty_brain_core::mind::Mind;
use types::{MindConfig, RustyBrainError};

/// CLI-specific error type wrapping core errors with CLI-specific variants.
#[derive(Debug)]
pub enum CliError {
    Core(RustyBrainError),
    MemoryFileNotFound { path: PathBuf },
    NotAFile { path: PathBuf },
    EmptyPattern,
    Io(std::io::Error),
}

impl CliError {
    fn exit_code(&self) -> i32 {
        match self {
            Self::Core(RustyBrainError::LockTimeout { .. }) => 2,
            Self::Core(_)
            | Self::MemoryFileNotFound { .. }
            | Self::NotAFile { .. }
            | Self::EmptyPattern
            | Self::Io(_) => 1,
        }
    }

    /// Stable, machine-parseable error code for this error.
    ///
    /// Core errors delegate to [`RustyBrainError::code()`]. CLI-specific
    /// variants use `E_CLI_*` prefixed codes.
    fn code(&self) -> &'static str {
        match self {
            Self::Core(e) => e.code(),
            Self::MemoryFileNotFound { .. } => "E_CLI_MEMORY_FILE_NOT_FOUND",
            Self::NotAFile { .. } => "E_CLI_NOT_A_FILE",
            Self::EmptyPattern => "E_CLI_EMPTY_PATTERN",
            Self::Io(_) => "E_CLI_IO",
        }
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Core(RustyBrainError::LockTimeout { .. }) => {
                write!(
                    f,
                    "Memory file is in use by another process. Try again shortly."
                )
            }
            Self::Core(RustyBrainError::CorruptedFile { .. }) => {
                write!(
                    f,
                    "Memory file appears corrupted. Try removing the .mv2 file and rebuilding from source."
                )
            }
            Self::Core(e) => {
                // Strip error codes from display to keep CLI output clean.
                let msg = e.to_string();
                let clean = msg
                    .strip_prefix('[')
                    .and_then(|s| s.find("] ").map(|i| &s[i + 2..]))
                    .unwrap_or(&msg);
                write!(f, "{clean}")
            }
            Self::MemoryFileNotFound { path } => {
                write!(
                    f,
                    "Memory file not found: {}\nUse --memory-path or run from a project directory.",
                    path.display()
                )
            }
            Self::NotAFile { path } => {
                write!(f, "Path is a directory, not a file: {}", path.display())
            }
            Self::EmptyPattern => {
                write!(f, "Search pattern must not be empty.")
            }
            Self::Io(e) => write!(f, "{e}"),
        }
    }
}

impl From<RustyBrainError> for CliError {
    fn from(e: RustyBrainError) -> Self {
        Self::Core(e)
    }
}

/// Returns `Ok(())` on success, or `Err((error, json_mode))` so `main()`
/// can choose between human and structured JSON error output.
fn run() -> Result<(), (CliError, bool)> {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) if e.kind() == clap::error::ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => {
            let _ = e.print();
            return Ok(());
        }
        Err(e) => e.exit(),
    };

    // Initialize tracing subscriber if verbose mode is requested.
    if cli.verbose {
        use tracing_subscriber::fmt;
        fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_writer(std::io::stderr)
            .init();
    }

    let json = cli.command.json();

    // Resolve memory path: --memory-path overrides auto-detection.
    let mut config = MindConfig::from_env().map_err(|e| (CliError::from(e), json))?;
    if let Some(path) = cli.memory_path {
        config.memory_path = path;
    }

    // Pre-validate file existence (read-only CLI should not create files).
    let path = &config.memory_path;
    if !path.exists() {
        return Err((CliError::MemoryFileNotFound { path: path.clone() }, json));
    }
    if !path.is_file() {
        return Err((CliError::NotAFile { path: path.clone() }, json));
    }

    let mind = Mind::open_read_only(config).map_err(|e| (CliError::from(e), json))?;
    let command = cli.command;

    // Dispatch subcommand under file lock for safe concurrent access.
    let mut cmd_result: Result<(), CliError> = Ok(());
    mind.with_lock(|mind| {
        cmd_result = match command {
            Command::Find {
                pattern,
                limit,
                r#type,
                json,
            } => commands::run_find(mind, &pattern, limit, r#type, json),
            Command::Ask { question, json } => commands::run_ask(mind, &question, json),
            Command::Stats { json } => commands::run_stats(mind, json),
            Command::Timeline {
                limit,
                r#type,
                oldest_first,
                json,
            } => commands::run_timeline(mind, limit, r#type, oldest_first, json),
        };
        Ok(())
    })
    .map_err(|e| (CliError::from(e), json))?;
    cmd_result.map_err(|e| (e, json))
}

fn main() {
    match run() {
        Ok(()) => {}
        Err((e, json)) => {
            if json {
                output::print_error_json(&e);
            } else {
                eprintln!("error: {e}");
            }
            std::process::exit(e.exit_code());
        }
    }
}
