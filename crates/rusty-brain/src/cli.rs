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

    /// Use this namespace instead of detecting one (env equivalent:
    /// `RUSTY_BRAIN_NAMESPACE`; explicit always wins).
    #[arg(long, global = true)]
    pub namespace: Option<String>,

    /// Accept and pin this repo's `CLAUDE.md` frontmatter `project:` namespace
    /// override (recorded per-directory in the local pin store; later runs
    /// honor it without warning).
    #[arg(long, global = true)]
    pub accept_namespace_override: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Run the memory daemon in the foreground until Ctrl-C.
    Serve {
        /// Path to the evolution-jobs TOML config (else `RB_JOBS_CONFIG`, else
        /// all jobs disabled).
        #[arg(long = "jobs-config", env = "RB_JOBS_CONFIG")]
        jobs_config: Option<String>,

        /// Accept a changed embedding model: adopt the configured model and
        /// mark the whole corpus for re-embedding (run `rusty-brain reembed`
        /// until changed=0). Without this, a model swap refuses to start.
        /// Env equivalent (for auto-start): `RB_ACCEPT_MODEL_CHANGE`.
        #[arg(long = "accept-model-change")]
        accept_model_change: bool,
    },

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

    /// Trigger one bounded evolution-job pass on the running daemon.
    Evolve {
        /// Which job to run: `link_decay`, `consolidation`, or
        /// `importance_recalibration`.
        job: String,
    },

    /// Re-embed active memories whose vector is built from a stale composition
    /// (P5 Feature A). Bounded and idempotent: a second run over unchanged data
    /// reports `changed=0`. Re-run until `changed=0` to converge a large corpus.
    Reembed {
        /// Maximum memories to scan/re-embed in this pass (else the daemon
        /// batch default).
        #[arg(long)]
        limit: Option<usize>,
    },

    /// Namespace administration (data-lifecycle helpers).
    Namespace {
        #[command(subcommand)]
        command: NamespaceCommand,
    },
}

#[derive(Subcommand, Debug)]
pub enum NamespaceCommand {
    /// One-time rename: re-scope every memory (active and archived) from OLD
    /// to NEW in one daemon transaction — memories, the vector-index partition
    /// and a durable oplog record move together. Use after pinning a repo's
    /// identity in `.rusty-brain.toml` to re-scope memories captured under the
    /// heuristic directory-name namespace. Refuses a non-empty NEW namespace
    /// unless `--merge` is passed.
    Rename {
        /// Source namespace: a bare project name (`my-proj`) or a full
        /// namespace string (`project:my-proj`, `global`, `session:proj:sid`).
        old: String,
        /// Target namespace (same forms as OLD).
        new: String,
        /// Append into a NEW namespace that already has memories instead of
        /// refusing; the combined counts are reported and logged.
        #[arg(long)]
        merge: bool,
    },
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
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

    #[test]
    fn namespace_flag_is_global_and_defaults_off() {
        let cli = Cli::parse_from(["rusty-brain", "status"]);
        assert_eq!(cli.namespace, None);
        assert!(!cli.accept_namespace_override);
        // Global: accepted after the subcommand too.
        let cli = Cli::parse_from(["rusty-brain", "status", "--namespace", "my-proj"]);
        assert_eq!(cli.namespace.as_deref(), Some("my-proj"));
    }

    #[test]
    fn accept_namespace_override_flag_parses() {
        let cli = Cli::parse_from(["rusty-brain", "--accept-namespace-override", "status"]);
        assert!(cli.accept_namespace_override);
    }

    #[test]
    fn evolve_parses_link_decay_job() {
        let cli = Cli::parse_from(["rusty-brain", "evolve", "link_decay"]);
        match cli.command {
            Command::Evolve { job } => assert_eq!(job, "link_decay"),
            other => panic!("expected Evolve, got {other:?}"),
        }
    }

    #[test]
    fn reembed_parses_with_optional_limit() {
        let cli = Cli::parse_from(["rusty-brain", "reembed"]);
        match cli.command {
            Command::Reembed { limit } => assert_eq!(limit, None),
            other => panic!("expected Reembed, got {other:?}"),
        }
        let cli = Cli::parse_from(["rusty-brain", "reembed", "--limit", "250"]);
        match cli.command {
            Command::Reembed { limit } => assert_eq!(limit, Some(250)),
            other => panic!("expected Reembed, got {other:?}"),
        }
    }

    #[test]
    fn namespace_rename_parses_old_new_and_defaults_merge_off() {
        let cli = Cli::parse_from(["rusty-brain", "namespace", "rename", "scratch", "rb"]);
        match cli.command {
            Command::Namespace {
                command: NamespaceCommand::Rename { old, new, merge },
            } => {
                assert_eq!(old, "scratch");
                assert_eq!(new, "rb");
                assert!(!merge, "--merge defaults off (refuse-on-collision)");
            }
            other => panic!("expected Namespace Rename, got {other:?}"),
        }
    }

    #[test]
    fn namespace_rename_accepts_merge_flag_and_db_string_forms() {
        let cli = Cli::parse_from([
            "rusty-brain",
            "namespace",
            "rename",
            "project:scratch",
            "global",
            "--merge",
        ]);
        match cli.command {
            Command::Namespace {
                command: NamespaceCommand::Rename { old, new, merge },
            } => {
                assert_eq!(old, "project:scratch");
                assert_eq!(new, "global");
                assert!(merge);
            }
            other => panic!("expected Namespace Rename, got {other:?}"),
        }
    }

    #[test]
    fn serve_accepts_jobs_config_flag() {
        let cli = Cli::parse_from(["rusty-brain", "serve", "--jobs-config", "/tmp/jobs.toml"]);
        match cli.command {
            Command::Serve {
                jobs_config,
                accept_model_change,
            } => {
                assert_eq!(jobs_config.as_deref(), Some("/tmp/jobs.toml"));
                assert!(!accept_model_change, "opt-in flag defaults to off");
            }
            other => panic!("expected Serve, got {other:?}"),
        }
    }

    #[test]
    fn serve_accepts_the_accept_model_change_flag() {
        let cli = Cli::parse_from(["rusty-brain", "serve", "--accept-model-change"]);
        match cli.command {
            Command::Serve {
                accept_model_change,
                ..
            } => assert!(accept_model_change),
            other => panic!("expected Serve, got {other:?}"),
        }
    }
}
