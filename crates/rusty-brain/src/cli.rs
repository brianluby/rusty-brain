//! Command-line surface for the `rusty-brain` binary (clap derive).

use clap::{value_parser, Parser, Subcommand};
use rb_types::{FeedbackKind, LinkType, MemoryType};

/// Parse a `--type` value into a `MemoryType` using the canonical db strings.
fn parse_memory_type(s: &str) -> Result<MemoryType, String> {
    MemoryType::parse(s).map_err(|e| e.to_string())
}

/// Parse `--format` for export.
fn parse_export_format(s: &str) -> Result<crate::export::ExportFormat, String> {
    match s.to_ascii_lowercase().as_str() {
        "md" | "markdown" => Ok(crate::export::ExportFormat::Markdown),
        "json" => Ok(crate::export::ExportFormat::Json),
        "csv" => Ok(crate::export::ExportFormat::Csv),
        _ => Err(format!(
            "unknown format '{s}'; expected markdown, json, or csv"
        )),
    }
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

/// Parse a `--since`/`--until` bound: an RFC 3339 timestamp
/// (`2026-07-10T12:00:00Z`), a date (`2026-07-01`, midnight UTC), or a
/// now-relative age (`7d`, `36h`, `45m`, `10s`).
fn parse_time_bound(s: &str) -> Result<chrono::DateTime<chrono::Utc>, String> {
    if let Ok(ts) = chrono::DateTime::parse_from_rfc3339(s) {
        return Ok(ts.with_timezone(&chrono::Utc));
    }
    if let Ok(date) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        let midnight = date
            .and_hms_opt(0, 0, 0)
            .ok_or_else(|| format!("invalid date '{s}'"))?;
        return Ok(chrono::DateTime::from_naive_utc_and_offset(
            midnight,
            chrono::Utc,
        ));
    }
    // Relative age: <number><unit> with unit d/h/m/s. The try_* constructors
    // (chrono 0.4.44) are the FALLIBLE forms — `Duration::days` etc. panic on
    // out-of-range values, and argv must never be able to panic the process.
    if let Some((digits, unit)) = s.split_at_checked(s.len().saturating_sub(1)) {
        if let Ok(n) = digits.parse::<i64>() {
            let duration = match unit {
                "d" => Some(chrono::TimeDelta::try_days(n)),
                "h" => Some(chrono::TimeDelta::try_hours(n)),
                "m" => Some(chrono::TimeDelta::try_minutes(n)),
                "s" => Some(chrono::TimeDelta::try_seconds(n)),
                _ => None,
            };
            if let Some(duration) = duration {
                if n < 0 {
                    return Err(format!("relative age '{s}' must be non-negative"));
                }
                let duration =
                    duration.ok_or_else(|| format!("relative age '{s}' is out of range"))?;
                return Ok(chrono::Utc::now() - duration);
            }
        }
    }
    Err(format!(
        "invalid time bound '{s}': expected RFC 3339 (2026-07-10T12:00:00Z), a date \
         (2026-07-01), or a relative age (7d, 36h, 45m, 10s)"
    ))
}

/// Parse a `remember --file` anchor spec: `PATH`, `PATH:LINE`, or
/// `PATH:START-END` (1-based, inclusive). Garbage ranges (`a.rs:12-`,
/// `a.rs:0`, inverted, overflow) fail at parse time — argv must never panic
/// the process (the `parse_time_bound` precedent).
fn parse_file_anchor(s: &str) -> Result<rb_types::MemoryAnchor, String> {
    rb_types::MemoryAnchor::parse_file_spec(s).map_err(|e| e.to_string())
}

/// Parse a `remember --commit` anchor value (a commit SHA string).
fn parse_commit_anchor(s: &str) -> Result<rb_types::MemoryAnchor, String> {
    rb_types::MemoryAnchor::new(rb_types::AnchorKind::Commit, s).map_err(|e| e.to_string())
}

/// Parse a `remember --symbol` anchor value (a symbol/identifier string).
fn parse_symbol_anchor(s: &str) -> Result<rb_types::MemoryAnchor, String> {
    rb_types::MemoryAnchor::new(rb_types::AnchorKind::Symbol, s).map_err(|e| e.to_string())
}

/// Parse a recall/list `--file` FILTER (path only — filters match by path,
/// so a `:LINE` range is a clean parse error pointing at capture).
fn parse_file_filter(s: &str) -> Result<rb_types::AnchorFilter, String> {
    rb_types::parse_file_filter(s).map_err(|e| e.to_string())
}

/// Parse a recall/list `--commit` FILTER value.
fn parse_commit_filter(s: &str) -> Result<rb_types::AnchorFilter, String> {
    let filter = rb_types::AnchorFilter {
        kind: rb_types::AnchorKind::Commit,
        value: s.to_string(),
    };
    filter.validate().map_err(|e| e.to_string())?;
    Ok(filter)
}

/// Parse a recall/list `--symbol` FILTER value.
fn parse_symbol_filter(s: &str) -> Result<rb_types::AnchorFilter, String> {
    let filter = rb_types::AnchorFilter {
        kind: rb_types::AnchorKind::Symbol,
        value: s.to_string(),
    };
    filter.validate().map_err(|e| e.to_string())?;
    Ok(filter)
}

/// Parse a `--source` producer surface; the daemon only ever stamps these four.
fn parse_source(s: &str) -> Result<String, String> {
    const SOURCES: [&str; 4] = ["hook", "mcp", "cli", "job"];
    if SOURCES.contains(&s) {
        Ok(s.to_string())
    } else {
        Err(format!(
            "unknown source '{s}': expected one of hook, mcp, cli, job"
        ))
    }
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

    /// Seed memory from existing project context (first-run cold start).
    Init {
        /// Skip the confirmation prompt and store the planned memories.
        #[arg(long, short = 'y')]
        yes: bool,
        /// Print the import plan without storing.
        #[arg(long)]
        dry_run: bool,
        /// Maximum project files to scan (well-known root files + docs/).
        #[arg(long, default_value_t = 50)]
        max_files: usize,
        /// Maximum bytes to read from any one source file.
        #[arg(long, default_value_t = 65_536)]
        max_bytes: usize,
        /// Default importance 1-10 for seeded memories (source adapters may
        /// nudge it, e.g. CLAUDE.md constraints are one point higher).
        #[arg(long, default_value_t = 6, value_parser = value_parser!(u8).range(1..=10))]
        importance: u8,
        /// Undo a prior import batch by id (printed after a successful init/import).
        #[arg(
            long,
            conflicts_with_all = ["yes", "dry_run", "max_files", "max_bytes", "importance", "list_batches"]
        )]
        undo: Option<String>,
        /// List undoable import batches for the current database.
        #[arg(
            long = "list-batches",
            conflicts_with_all = ["yes", "dry_run", "max_files", "max_bytes", "importance", "undo"]
        )]
        list_batches: bool,
    },

    /// Import a text/markdown file, or '-' for stdin, into the current namespace.
    Import {
        /// Path to a text/markdown file, or '-' to read stdin.
        path: String,
        /// Memory type applied to the imported item.
        #[arg(long = "type", default_value = "insight", value_parser = parse_memory_type)]
        memory_type: MemoryType,
        /// Importance 1-10.
        #[arg(long, default_value_t = 5, value_parser = value_parser!(u8).range(1..=10))]
        importance: u8,
        /// Tags (repeatable). An `import_batch:<id>` tag is added automatically.
        #[arg(long)]
        tags: Vec<String>,
        /// Print the import plan without storing.
        #[arg(long)]
        dry_run: bool,
        /// Maximum bytes to read from the input.
        #[arg(long, default_value_t = 65_536)]
        max_bytes: usize,
    },

    /// Export memories to stdout. CSV omits the content body (metadata only);
    /// use markdown or json for full-fidelity dumps.
    Export {
        /// Output format: markdown, json, or csv.
        #[arg(long, default_value = "markdown", value_parser = parse_export_format)]
        format: crate::export::ExportFormat,
        /// Filter by memory type.
        #[arg(long = "type", value_parser = parse_memory_type)]
        memory_type: Option<MemoryType>,
        /// Filter by tags (repeatable; all must be present).
        #[arg(long)]
        tags: Vec<String>,
        /// Minimum importance 1-10.
        #[arg(long, value_parser = value_parser!(u8).range(1..=10))]
        min_importance: Option<u8>,
    },

    /// Write a timestamped backup snapshot to the data dir.
    Backup {
        /// Backup format (default json for full fidelity).
        #[arg(long, default_value = "json", value_parser = parse_export_format)]
        format: crate::export::ExportFormat,
        /// Keep only the N most recent backups (prune older ones).
        #[arg(long)]
        retention: Option<usize>,
        /// List existing backups instead of creating one.
        #[arg(long = "list")]
        list: bool,
    },

    /// Restore memories from a JSON export file or stdin.
    Restore {
        /// Path to a JSON export file, or '-' for stdin.
        path: String,
        /// Tags to add to restored memories (repeatable).
        #[arg(long)]
        tags: Vec<String>,
        /// Preview what would be restored without storing.
        #[arg(long)]
        dry_run: bool,
    },

    /// Store a new memory.
    Remember {
        /// Memory content (the body to remember). Required UNLESS `--batch` is
        /// set, in which case facts are read from stdin instead. (The
        /// content-vs-batch conflict is declared once on `batch` below.)
        #[arg(required_unless_present = "batch")]
        content: Option<String>,
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
        /// Anchor this memory to a file (repeatable): PATH, PATH:LINE, or
        /// PATH:START-END (1-based, inclusive). Recall can then filter with
        /// `recall --file PATH`.
        #[arg(long = "file", value_parser = parse_file_anchor)]
        file: Vec<rb_types::MemoryAnchor>,
        /// Anchor this memory to a commit SHA (repeatable).
        #[arg(long = "commit", value_parser = parse_commit_anchor)]
        commit: Vec<rb_types::MemoryAnchor>,
        /// Anchor this memory to a symbol/identifier, e.g. `Foo::bar`
        /// (repeatable). A caller-supplied string, not resolved AST.
        #[arg(long = "symbol", value_parser = parse_symbol_anchor)]
        symbol: Vec<rb_types::MemoryAnchor>,
        /// Bulk mode: read one fact per line from stdin and store them all over
        /// a SINGLE daemon connection. The `--type`/`--importance`/`--tags`/
        /// `--context` flags apply uniformly to every fact; blank lines are
        /// skipped. This avoids one process spawn + handshake per fact when
        /// planting large corpora (e.g. retrieval-at-scale evals). Incompatible
        /// with a positional content argument and with `--supersedes` (both
        /// conflicts declared here).
        #[arg(long, conflicts_with_all = ["supersedes", "content"])]
        batch: bool,
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
        /// Filter by tags (repeatable; all must be present).
        #[arg(long)]
        tags: Vec<String>,
        /// Only memories with at least this importance (1-10).
        #[arg(long, value_parser = value_parser!(u8).range(1..=10))]
        min_importance: Option<u8>,
        /// Only memories with at most this importance (1-10).
        #[arg(long, value_parser = value_parser!(u8).range(1..=10))]
        max_importance: Option<u8>,
        /// Only memories with at least this confidence (0.0-1.0).
        #[arg(long, value_parser = parse_confidence)]
        min_confidence: Option<f32>,
        /// Only memories with at most this confidence (0.0-1.0).
        #[arg(long, value_parser = parse_confidence)]
        max_confidence: Option<f32>,
        /// Only memories created at/after this bound: RFC 3339, a date, or a
        /// relative age (7d, 36h).
        #[arg(long, value_parser = parse_time_bound)]
        since: Option<chrono::DateTime<chrono::Utc>>,
        /// Only memories created at/before this bound (same forms as --since).
        #[arg(long, value_parser = parse_time_bound)]
        until: Option<chrono::DateTime<chrono::Utc>>,
        /// Only memories written by this producer surface: hook, mcp, cli, or
        /// job (repeatable; any listed source matches).
        #[arg(long = "source", value_parser = parse_source)]
        source: Vec<String>,
        /// Only contested memories (`--contested`), or only uncontested ones
        /// (`--contested false`). Absent: no constraint.
        #[arg(long, num_args = 0..=1, default_missing_value = "true")]
        contested: Option<bool>,
        /// Search archived memories instead of active ones (keyword channel
        /// only: archived vectors are pruned).
        #[arg(long)]
        archived: bool,
        /// Only memories anchored to this file path (repeatable; every
        /// listed anchor must match). Path only — line ranges are for
        /// capture (`remember --file`).
        #[arg(long = "file", value_parser = parse_file_filter)]
        file: Vec<rb_types::AnchorFilter>,
        /// Only memories anchored to this commit SHA (repeatable).
        #[arg(long = "commit", value_parser = parse_commit_filter)]
        commit: Vec<rb_types::AnchorFilter>,
        /// Only memories anchored to this symbol (repeatable).
        #[arg(long = "symbol", value_parser = parse_symbol_filter)]
        symbol: Vec<rb_types::AnchorFilter>,
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
        /// Restrict to a memory type (db string).
        #[arg(long = "type", value_parser = parse_memory_type)]
        memory_type: Option<MemoryType>,
        /// Filter by tags (repeatable; all must be present).
        #[arg(long)]
        tags: Vec<String>,
        /// Only memories with at least this importance (1-10).
        #[arg(long, value_parser = value_parser!(u8).range(1..=10))]
        min_importance: Option<u8>,
        /// Only memories with at most this importance (1-10).
        #[arg(long, value_parser = value_parser!(u8).range(1..=10))]
        max_importance: Option<u8>,
        /// Only memories with at least this confidence (0.0-1.0).
        #[arg(long, value_parser = parse_confidence)]
        min_confidence: Option<f32>,
        /// Only memories with at most this confidence (0.0-1.0).
        #[arg(long, value_parser = parse_confidence)]
        max_confidence: Option<f32>,
        /// Only memories created at/after this bound: RFC 3339, a date, or a
        /// relative age (7d, 36h).
        #[arg(long, value_parser = parse_time_bound)]
        since: Option<chrono::DateTime<chrono::Utc>>,
        /// Only memories created at/before this bound (same forms as --since).
        #[arg(long, value_parser = parse_time_bound)]
        until: Option<chrono::DateTime<chrono::Utc>>,
        /// Only memories written by this producer surface: hook, mcp, cli, or
        /// job (repeatable; any listed source matches).
        #[arg(long = "source", value_parser = parse_source)]
        source: Vec<String>,
        /// Only contested memories (`--contested`), or only uncontested ones
        /// (`--contested false`). Absent: no constraint.
        #[arg(long, num_args = 0..=1, default_missing_value = "true")]
        contested: Option<bool>,
        /// List archived memories instead of active ones.
        #[arg(long)]
        archived: bool,
        /// Only memories anchored to this file path (repeatable; every
        /// listed anchor must match). Path only — line ranges are for
        /// capture (`remember --file`).
        #[arg(long = "file", value_parser = parse_file_filter)]
        file: Vec<rb_types::AnchorFilter>,
        /// Only memories anchored to this commit SHA (repeatable).
        #[arg(long = "commit", value_parser = parse_commit_filter)]
        commit: Vec<rb_types::AnchorFilter>,
        /// Only memories anchored to this symbol (repeatable).
        #[arg(long = "symbol", value_parser = parse_symbol_filter)]
        symbol: Vec<rb_types::AnchorFilter>,
    },

    /// Show memories connected to an id by graph links.
    Graph {
        /// Memory id (UUID).
        id: String,
        /// Traversal depth.
        #[arg(long, default_value_t = 1)]
        depth: u8,
    },

    /// Show a memory's decision history: the supersede chain in both
    /// directions (prior and newer versions) plus active contradicts/extends/
    /// references links, with current/superseded/contested markers. Read-only
    /// — issues zero writer ops.
    History {
        /// Memory id (UUID).
        id: String,
        /// Maximum supersede hops walked per direction (server-clamped to
        /// 1-100; default: the server safety cap).
        #[arg(long, value_parser = value_parser!(u32).range(1..=100))]
        depth: Option<u32>,
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

    /// Ping the daemon and report its health: contract version, writer
    /// health, embedding model, DB path/mode, WAL size, corpus counts.
    Status,

    /// Show value/health aggregates for the current namespace: recall volume,
    /// feedback ratios, top/never-recalled memories, contested count, corpus
    /// growth, re-embed backlog. Read-only — issues zero writer ops.
    Stats {
        /// Window in days for the windowed aggregates (recent accesses,
        /// growth buckets). Server-clamped to 1-365; default 30.
        #[arg(long = "window-days", value_parser = value_parser!(u32).range(1..=365))]
        window_days: Option<u32>,
    },

    /// Run health checks (daemon, socket/DB permissions, embedding model vs
    /// DB meta, WAL size) and exit non-zero if any check fails.
    Doctor,

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

    /// Preview or execute the declarative [retention] forgetting policy
    /// (retention PRD). With no flags this is a DRY-RUN: it lists exactly
    /// what one apply pass would archive — with reasons (age, importance,
    /// last-recalled, archived state) — and writes NOTHING. Requires a
    /// [retention] section in the user config; executing additionally
    /// requires `retention.enabled = true`. Guardrails are absolute: the
    /// importance floor (effective AND author-set), protected tags, and
    /// contested memories are never swept.
    Forget {
        /// Preview only (the default posture even without this flag).
        /// Combine with --hard to preview the purge set instead.
        #[arg(long)]
        dry_run: bool,
        /// Execute one bounded pass: ARCHIVE eligible memories (soft delete,
        /// reversible; vectors are pruned, the row is kept).
        #[arg(long, conflicts_with = "hard")]
        apply: bool,
        /// HARD mode: irreversibly purge eligible memories (row, FTS,
        /// vectors, feedback, and their oplog history — one purge marker
        /// remains). Includes already-archived rows past max_age_days.
        /// Admin op like scrub: only a client running as the daemon's own
        /// user may execute it. With --dry-run, previews the purge set.
        #[arg(long)]
        hard: bool,
        /// Skip the interactive confirmation that hard EXECUTION otherwise
        /// requires (the import-confirmation precedent). Without it, a
        /// non-interactive invocation (--json or piped stdin) refuses
        /// instead of purging.
        #[arg(long, short = 'y')]
        yes: bool,
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
    fn recall_parses_the_unified_filter_flags() {
        let cli = Cli::parse_from([
            "rusty-brain",
            "recall",
            "query text",
            "--type",
            "bug_fix",
            "--tags",
            "sqlite",
            "--min-importance",
            "3",
            "--max-importance",
            "9",
            "--min-confidence",
            "0.2",
            "--max-confidence",
            "0.9",
            "--since",
            "2026-07-01",
            "--until",
            "2026-07-10T12:00:00Z",
            "--source",
            "hook",
            "--source",
            "mcp",
            "--contested",
            "--archived",
        ]);
        match cli.command {
            Command::Recall {
                query,
                memory_type,
                tags,
                min_importance,
                max_importance,
                min_confidence,
                max_confidence,
                since,
                until,
                source,
                contested,
                archived,
                ..
            } => {
                assert_eq!(query, "query text");
                assert_eq!(memory_type, Some(MemoryType::BugFix));
                assert_eq!(tags, vec!["sqlite".to_string()]);
                assert_eq!(min_importance, Some(3));
                assert_eq!(max_importance, Some(9));
                assert_eq!(min_confidence, Some(0.2));
                assert_eq!(max_confidence, Some(0.9));
                assert_eq!(
                    since,
                    Some(
                        "2026-07-01T00:00:00Z"
                            .parse::<chrono::DateTime<chrono::Utc>>()
                            .unwrap()
                    )
                );
                assert_eq!(
                    until,
                    Some(
                        "2026-07-10T12:00:00Z"
                            .parse::<chrono::DateTime<chrono::Utc>>()
                            .unwrap()
                    )
                );
                assert_eq!(source, vec!["hook".to_string(), "mcp".to_string()]);
                assert_eq!(contested, Some(true));
                assert!(archived);
            }
            other => panic!("expected Recall, got {other:?}"),
        }
    }

    #[test]
    fn list_parses_the_same_filter_flags_as_recall() {
        // SRH-2 parity: `list` accepts the identical filter set, including the
        // type/tags flags recall already had.
        let cli = Cli::parse_from([
            "rusty-brain",
            "list",
            "--type",
            "constraint",
            "--tags",
            "infra",
            "--min-importance",
            "5",
            "--max-importance",
            "8",
            "--min-confidence",
            "0.5",
            "--source",
            "cli",
            "--contested",
            "false",
        ]);
        match cli.command {
            Command::List {
                memory_type,
                tags,
                min_importance,
                max_importance,
                min_confidence,
                max_confidence,
                source,
                contested,
                archived,
                ..
            } => {
                assert_eq!(memory_type, Some(MemoryType::Constraint));
                assert_eq!(tags, vec!["infra".to_string()]);
                assert_eq!(min_importance, Some(5));
                assert_eq!(max_importance, Some(8));
                assert_eq!(min_confidence, Some(0.5));
                assert_eq!(max_confidence, None);
                assert_eq!(source, vec!["cli".to_string()]);
                assert_eq!(
                    contested,
                    Some(false),
                    "--contested false selects uncontested"
                );
                assert!(!archived);
            }
            other => panic!("expected List, got {other:?}"),
        }
    }

    #[test]
    fn contested_flag_is_tri_state() {
        // Absent -> no constraint; bare flag -> contested-only; explicit false
        // -> uncontested-only.
        let absent = Cli::parse_from(["rusty-brain", "list"]);
        match absent.command {
            Command::List { contested, .. } => assert_eq!(contested, None),
            other => panic!("expected List, got {other:?}"),
        }
        let bare = Cli::parse_from(["rusty-brain", "recall", "q", "--contested"]);
        match bare.command {
            Command::Recall { contested, .. } => assert_eq!(contested, Some(true)),
            other => panic!("expected Recall, got {other:?}"),
        }
    }

    #[test]
    fn forget_defaults_to_dry_run_and_flags_compose() {
        // Bare `forget` is a DRY-RUN (safety posture): no execution flag, no
        // writes. `--apply` and `--hard` are the explicit executes; adding
        // `--dry-run` to either turns it back into a preview of that mode.
        let cli = Cli::parse_from(["rusty-brain", "forget"]);
        match cli.command {
            Command::Forget {
                dry_run,
                apply,
                hard,
                yes,
            } => {
                assert!(!dry_run && !apply && !hard && !yes);
            }
            other => panic!("expected Forget, got {other:?}"),
        }
        for args in [
            ["rusty-brain", "forget", "--apply"].as_slice(),
            ["rusty-brain", "forget", "--hard"].as_slice(),
            ["rusty-brain", "forget", "--dry-run"].as_slice(),
            ["rusty-brain", "forget", "--hard", "--dry-run"].as_slice(),
            ["rusty-brain", "forget", "--apply", "--dry-run"].as_slice(),
        ] {
            assert!(Cli::try_parse_from(args).is_ok(), "{args:?} must parse");
        }
    }

    #[test]
    fn forget_hard_accepts_yes_for_automation() {
        // PR #60 review (MEDIUM): hard EXECUTE prompts interactively; --yes
        // is the explicit automation bypass.
        let cli = Cli::parse_from(["rusty-brain", "forget", "--hard", "--yes"]);
        match cli.command {
            Command::Forget { hard, yes, .. } => {
                assert!(hard && yes);
            }
            other => panic!("expected Forget, got {other:?}"),
        }
        assert!(Cli::try_parse_from(["rusty-brain", "forget", "-y", "--hard"]).is_ok());
    }

    #[test]
    fn forget_rejects_apply_with_hard_and_garbage() {
        // apply and hard are mutually exclusive modes; garbage input is a
        // clean parse error, never a panic.
        assert!(
            Cli::try_parse_from(["rusty-brain", "forget", "--apply", "--hard"]).is_err(),
            "--apply conflicts with --hard"
        );
        assert!(Cli::try_parse_from(["rusty-brain", "forget", "--nonsense"]).is_err());
        assert!(Cli::try_parse_from(["rusty-brain", "forget", "extra-positional"]).is_err());
    }

    #[test]
    fn since_accepts_relative_ages() {
        // "7d" (and h/m suffixes) parse as now-relative bounds — the "age"
        // half of the PRD's date/age filter.
        let before = chrono::Utc::now() - chrono::Duration::days(7);
        let cli = Cli::parse_from(["rusty-brain", "recall", "q", "--since", "7d"]);
        let after = chrono::Utc::now() - chrono::Duration::days(7);
        match cli.command {
            Command::Recall { since, .. } => {
                let since = since.expect("--since 7d must parse");
                assert!(since >= before && since <= after, "since={since}");
            }
            other => panic!("expected Recall, got {other:?}"),
        }
        for ok in ["36h", "45m", "10s"] {
            assert!(
                Cli::try_parse_from(["rusty-brain", "list", "--until", ok]).is_ok(),
                "relative age {ok} must parse"
            );
        }
    }

    #[test]
    fn time_bounds_reject_overflowing_relative_age() {
        // chrono's panicking Duration constructors must never be reachable
        // from argv: an absurd relative age is a clean parse ERROR, not an
        // exit-101 panic.
        for bad in ["200000000000d", "999999999999999h", "99999999999999999m"] {
            assert!(
                Cli::try_parse_from(["rusty-brain", "recall", "q", "--since", bad]).is_err(),
                "--since {bad} must be rejected, not panic"
            );
            assert!(
                Cli::try_parse_from(["rusty-brain", "list", "--until", bad]).is_err(),
                "--until {bad} must be rejected, not panic"
            );
        }
    }

    #[test]
    fn time_bounds_reject_garbage() {
        for bad in ["yesterdayish", "2026-13-40", "7x", "d7", ""] {
            assert!(
                Cli::try_parse_from(["rusty-brain", "recall", "q", "--since", bad]).is_err(),
                "--since {bad:?} must be rejected at parse time"
            );
        }
    }

    #[test]
    fn source_rejects_unknown_producers() {
        for ok in ["hook", "mcp", "cli", "job"] {
            assert!(
                Cli::try_parse_from(["rusty-brain", "list", "--source", ok]).is_ok(),
                "source {ok} must parse"
            );
        }
        assert!(
            Cli::try_parse_from(["rusty-brain", "list", "--source", "carrier-pigeon"]).is_err(),
            "unknown --source must be rejected at parse time"
        );
    }

    #[test]
    fn confidence_bounds_reject_out_of_range_at_parse_time() {
        for bad in ["-0.1", "1.5", "NaN"] {
            assert!(
                Cli::try_parse_from(["rusty-brain", "recall", "q", "--min-confidence", bad])
                    .is_err(),
                "--min-confidence {bad} must fail to parse"
            );
            assert!(
                Cli::try_parse_from(["rusty-brain", "list", "--max-confidence", bad]).is_err(),
                "--max-confidence {bad} must fail to parse"
            );
        }
    }

    #[test]
    fn init_parses_yes_dry_run_and_bounds() {
        let cli = Cli::parse_from([
            "rusty-brain",
            "init",
            "--yes",
            "--dry-run",
            "--max-files",
            "12",
            "--max-bytes",
            "4096",
            "--importance",
            "7",
        ]);
        match cli.command {
            Command::Init {
                yes,
                dry_run,
                max_files,
                max_bytes,
                importance,
                undo,
                list_batches,
            } => {
                assert!(yes);
                assert!(dry_run);
                assert_eq!(max_files, 12);
                assert_eq!(max_bytes, 4096);
                assert_eq!(importance, 7);
                assert_eq!(undo, None);
                assert!(!list_batches);
            }
            other => panic!("expected Init, got {other:?}"),
        }
    }

    #[test]
    fn init_parses_undo_and_list_batches_conflicts() {
        let undo = Cli::parse_from(["rusty-brain", "init", "--undo", "import-abc"]);
        assert!(matches!(undo.command, Command::Init { undo: Some(_), .. }));

        let err = Cli::try_parse_from([
            "rusty-brain",
            "init",
            "--undo",
            "import-abc",
            "--list-batches",
        ])
        .unwrap_err();
        assert!(err.to_string().contains("cannot be used with"));
    }

    #[test]
    fn init_undo_conflicts_with_scan_flags() {
        let err = Cli::try_parse_from([
            "rusty-brain",
            "init",
            "--undo",
            "import-abc",
            "--max-files",
            "10",
        ])
        .unwrap_err();
        assert!(err.to_string().contains("cannot be used with"));

        let err =
            Cli::try_parse_from(["rusty-brain", "init", "--list-batches", "--importance", "7"])
                .unwrap_err();
        assert!(err.to_string().contains("cannot be used with"));
    }

    #[test]
    fn import_parses_file_type_tags_and_dry_run() {
        let cli = Cli::parse_from([
            "rusty-brain",
            "import",
            "docs/ADR.md",
            "--type",
            "architecture_decision",
            "--importance",
            "8",
            "--tags",
            "seed",
            "--dry-run",
            "--max-bytes",
            "8192",
        ]);
        match cli.command {
            Command::Import {
                path,
                memory_type,
                importance,
                tags,
                dry_run,
                max_bytes,
            } => {
                assert_eq!(path, "docs/ADR.md");
                assert_eq!(memory_type, MemoryType::ArchitectureDecision);
                assert_eq!(importance, 8);
                assert_eq!(tags, vec!["seed"]);
                assert!(dry_run);
                assert_eq!(max_bytes, 8192);
            }
            other => panic!("expected Import, got {other:?}"),
        }
    }

    #[test]
    fn remember_batch_flag_parses_without_content() {
        let cli = Cli::parse_from(["rusty-brain", "remember", "--batch", "--importance", "6"]);
        match cli.command {
            Command::Remember {
                content,
                batch,
                importance,
                ..
            } => {
                assert!(batch, "--batch must set the batch flag");
                assert_eq!(
                    content, None,
                    "--batch reads facts from stdin, not a positional"
                );
                assert_eq!(importance, 6, "uniform --importance applies to the batch");
            }
            other => panic!("expected Remember, got {other:?}"),
        }
    }

    #[test]
    fn remember_without_batch_keeps_content_required_and_optional_typed() {
        // required_unless_present("batch"): no content AND no --batch => parse error
        // (preserves the historical `remember` contract for a missing positional).
        assert!(
            Cli::try_parse_from(["rusty-brain", "remember"]).is_err(),
            "`remember` with neither content nor --batch must fail to parse"
        );
        // A bare positional still parses with batch defaulting off.
        let cli = Cli::parse_from(["rusty-brain", "remember", "a fact"]);
        match cli.command {
            Command::Remember { content, batch, .. } => {
                assert_eq!(content.as_deref(), Some("a fact"));
                assert!(!batch, "--batch defaults off");
            }
            other => panic!("expected Remember, got {other:?}"),
        }
    }

    #[test]
    fn remember_batch_conflicts_with_supersedes_and_content() {
        // Bulk insert has no single prior, so --supersedes is meaningless with it.
        assert!(
            Cli::try_parse_from(["rusty-brain", "remember", "--batch", "--supersedes", "x"])
                .is_err(),
            "--batch must conflict with --supersedes"
        );
        // A positional + --batch is ambiguous (stdin vs the positional).
        assert!(
            Cli::try_parse_from(["rusty-brain", "remember", "a fact", "--batch"]).is_err(),
            "--batch must conflict with a positional content argument"
        );
    }

    #[test]
    fn remember_parses_anchor_capture_flags() {
        let cli = Cli::parse_from([
            "rusty-brain",
            "remember",
            "we chose tokio here",
            "--file",
            "src/server.rs:12-40",
            "--file",
            "./src/lib.rs",
            "--commit",
            "abc123",
            "--symbol",
            "Server::run",
        ]);
        match cli.command {
            Command::Remember {
                file,
                commit,
                symbol,
                ..
            } => {
                assert_eq!(file.len(), 2);
                assert_eq!(file[0].value, "src/server.rs");
                assert_eq!((file[0].start_line, file[0].end_line), (Some(12), Some(40)));
                assert_eq!(file[1].value, "src/lib.rs", "paths normalize");
                assert_eq!(commit.len(), 1);
                assert_eq!(commit[0].kind, rb_types::AnchorKind::Commit);
                assert_eq!(commit[0].value, "abc123");
                assert_eq!(symbol.len(), 1);
                assert_eq!(symbol[0].kind, rb_types::AnchorKind::Symbol);
                assert_eq!(symbol[0].value, "Server::run");
            }
            other => panic!("expected Remember, got {other:?}"),
        }
    }

    #[test]
    fn remember_anchor_flags_reject_garbage_at_parse_time() {
        // Clean parse errors, never a panic (the parse_time_bound precedent) —
        // incl. line-number overflow past u32.
        for bad in [
            "a.rs:12-",
            "a.rs:0",
            "a.rs:40-12",
            "a.rs:99999999999999999999",
            "",
            "./",
        ] {
            assert!(
                Cli::try_parse_from(["rusty-brain", "remember", "c", "--file", bad]).is_err(),
                "remember --file {bad:?} must be rejected at parse time"
            );
        }
        for flag in ["--commit", "--symbol"] {
            assert!(
                Cli::try_parse_from(["rusty-brain", "remember", "c", flag, "  "]).is_err(),
                "remember {flag} with a blank value must be rejected"
            );
        }
    }

    #[test]
    fn recall_and_list_parse_anchor_filters() {
        for cmd in [
            vec!["rusty-brain", "recall", "q"],
            vec!["rusty-brain", "list"],
        ] {
            let mut argv = cmd.clone();
            argv.extend([
                "--file",
                "./src/server.rs",
                "--commit",
                "abc123",
                "--symbol",
                "Engine::recall",
            ]);
            let cli = Cli::parse_from(argv);
            let (file, commit, symbol) = match cli.command {
                Command::Recall {
                    file,
                    commit,
                    symbol,
                    ..
                }
                | Command::List {
                    file,
                    commit,
                    symbol,
                    ..
                } => (file, commit, symbol),
                other => panic!("expected Recall/List, got {other:?}"),
            };
            assert_eq!(file.len(), 1);
            assert_eq!(file[0].kind, rb_types::AnchorKind::File);
            assert_eq!(file[0].value, "src/server.rs", "filter values normalize");
            assert_eq!(commit[0].kind, rb_types::AnchorKind::Commit);
            assert_eq!(symbol[0].kind, rb_types::AnchorKind::Symbol);
        }
    }

    #[test]
    fn recall_file_filter_rejects_line_ranges_and_blanks() {
        // Filters match by path only: a line range is a clean parse error
        // pointing at capture, never a silently-widened (or never-matching)
        // query.
        for bad in ["src/a.rs:12", "src/a.rs:12-40", "", "  "] {
            assert!(
                Cli::try_parse_from(["rusty-brain", "recall", "q", "--file", bad]).is_err(),
                "recall --file {bad:?} must be rejected at parse time"
            );
            assert!(
                Cli::try_parse_from(["rusty-brain", "list", "--file", bad]).is_err(),
                "list --file {bad:?} must be rejected at parse time"
            );
        }
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
    fn parses_history_with_optional_depth() {
        let id = "0c8e7f76-3a4f-4f7e-9d3a-111111111111";
        let cli = Cli::parse_from(["rusty-brain", "history", id]);
        match cli.command {
            Command::History { id: got, depth } => {
                assert_eq!(got, id);
                assert_eq!(depth, None, "absent depth defers to the server cap");
            }
            other => panic!("expected History, got {other:?}"),
        }
        let cli = Cli::parse_from(["rusty-brain", "history", id, "--depth", "3"]);
        assert!(matches!(
            cli.command,
            Command::History { depth: Some(3), .. }
        ));
        // Global --json applies to history.
        let cli = Cli::parse_from(["rusty-brain", "--json", "history", id]);
        assert!(cli.json);
        assert!(matches!(cli.command, Command::History { .. }));
    }

    #[test]
    fn history_rejects_out_of_range_depth_at_parse_time() {
        for bad in ["0", "101"] {
            let res = Cli::try_parse_from(["rusty-brain", "history", "some-id", "--depth", bad]);
            assert!(res.is_err(), "--depth {bad} must be rejected at parse time");
        }
        // An id argument is required.
        assert!(Cli::try_parse_from(["rusty-brain", "history"]).is_err());
    }

    #[test]
    fn parses_stats_with_optional_window() {
        let cli = Cli::parse_from(["rusty-brain", "stats"]);
        assert!(
            matches!(cli.command, Command::Stats { window_days: None }),
            "`rusty-brain stats` must parse with no window (daemon default)"
        );
        let cli = Cli::parse_from(["rusty-brain", "stats", "--window-days", "7"]);
        assert!(matches!(
            cli.command,
            Command::Stats {
                window_days: Some(7)
            }
        ));
        // Global --json applies to stats.
        let cli = Cli::parse_from(["rusty-brain", "--json", "stats"]);
        assert!(cli.json);
        assert!(matches!(cli.command, Command::Stats { .. }));
    }

    #[test]
    fn stats_rejects_a_zero_window_at_parse_time() {
        let res = Cli::try_parse_from(["rusty-brain", "stats", "--window-days", "0"]);
        assert!(res.is_err(), "a zero-day window is meaningless");
    }

    #[test]
    fn parses_doctor_subcommand() {
        let cli = Cli::parse_from(["rusty-brain", "doctor"]);
        assert!(
            matches!(cli.command, Command::Doctor),
            "`rusty-brain doctor` must parse to Command::Doctor"
        );
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
