//! External benchmark runner (Vikunja #60). Datasets live outside git; this
//! binary refuses to run on a file whose SHA-256 differs from the pinned
//! manifest (`crates/rb-eval/external/manifest.json`).

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use rb_embed::{EmbeddingProvider, LocalProvider};
use rb_eval::external::core::Memo;
use rb_eval::external::locomo::{run_sample, QaRecord, Sample};
use rb_eval::external::longmemeval::{aggregate, run_question, Entry, Profile, TOP_N};
use rb_eval::external::{convomem, membench};
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
        /// Embedding truncation in tokens (default: production, fastembed 512).
        #[arg(long)]
        embed_max_tokens: Option<usize>,
    },
    /// ConvoMem sample protocol over MemPalace's cache layout.
    Convomem {
        #[arg(long)]
        cache: PathBuf,
        #[arg(long, value_enum)]
        profile: Profile,
        #[arg(long)]
        out: PathBuf,
        #[arg(long, default_value_t = 50)]
        per_category: usize,
        #[arg(long, default_value_t = 10)]
        top_k: usize,
        #[arg(long)]
        embed_max_tokens: Option<usize>,
        /// Write the selection pin instead of verifying it (done once, before the freeze).
        #[arg(long)]
        write_selection: bool,
    },
    /// MemBench FirstAgent files, all categories.
    Membench {
        #[arg(long)]
        data: PathBuf,
        #[arg(long, value_enum)]
        profile: Profile,
        #[arg(long)]
        out: PathBuf,
        #[arg(long, default_value = "movie")]
        topic: String,
        #[arg(long, default_value_t = 5)]
        top_k: usize,
        #[arg(long)]
        embed_max_tokens: Option<usize>,
    },
    Locomo {
        #[arg(long)]
        data: PathBuf,
        #[arg(long, value_enum)]
        profile: Profile,
        #[arg(long)]
        out: PathBuf,
        #[arg(long, default_value_t = 10)]
        top_k: usize,
        /// First N conversations only (smoke runs). A limited run is never a result.
        #[arg(long)]
        limit: Option<usize>,
        /// Embedding truncation in tokens (default: production, fastembed 512).
        #[arg(long)]
        embed_max_tokens: Option<usize>,
    },
}

fn read_pinned(
    manifest: &serde_json::Value,
    key: &str,
    path: &PathBuf,
) -> Result<(Vec<u8>, String)> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let sha = format!("{:x}", Sha256::digest(&bytes));
    if Some(sha.as_str()) != manifest["datasets"][key]["sha256"].as_str() {
        bail!("{key} sha256 {sha} does not match the pinned manifest");
    }
    Ok((bytes, sha))
}

fn profile_name(profile: Profile) -> String {
    serde_json::to_value(profile)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

fn token_suffix(max_tokens: Option<usize>) -> String {
    max_tokens.map(|n| format!("-tok{n}")).unwrap_or_default()
}

fn max_tokens_json(max_tokens: Option<usize>) -> serde_json::Value {
    max_tokens.map_or_else(
        || json!("512 (fastembed default, production)"),
        |n| json!(n),
    )
}

fn run_meta(
    manifest: &serde_json::Value,
    profile: Profile,
    embed_max_tokens: Option<usize>,
) -> serde_json::Value {
    json!({
        "profile": profile,
        "rusty_brain_commit": git(&["rev-parse", "HEAD"]),
        "rusty_brain_dirty": git(&["status", "--porcelain"]).map(|s| !s.is_empty()),
        "mempalace_commit": manifest["mempalace"]["commit"],
        "embedding": {"model": manifest["rusty_brain"]["embedding_model"], "dim": manifest["rusty_brain"]["embedding_dim"], "input_kind": "document/query (symmetric model; kind ignored)", "max_tokens": max_tokens_json(embed_max_tokens)},
        "fusion": if profile == Profile::Native { "production default (Linear) + heuristic enrichment" } else { "none (vector index only)" },
        "os": format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
        "db": "in-memory per question (size not measured)",
        "external_api_cost_usd": 0.0,
    })
}

fn mean(xs: impl Iterator<Item = f64>) -> Option<f64> {
    let (sum, n) = xs.fold((0.0, 0usize), |(s, n), x| (s + x, n + 1));
    (n > 0).then(|| sum / n as f64)
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
    let manifest: serde_json::Value = serde_json::from_str(MANIFEST)?;
    match Cli::parse().command {
        Command::Longmemeval {
            data,
            profile,
            out,
            limit,
            embed_max_tokens,
        } => longmemeval(&manifest, data, profile, out, limit, embed_max_tokens).await,
        Command::Convomem {
            cache,
            profile,
            out,
            per_category,
            top_k,
            embed_max_tokens,
            write_selection,
        } => {
            convomem_cmd(
                &manifest,
                cache,
                profile,
                out,
                per_category,
                top_k,
                embed_max_tokens,
                write_selection,
            )
            .await
        }
        Command::Membench {
            data,
            profile,
            out,
            topic,
            top_k,
            embed_max_tokens,
        } => {
            membench_cmd(
                &manifest,
                data,
                profile,
                out,
                topic,
                top_k,
                embed_max_tokens,
            )
            .await
        }
        Command::Locomo {
            data,
            profile,
            out,
            top_k,
            limit,
            embed_max_tokens,
        } => {
            locomo(
                &manifest,
                data,
                profile,
                out,
                top_k,
                limit,
                embed_max_tokens,
            )
            .await
        }
    }
}

fn sha256_file(path: &std::path::Path) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(std::fs::read(path)?)))
}

fn write_outputs(
    out: &std::path::Path,
    tag: &str,
    records: &[impl serde::Serialize],
    summary: &serde_json::Value,
) -> Result<()> {
    let mut jsonl =
        std::io::BufWriter::new(std::fs::File::create(out.join(format!("{tag}.jsonl")))?);
    for r in records {
        serde_json::to_writer(&mut jsonl, r)?;
        jsonl.write_all(b"\n")?;
    }
    jsonl.flush()?;
    let path = out.join(format!("{tag}.summary.json"));
    std::fs::write(&path, serde_json::to_string_pretty(summary)?)?;
    println!("{}", serde_json::to_string_pretty(&summary["overall"])?);
    eprintln!("wrote {}", path.display());
    Ok(())
}

fn merge(mut base: serde_json::Value, extra: serde_json::Value) -> serde_json::Value {
    if let (Some(a), Some(b)) = (base.as_object_mut(), extra.as_object()) {
        a.extend(b.clone());
    }
    base
}

const CONVOMEM_SELECTION: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/external/convomem_selection.json"
);

#[allow(clippy::too_many_arguments)]
async fn convomem_cmd(
    manifest: &serde_json::Value,
    cache: PathBuf,
    profile: Profile,
    out: PathBuf,
    per_category: usize,
    top_k: usize,
    embed_max_tokens: Option<usize>,
    write_selection: bool,
) -> Result<()> {
    let mut selected = Vec::new();
    let mut pins = serde_json::Map::new();
    for category in convomem::CATEGORIES {
        let (items, files) = convomem::select(&cache, category, per_category)?;
        let mut hashes = serde_json::Map::new();
        let list = cache.join(format!("{category}_filelist.json"));
        if list.exists() {
            hashes.insert("filelist".into(), json!(sha256_file(&list)?));
        } else {
            hashes.insert(
                "skipped".into(),
                json!("no 1_evidence file list (MemPalace skips this category)"),
            );
        }
        for f in &files {
            hashes.insert(
                f.clone(),
                json!(sha256_file(
                    &cache.join(category).join(f.replace('/', "_"))
                )?),
            );
        }
        pins.insert(
            category.into(),
            json!({"items": items.len(), "files": hashes}),
        );
        selected.extend(items);
    }
    let pins = serde_json::Value::Object(pins);
    if write_selection {
        std::fs::write(
            CONVOMEM_SELECTION,
            serde_json::to_string_pretty(&pins)? + "\n",
        )?;
        eprintln!("wrote {CONVOMEM_SELECTION}; commit it before any full run");
        return Ok(());
    }
    let pinned: serde_json::Value =
        serde_json::from_str(include_str!("../../external/convomem_selection.json"))?;
    if pinned != pins {
        bail!("ConvoMem selection differs from {CONVOMEM_SELECTION}");
    }
    let model = manifest["rusty_brain"]["embedding_model"]
        .as_str()
        .context("embedding_model")?;
    let provider = Arc::new(LocalProvider::load_with_max_tokens(
        model,
        embed_max_tokens,
    )?);
    std::fs::create_dir_all(&out)?;
    let wall = Instant::now();
    let mut records = Vec::with_capacity(selected.len());
    for s in &selected {
        records.push(convomem::run_item(&provider, s, profile, top_k).await?);
    }
    let mut per_cat: BTreeMap<&str, Vec<&convomem::ItemRecord>> = BTreeMap::new();
    for r in &records {
        per_cat.entry(r.category).or_default().push(r);
    }
    let per_category: BTreeMap<&str, serde_json::Value> = per_cat
        .iter()
        .map(|(c, rs)| {
            (
                *c,
                json!({"n": rs.len(), "recall": mean(rs.iter().map(|r| r.recall))}),
            )
        })
        .collect();
    let mut query_us: Vec<u128> = records.iter().map(|r| r.query_us).collect();
    query_us.sort_unstable();
    let summary = merge(
        run_meta(manifest, profile, embed_max_tokens),
        json!({
            "benchmark": "convomem",
            "complete": true,
            "dataset_hf_revision": manifest["datasets"]["convomem"]["hf_revision"],
            "selection": pins,
            "granularity": "message",
            "top_k": top_k,
            "wall_clock_s": wall.elapsed().as_secs_f64(),
            "ingest_ms_total": records.iter().map(|r| r.ingest_ms).sum::<u128>(),
            "query_us": {"p50": percentile(&query_us, 50.0), "p95": percentile(&query_us, 95.0), "p99": percentile(&query_us, 99.0)},
            "max_rss_bytes": max_rss_bytes(),
            "overall": {
                "n": records.len(),
                "recall_mempalace": mean(records.iter().map(|r| r.recall)),
                "items_with_degenerate_gold": records.iter().filter(|r| r.degenerate_gold > 0).count(),
            "items_with_empty_evidence": records.iter().filter(|r| r.empty_evidence > 0).count(),
                "items_without_evidence": records.iter().filter(|r| r.evidence_count == 0).count(),
                "abstained": records.iter().filter(|r| r.abstained.is_some()).count(),
            },
            "per_category": per_category,
        }),
    );
    let tag = format!(
        "convomem-top{top_k}-{}{}",
        profile_name(profile),
        token_suffix(embed_max_tokens)
    );
    write_outputs(&out, &tag, &records, &summary)
}

async fn membench_cmd(
    manifest: &serde_json::Value,
    data: PathBuf,
    profile: Profile,
    out: PathBuf,
    topic: String,
    top_k: usize,
    embed_max_tokens: Option<usize>,
) -> Result<()> {
    for (_, file) in membench::CATEGORY_FILES {
        let path = data.join(file);
        let Some(want) = manifest["datasets"]["membench"]["files"][file].as_str() else {
            bail!("membench {file} has no sha256 pin in the manifest");
        };
        if !path.exists() {
            bail!("membench {file} is missing from {}", data.display());
        }
        if sha256_file(&path)? != want {
            bail!("membench {file} sha256 does not match the pinned manifest");
        }
    }
    let items = membench::load(&data, &topic)?;
    let model = manifest["rusty_brain"]["embedding_model"]
        .as_str()
        .context("embedding_model")?;
    let provider = Arc::new(LocalProvider::load_with_max_tokens(
        model,
        embed_max_tokens,
    )?);
    std::fs::create_dir_all(&out)?;
    let wall = Instant::now();
    let mut records = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        if let Some(r) = membench::run_item(&provider, item, profile, top_k).await? {
            records.push(r);
        }
        if (i + 1) % 500 == 0 {
            eprintln!(
                "[{}/{}] {:.0}s",
                i + 1,
                items.len(),
                wall.elapsed().as_secs_f64()
            );
        }
    }
    let mut per_cat: BTreeMap<&str, Vec<&membench::ItemRecord>> = BTreeMap::new();
    for r in &records {
        per_cat.entry(r.category).or_default().push(r);
    }
    let rate = |rs: &[&membench::ItemRecord], f: fn(&membench::ItemRecord) -> bool| {
        mean(rs.iter().map(|r| f64::from(u8::from(f(r)))))
    };
    let per_category: BTreeMap<&str, serde_json::Value> = per_cat
        .iter()
        .map(|(c, rs)| (*c, json!({"n": rs.len(), "hit_at_k": rate(rs, |r| r.hit), "hit_at_k_sid_only": rate(rs, |r| r.hit_sid_only)})))
        .collect();
    let all: Vec<&membench::ItemRecord> = records.iter().collect();
    let mut query_us: Vec<u128> = records.iter().map(|r| r.query_us).collect();
    query_us.sort_unstable();
    let summary = merge(
        run_meta(manifest, profile, embed_max_tokens),
        json!({
            "benchmark": "membench",
            "complete": true,
            "dataset_commit": manifest["datasets"]["membench"]["commit"],
            "topic": topic,
            "granularity": "turn",
            "top_k": top_k,
            "wall_clock_s": wall.elapsed().as_secs_f64(),
            "ingest_ms_total": records.iter().map(|r| r.ingest_ms).sum::<u128>(),
            "query_us": {"p50": percentile(&query_us, 50.0), "p95": percentile(&query_us, 95.0), "p99": percentile(&query_us, 99.0)},
            "max_rss_bytes": max_rss_bytes(),
            "overall": {
                "n": records.len(),
                "hit_at_k_mempalace": rate(&all, |r| r.hit),
                "hit_at_k_sid_only": rate(&all, |r| r.hit_sid_only),
                "abstained": records.iter().filter(|r| r.abstained.is_some()).count(),
            },
            "per_category": per_category,
        }),
    );
    let tag = format!(
        "membench-{topic}-top{top_k}-{}{}",
        profile_name(profile),
        token_suffix(embed_max_tokens)
    );
    write_outputs(&out, &tag, &records, &summary)
}

async fn locomo(
    manifest: &serde_json::Value,
    data: PathBuf,
    profile: Profile,
    out: PathBuf,
    top_k: usize,
    limit: Option<usize>,
    embed_max_tokens: Option<usize>,
) -> Result<()> {
    let (bytes, sha) = read_pinned(manifest, "locomo10", &data)?;
    let mut samples: Vec<Sample> = serde_json::from_slice(&bytes)?;
    if let Some(n) = limit {
        samples.truncate(n);
    }
    let model = manifest["rusty_brain"]["embedding_model"]
        .as_str()
        .context("embedding_model")?;
    let provider = Arc::new(Memo::new(LocalProvider::load_with_max_tokens(
        model,
        embed_max_tokens,
    )?));
    std::fs::create_dir_all(&out)?;
    let tag = format!(
        "locomo-top{top_k}-{}{}",
        profile_name(profile),
        token_suffix(embed_max_tokens)
    );
    let mut jsonl =
        std::io::BufWriter::new(std::fs::File::create(out.join(format!("{tag}.jsonl")))?);
    let wall = Instant::now();
    let mut records: Vec<QaRecord> = Vec::new();
    for sample in &samples {
        for r in run_sample(&provider, sample, profile, top_k).await? {
            serde_json::to_writer(&mut jsonl, &r)?;
            jsonl.write_all(b"\n")?;
            records.push(r);
        }
        eprintln!(
            "{} done, {:.0}s",
            sample.sample_id,
            wall.elapsed().as_secs_f64()
        );
    }
    jsonl.flush()?;
    let mut by_cat: BTreeMap<u8, Vec<&QaRecord>> = BTreeMap::new();
    for r in &records {
        by_cat.entry(r.category).or_default().push(r);
    }
    let per_category: BTreeMap<String, serde_json::Value> = by_cat
        .iter()
        .map(|(c, rs)| {
            (c.to_string(), json!({
                "n": rs.len(),
                "evidence_recall": mean(rs.iter().map(|r| r.evidence_recall)),
                "evidence_recall_excl_empty": mean(rs.iter().filter(|r| !r.evidence_empty).map(|r| r.evidence_recall)),
            }))
        })
        .collect();
    let mut query_us: Vec<u128> = records.iter().map(|r| r.query_us).collect();
    query_us.sort_unstable();
    let mut summary = run_meta(manifest, profile, embed_max_tokens);
    let extra = json!({
        "benchmark": "locomo10",
        "complete": limit.is_none(),
        "dataset_sha256": sha,
        "dataset_commit": manifest["datasets"]["locomo10"]["commit"],
        "granularity": "session (MemPalace raw: speaker said, \"text\" lines)",
        "top_k": top_k,
        "category_labels_note": "numeric LoCoMo categories; MemPalace names them 1 Single-hop, 2 Temporal, 3 Temporal-inference, 4 Open-domain, 5 Adversarial, upstream does not name them",
        "wall_clock_s": wall.elapsed().as_secs_f64(),
        "ingest_ms_total": records.iter().map(|r| r.ingest_ms).sum::<u128>(),
        "query_us": {"p50": percentile(&query_us, 50.0), "p95": percentile(&query_us, 95.0), "p99": percentile(&query_us, 99.0)},
        "max_rss_bytes": max_rss_bytes(),
        "overall": {
            "n": records.len(),
            "empty_evidence": records.iter().filter(|r| r.evidence_empty).count(),
            "abstained": records.iter().filter(|r| r.abstained.is_some()).count(),
            "evidence_recall_mempalace": mean(records.iter().map(|r| r.evidence_recall)),
            "evidence_recall_excl_empty": mean(records.iter().filter(|r| !r.evidence_empty).map(|r| r.evidence_recall)),
            "recall_any@10_excl_empty": mean(records.iter().filter(|r| !r.evidence_empty).map(|r| r.metrics.at[&10].recall_any)),
            "mrr_excl_empty": mean(records.iter().filter(|r| !r.evidence_empty).map(|r| r.metrics.mrr)),
        },
        "per_category": per_category,
    });
    if let (Some(a), Some(b)) = (summary.as_object_mut(), extra.as_object()) {
        a.extend(b.clone());
    }
    let path = out.join(format!("{tag}.summary.json"));
    std::fs::write(&path, serde_json::to_string_pretty(&summary)?)?;
    println!("{}", serde_json::to_string_pretty(&summary["overall"])?);
    eprintln!("wrote {}", path.display());
    Ok(())
}

async fn longmemeval(
    manifest: &serde_json::Value,
    data: PathBuf,
    profile: Profile,
    out: PathBuf,
    limit: Option<usize>,
    embed_max_tokens: Option<usize>,
) -> Result<()> {
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
    let provider = Arc::new(LocalProvider::load_with_max_tokens(
        model,
        embed_max_tokens,
    )?);
    std::fs::create_dir_all(&out)?;
    let tag = format!(
        "longmemeval-{}{}",
        profile_name(profile),
        token_suffix(embed_max_tokens)
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
        "embedding": {"model": provider.model_id(), "dim": provider.dim(), "input_kind": "document/query (symmetric model; kind ignored)", "max_tokens": max_tokens_json(embed_max_tokens)},
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
