//! Command-line surface for the `rusty-brain` binary (clap derive).

use clap::{value_parser, Parser, Subcommand};
use rb_types::{FeedbackKind, LinkType, MemoryType};

/// Parse a `--type` value into a `MemoryType` using the canonical db strings.
fn parse_memory_type(s: &str) -> Result<MemoryType, String> {
    MemoryType::parse(s).map_err(|e| e.to_string())
}

/// Parse a feedback `--kind` value into a `FeedbackKind` (helpful|wrong|stale).
fn parse_feedback_kind(s: &str) -> Result<FeedbackKind, String> {
    FeedbackKind::parse(s).map_err(|e| e.to_string())
}

/// Parse a link `--type` value into a `LinkType` using the canonical db strings.
/// `supersedes` is rejected here — that edge is created by storing a
/// replacement memory, not by linking — so `rusty-brain link --type supersedes`
/// fails locally instead of round-tripping to a daemon rejection.
fn parse_link_type(s: &str) -> Result<LinkType, String> {
    let link_type = LinkType::parse(s).map_err(|e| e.to_string())?;
    if link_type == LinkType::Supersedes {
        return Err(
            "supersedes links are created by storing a replacement memory, not by linking"
                .to_string(),
        );
    }
    Ok(link_type)
}

/// Clap range check for `--confidence` (inclusive 0.0..=1.0, finite).
fn parse_confidence(s: &str) -> Result<f32, String> {
    let v: f32 = s.parse().map_err(|e| format!("not a number: {e}"))?;
    rb_types::validate_confidence(v).map_err(|e| e.to_string())?;
    Ok(v)
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
        /// Supersede an existing memory: store this as the replacement and
        /// archive the prior memory in one atomic op (the `supersedes` edge —
        /// see `link`, which rejects `--type supersedes` for this reason). Value
        /// is the UUID of the memory being replaced.
        #[arg(long)]
        supersedes: Option<String>,
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

    /// Apply a partial update to a memory (only provided fields change).
    /// Content cannot be updated — store a new memory so embeddings stay
    /// consistent.
    Update {
        /// Memory id (UUID).
        id: String,
        /// Replacement summary.
        #[arg(long)]
        summary: Option<String>,
        /// Importance 1-10.
        #[arg(long, value_parser = value_parser!(u8).range(1..=10))]
        importance: Option<u8>,
        /// Replacement tags (repeatable: `--tags a --tags b`).
        #[arg(long)]
        tags: Vec<String>,
        /// Replacement context string.
        #[arg(long)]
        context: Option<String>,
        /// Trust prior 0.0-1.0 (W2.2: e.g. lower it on a memory you no longer
        /// fully trust).
        #[arg(long, value_parser = parse_confidence)]
        confidence: Option<f32>,
    },

    /// Link two memories (e.g. mark one as contradicting another). Both sides
    /// of an active `contradicts` link surface `contested` on reads.
    Link {
        /// Source memory id (UUID).
        from: String,
        /// Target memory id (UUID).
        to: String,
        /// Link type: `contradicts`, `extends`, `implements`, or `references`.
        #[arg(long = "type", value_parser = parse_link_type)]
        link_type: LinkType,
        /// Why this link exists (stored on the edge).
        #[arg(long)]
        reason: Option<String>,
    },

    /// Record a usefulness signal about a memory (W3.7): `helpful`, `wrong`, or
    /// `stale`. Nudges the memory's trust prior so future recalls improve.
    Feedback {
        /// Memory id (UUID) the feedback is about.
        id: String,
        /// Feedback kind: `helpful`, `wrong`, or `stale`.
        #[arg(long = "kind", value_parser = parse_feedback_kind)]
        kind: FeedbackKind,
    },

    /// Soft-delete (archive) a memory.
    Delete {
        /// Memory id (UUID).
        id: String,
    },

    /// Show the project context payload (recent + important).
    Context,

    /// Stream live change notifications for the current namespace until Ctrl-C.
    Subscribe {
        /// Resume from this oplog cursor: changes with seq > SINCE are
        /// replayed from the durable log before live streaming begins, so a
        /// reconnecting consumer misses nothing. Each streamed change carries
        /// its `seq`; track the max you have seen.
        #[arg(long)]
        since: Option<u64>,
    },

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

    /// Retroactively redact secrets from every stored memory (W2.4). Rewrites
    /// content/summary/context in place and marks affected rows for
    /// re-embedding; follow with `rusty-brain reembed` until changed=0. Admin
    /// op: only a client running as the daemon's own user may invoke it.
    Scrub,

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
    /// unless `--merge` is passed. Restart any active agent sessions
    /// afterwards: sessions resolve their namespace once at connect, so one
    /// opened before the rename keeps writing to the old namespace (re-run
    /// with `--merge` to sweep up stragglers).
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
            matches!(cli.command, Command::Subscribe { since: None }),
            "`rusty-brain subscribe` must parse to Command::Subscribe"
        );
    }

    #[test]
    fn subscribe_accepts_a_since_cursor() {
        let cli = Cli::parse_from(["rusty-brain", "subscribe", "--since", "42"]);
        assert!(
            matches!(cli.command, Command::Subscribe { since: Some(42) }),
            "--since must parse as the oplog cursor"
        );
    }

    #[test]
    fn parses_subscribe_with_global_json_flag() {
        let cli = Cli::parse_from(["rusty-brain", "--json", "subscribe"]);
        assert!(cli.json, "--json is a global flag and applies to subscribe");
        assert!(matches!(cli.command, Command::Subscribe { .. }));
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
    fn update_parses_partial_fields_including_confidence() {
        let cli = Cli::parse_from([
            "rusty-brain",
            "update",
            "0c8e7f76-3a4f-4f7e-9d3a-111111111111",
            "--importance",
            "9",
            "--confidence",
            "0.3",
        ]);
        match cli.command {
            Command::Update {
                id,
                summary,
                importance,
                tags,
                context,
                confidence,
            } => {
                assert_eq!(id, "0c8e7f76-3a4f-4f7e-9d3a-111111111111");
                assert_eq!(summary, None);
                assert_eq!(importance, Some(9));
                assert!(tags.is_empty());
                assert_eq!(context, None);
                assert_eq!(confidence, Some(0.3));
            }
            other => panic!("expected Update, got {other:?}"),
        }
    }

    #[test]
    fn update_rejects_out_of_range_confidence_at_parse_time() {
        for bad in ["-0.1", "1.5", "NaN"] {
            let res =
                Cli::try_parse_from(["rusty-brain", "update", "some-id", "--confidence", bad]);
            assert!(res.is_err(), "confidence {bad} must fail to parse");
        }
    }

    #[test]
    fn link_parses_ids_type_and_reason() {
        let cli = Cli::parse_from([
            "rusty-brain",
            "link",
            "0c8e7f76-3a4f-4f7e-9d3a-111111111111",
            "0c8e7f76-3a4f-4f7e-9d3a-222222222222",
            "--type",
            "contradicts",
            "--reason",
            "team reversed the decision",
        ]);
        match cli.command {
            Command::Link {
                from,
                to,
                link_type,
                reason,
            } => {
                assert_eq!(from, "0c8e7f76-3a4f-4f7e-9d3a-111111111111");
                assert_eq!(to, "0c8e7f76-3a4f-4f7e-9d3a-222222222222");
                assert_eq!(link_type, LinkType::Contradicts);
                assert_eq!(reason.as_deref(), Some("team reversed the decision"));
            }
            other => panic!("expected Link, got {other:?}"),
        }
    }

    #[test]
    fn link_rejects_supersedes_locally() {
        // supersede is its own atomic op; the CLI must fail before the daemon.
        let res = Cli::try_parse_from(["rusty-brain", "link", "a", "b", "--type", "supersedes"]);
        assert!(
            res.is_err(),
            "--type supersedes must be rejected at parse time"
        );
    }

    #[test]
    fn remember_accepts_supersedes_id() {
        // The supersede edge IS created here (the replacement-memory path that
        // `link --type supersedes` points at). Default: no prior.
        let plain = Cli::parse_from(["rusty-brain", "remember", "a new decision"]);
        match plain.command {
            Command::Remember { supersedes, .. } => assert_eq!(supersedes, None),
            other => panic!("expected Remember, got {other:?}"),
        }
        let old = "0c8e7f76-3a4f-4f7e-9d3a-111111111111";
        let cli = Cli::parse_from([
            "rusty-brain",
            "remember",
            "the replacement",
            "--supersedes",
            old,
        ]);
        match cli.command {
            Command::Remember { supersedes, .. } => assert_eq!(supersedes.as_deref(), Some(old)),
            other => panic!("expected Remember, got {other:?}"),
        }
    }

    #[test]
    fn link_requires_an_explicit_type() {
        let res = Cli::try_parse_from(["rusty-brain", "link", "a-id", "b-id"]);
        assert!(res.is_err(), "--type is required for link");
    }

    #[test]
    fn feedback_parses_id_and_kind() {
        let id = "0c8e7f76-3a4f-4f7e-9d3a-111111111111";
        let cli = Cli::parse_from(["rusty-brain", "feedback", id, "--kind", "wrong"]);
        match cli.command {
            Command::Feedback { id: got, kind } => {
                assert_eq!(got, id);
                assert_eq!(kind, FeedbackKind::Wrong);
            }
            other => panic!("expected Feedback, got {other:?}"),
        }
    }

    #[test]
    fn feedback_rejects_unknown_kind_and_requires_it() {
        // An invalid --kind fails at parse time (value_parser = parse_feedback_kind).
        let res = Cli::try_parse_from(["rusty-brain", "feedback", "an-id", "--kind", "useless"]);
        assert!(
            res.is_err(),
            "unknown --kind must be rejected at parse time"
        );
        // --kind is required.
        let res = Cli::try_parse_from(["rusty-brain", "feedback", "an-id"]);
        assert!(res.is_err(), "--kind is required for feedback");
    }

    #[test]
    fn parses_scrub_subcommand() {
        let cli = Cli::parse_from(["rusty-brain", "scrub"]);
        assert!(
            matches!(cli.command, Command::Scrub),
            "`rusty-brain scrub` must parse to Command::Scrub"
        );
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
