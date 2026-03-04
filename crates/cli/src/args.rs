//! CLI argument definitions using clap derive macros.

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use types::ObservationType;

#[derive(Parser)]
#[command(
    name = "rusty-brain",
    about = "Query your AI agent's memory",
    version,
    arg_required_else_help = true
)]
pub struct Cli {
    /// Path to memory file (overrides auto-detection)
    #[arg(long, global = true)]
    pub memory_path: Option<PathBuf>,

    /// Enable verbose debug output to stderr
    #[arg(short, long, global = true)]
    pub verbose: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Search memories by text pattern
    Find {
        /// Search pattern
        #[arg(value_parser = parse_pattern)]
        pattern: String,
        /// Maximum results
        #[arg(long, default_value_t = 10, value_parser = parse_limit)]
        limit: usize,
        /// Filter by observation type
        #[arg(long, value_parser = parse_obs_type)]
        r#type: Option<ObservationType>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Ask a question about your memory
    Ask {
        /// Natural language question
        #[arg(value_parser = parse_question)]
        question: String,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// View memory statistics
    Stats {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// View chronological timeline
    Timeline {
        /// Maximum entries
        #[arg(long, default_value_t = 10, value_parser = parse_limit)]
        limit: usize,
        /// Filter by observation type
        #[arg(long, value_parser = parse_obs_type)]
        r#type: Option<ObservationType>,
        /// Show oldest entries first
        #[arg(long)]
        oldest_first: bool,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
}

impl Command {
    /// Whether `--json` output was requested for this subcommand.
    pub fn json(&self) -> bool {
        match self {
            Self::Find { json, .. }
            | Self::Ask { json, .. }
            | Self::Stats { json, .. }
            | Self::Timeline { json, .. } => *json,
        }
    }
}

/// Parse and validate search pattern (must be non-empty after trimming).
fn parse_pattern(s: &str) -> Result<String, String> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Err("pattern must not be empty".to_string());
    }
    Ok(trimmed.to_string())
}

/// Parse and validate question (must be non-empty after trimming).
fn parse_question(s: &str) -> Result<String, String> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Err("question must not be empty".to_string());
    }
    Ok(trimmed.to_string())
}

/// Parse limit as a positive integer (>= 1).
fn parse_limit(s: &str) -> Result<usize, String> {
    let n: usize = s
        .parse()
        .map_err(|_| format!("invalid value '{s}' for '--limit': not a valid integer"))?;
    if n == 0 {
        return Err("invalid value '0' for '--limit': must be at least 1".to_string());
    }
    Ok(n)
}

/// Parse observation type from string (case-insensitive).
/// On error, lists all valid type names.
fn parse_obs_type(s: &str) -> Result<ObservationType, String> {
    s.parse::<ObservationType>().map_err(|_| {
        format!(
            "invalid observation type '{s}'; valid types: discovery, decision, problem, \
             solution, pattern, warning, success, refactor, bugfix, feature"
        )
    })
}
