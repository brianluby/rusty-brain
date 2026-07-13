//! Aggregate dry-run and local-only session replay dataset builder.

use std::path::PathBuf;

use anyhow::{anyhow, Context};
use chrono::{DateTime, Utc};
use clap::Parser;
use rb_eval::session_replay::{
    build_candidate_dataset, build_inventory_report, derive_holdout_boundary, import_claude_jsonl,
    import_opencode_db, write_local_artifacts, DEFAULT_FAKER_SEED,
};

#[derive(Debug, Parser)]
#[command(
    about = "Build privacy-preserving local session replay shapes (dry-run by default)",
    version
)]
struct Options {
    /// Claude Code transcript root. Defaults below the current user's home.
    #[arg(long)]
    claude_root: Option<PathBuf>,
    /// OpenCode SQLite store. Defaults below the current user's home.
    #[arg(long)]
    opencode_db: Option<PathBuf>,
    /// Whole-session holdout boundary as RFC3339. Defaults to the 80% session-start boundary.
    #[arg(long)]
    holdout_after: Option<String>,
    /// Faker/de-identification seed recorded in every aggregate report and augmentation record.
    #[arg(long, default_value_t = DEFAULT_FAKER_SEED)]
    seed: u64,
    /// Write owner-only datasets under the ignored output directory. Omit for aggregate dry-run.
    #[arg(long)]
    write_local: bool,
    /// Must end in `session-replay-local`; enforced by the writer.
    #[arg(long, default_value = "target/session-replay-local")]
    output: PathBuf,
}

fn main() -> anyhow::Result<()> {
    let options = Options::parse();
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("the local home directory is unavailable"))?;
    let claude_root = options
        .claude_root
        .unwrap_or_else(|| home.join(".claude/projects"));
    let opencode_db = options
        .opencode_db
        .unwrap_or_else(|| home.join(".local/share/opencode/opencode.db"));

    let claude = import_claude_jsonl(&claude_root, options.seed)
        .context("Claude Code aggregate import failed")?;
    let opencode = import_opencode_db(&opencode_db, options.seed)
        .context("OpenCode aggregate import failed")?;
    let mut sessions = Vec::with_capacity(claude.sessions.len() + opencode.sessions.len());
    sessions.extend(claude.sessions.iter().cloned());
    sessions.extend(opencode.sessions.iter().cloned());
    sessions.sort_by(|left, right| {
        (left.started_at, &left.session_id).cmp(&(right.started_at, &right.session_id))
    });

    let boundary = match options.holdout_after {
        Some(value) => DateTime::parse_from_rfc3339(&value)
            .map(|value| value.with_timezone(&Utc))
            .map_err(|_| anyhow!("holdout boundary must be valid RFC3339"))?,
        None => derive_holdout_boundary(&sessions)
            .ok_or_else(|| anyhow!("no normalized sessions are available for splitting"))?,
    };
    let dataset = build_candidate_dataset(&sessions, boundary);
    let mut report = build_inventory_report(&[&claude, &opencode], &dataset, options.seed);
    if options.write_local {
        report.mode = "local_build".to_string();
        write_local_artifacts(&options.output, &sessions, &dataset, &report)
            .context("local artifact write failed")?;
    }

    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
