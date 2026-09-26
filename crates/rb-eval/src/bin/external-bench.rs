//! External benchmark runner (Vikunja #60). Datasets live outside git; this
//! binary refuses to run on a file whose SHA-256 differs from the pinned
//! manifest (`crates/rb-eval/external/manifest.json`).

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use rb_embed::{EmbeddingProvider, LocalProvider};
use rb_eval::external::longmemeval::{aggregate, run_question, Entry, Profile, TOP_N};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

const MANIFEST: &str = include_str!("../../external/manifest.json");

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Longmemeval {
        #[arg(long)]
        data: PathBuf,
        #[arg(long, value_enum)]
        profile: Profile,
        #[arg(long)]
        out: PathBuf,
        /// First N questions only (smoke runs). A limited run is never a result.
        #[arg(long)]
        limit: Option<usize>,
    },
}

fn git(args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git").args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[allow(unsafe_code)]
fn max_rss_bytes() -> i64 {
    // SAFETY: rusage is plain-old-data, so all-zero is a valid value, and
    // getrusage only writes into the struct we own.
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) } != 0 {
        return -1;
    }
    // macOS reports bytes, Linux kilobytes.
    if cfg!(target_os = "macos") {
        usage.ru_maxrss
    } else {
        usage.ru_maxrss * 1024
    }
}

fn percentile(sorted: &[u128], p: f64) -> u128 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = ((p / 100.0) * (sorted.len() - 1) as f64).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

#[tokio::main]
async fn main() -> Result<()> {
    let Command::Longmemeval {
        data,
        profile,
        out,
        limit,
    } = Cli::parse().command;
    let manifest: serde_json::Value = serde_json::from_str(MANIFEST)?;
    let pinned = &manifest["datasets"]["longmemeval_s_cleaned"];

    let bytes = std::fs::read(&data).with_context(|| format!("reading {}", data.display()))?;
    let sha = format!("{:x}", Sha256::digest(&bytes));
    if Some(sha.as_str()) != pinned["sha256"].as_str() {
        bail!("dataset sha256 {sha} does not match the pinned manifest");
    }
    let mut entries: Vec<Entry> = serde_json::from_slice(&bytes)?;
    drop(bytes);
    if let Some(n) = limit {
        entries.truncate(n);
    }

    let model = manifest["rusty_brain"]["embedding_model"]
        .as_str()
        .context("manifest embedding_model")?;
    let provider = Arc::new(LocalProvider::load(model)?);
    std::fs::create_dir_all(&out)?;
    let tag = format!(
        "longmemeval-{}",
        serde_json::to_value(profile)?.as_str().unwrap_or("?")
    );
    let mut jsonl =
        std::io::BufWriter::new(std::fs::File::create(out.join(format!("{tag}.jsonl")))?);

    let wall = Instant::now();
    let mut records = Vec::with_capacity(entries.len());
    for (i, entry) in entries.iter().enumerate() {
        let record = run_question(&provider, entry, profile).await?;
        serde_json::to_writer(&mut jsonl, &record)?;
        jsonl.write_all(b"\n")?;
        if (i + 1) % 50 == 0 {
            eprintln!(
                "[{}/{}] {:.0}s",
                i + 1,
                entries.len(),
                wall.elapsed().as_secs_f64()
            );
        }
        records.push(record);
    }
    jsonl.flush()?;

    let mut by_type: BTreeMap<&str, Vec<_>> = BTreeMap::new();
    for r in &records {
        by_type.entry(r.question_type.as_str()).or_default().push(r);
    }
    let per_type: BTreeMap<_, _> = by_type
        .iter()
        .map(|(t, rs)| (*t, aggregate(rs.iter().copied())))
        .collect();
    let mut failures: BTreeMap<&str, usize> = BTreeMap::new();
    for r in &records {
        *failures.entry(r.failure).or_default() += 1;
    }
    let mut query_us: Vec<u128> = records.iter().map(|r| r.query_us).collect();
    query_us.sort_unstable();

    let summary = json!({
        "benchmark": "longmemeval_s_cleaned",
        "profile": profile,
        "complete": limit.is_none(),
        "rusty_brain_commit": git(&["rev-parse", "HEAD"]),
        "rusty_brain_dirty": git(&["status", "--porcelain"]).map(|s| !s.is_empty()),
        "mempalace_commit": manifest["mempalace"]["commit"],
        "dataset_sha256": sha,
        "dataset_hf_revision": pinned["hf_revision"],
        "embedding": {"model": provider.model_id(), "dim": provider.dim(), "input_kind": "document/query (symmetric model; kind ignored)"},
        "granularity": "session (user turns joined by newline)",
        "fusion": if profile == Profile::Native { "production default (Linear) + heuristic enrichment" } else { "none (vector index only)" },
        "top_n": TOP_N,
        "os": format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
        "wall_clock_s": wall.elapsed().as_secs_f64(),
        "ingest_ms_total": records.iter().map(|r| r.ingest_ms).sum::<u128>(),
        "query_us": {"p50": percentile(&query_us, 50.0), "p95": percentile(&query_us, 95.0), "p99": percentile(&query_us, 99.0)},
        "max_rss_bytes": max_rss_bytes(),
        "db": "in-memory per question (size not measured)",
        "external_api_cost_usd": 0.0,
        "overall": aggregate(&records),
        "per_type": per_type,
        "failures": failures,
    });
    let path = out.join(format!("{tag}.summary.json"));
    std::fs::write(&path, serde_json::to_string_pretty(&summary)?)?;
    println!("{}", serde_json::to_string_pretty(&summary["overall"])?);
    eprintln!("wrote {}", path.display());
    Ok(())
}
