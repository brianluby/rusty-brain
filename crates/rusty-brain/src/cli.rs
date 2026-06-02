//! Command-line surface for the `rusty-brain` binary (clap derive).

use clap::{value_parser, Parser, Subcommand};
use rb_types::MemoryType;

/// Parse a `--type` value into a `MemoryType` using the canonical db strings.
fn parse_memory_type(s: &str) -> Result<MemoryType, String> {
    MemoryType::parse(s).map_err(|e| e.to_string())
}

#[derive(Parser, Debug)]
#[command(
    name = "rusty-brain",
    about = "Shared semantic memory for AI agents (daemon + CLI).",
    version
)]
pub struct Cli {
    /// Emit machine-readable JSON instead of human text (where supported).
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Run the memory daemon in the foreground until Ctrl-C.
    Serve,

    /// Run the MCP (Model Context Protocol) stdio server for agents.
    Mcp,

    /// Store a new memory.
    Remember {
        /// Memory content (the body to remember).
        content: String,
        /// Memory type (db string, e.g. `insight`, `bug_fix`).
        #[arg(long = "type", default_value = "insight", value_parser = parse_memory_type)]
        memory_type: MemoryType,
        /// Importance 1-10.
        #[arg(long, default_value_t = 5, value_parser = value_parser!(u8).range(1..=10))]
        importance: u8,
        /// Optional context string.
        #[arg(long)]
        context: Option<String>,
        /// Tags (repeatable: `--tags a --tags b`).
        #[arg(long)]
        tags: Vec<String>,
    },

    /// Recall memories matching a query.
    Recall {
        /// Free-text query.
        query: String,
        /// Maximum number of results.
        #[arg(long, default_value_t = 10)]
        limit: usize,
        /// Restrict to a memory type (db string).
        #[arg(long = "type", value_parser = parse_memory_type)]
        memory_type: Option<MemoryType>,
        /// Filter by tags (repeatable).
        #[arg(long)]
        tags: Vec<String>,
    },

    /// Fetch a single memory by id.
    Get {
        /// Memory id (UUID).
        id: String,
    },

    /// List memories in the current namespace.
    List {
        /// Maximum number of results.
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Only memories with at least this importance.
        #[arg(long, value_parser = value_parser!(u8).range(1..=10))]
        min_importance: Option<u8>,
    },

    /// Show memories connected to an id by graph links.
    Graph {
        /// Memory id (UUID).
        id: String,
        /// Traversal depth.
        #[arg(long, default_value_t = 1)]
        depth: u8,
    },

    /// Soft-delete (archive) a memory.
    Delete {
        /// Memory id (UUID).
        id: String,
    },

    /// Show the project context payload (recent + important).
    Context,

    /// Stream live change notifications for the current namespace until Ctrl-C.
    Subscribe,

    /// Ping the daemon and report its contract version.
    Status,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use clap::Parser;

    #[test]
    fn parses_subscribe_subcommand() {
        let cli = Cli::parse_from(["rusty-brain", "subscribe"]);
        assert!(
            matches!(cli.command, Command::Subscribe),
            "`rusty-brain subscribe` must parse to Command::Subscribe"
        );
    }

    #[test]
    fn parses_subscribe_with_global_json_flag() {
        let cli = Cli::parse_from(["rusty-brain", "--json", "subscribe"]);
        assert!(cli.json, "--json is a global flag and applies to subscribe");
        assert!(matches!(cli.command, Command::Subscribe));
    }
}
