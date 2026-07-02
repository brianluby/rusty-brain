//! First-run cold-start and project import.
//!
//! Implements PRD `docs/prds/2026-07-02-init-and-project-import.md`: seed a
//! fresh brain from existing project context (CLAUDE.md, README, docs/, the git
//! log) so the first recall is non-empty, plus a general `import` for arbitrary
//! text/markdown.
//!
//! Design rules (from the PRD):
//! - No new storage schema. Everything flows through the existing wire
//!   `remember` op; the daemon does enrich -> embed -> store, exactly like the
//!   interactive `remember` command.
//! - Every external byte is redacted client-side via `rb-redact` BEFORE it
//!   touches the wire (the same discipline the capture hooks use), so a secret
//!   in a source file never reaches the DB.
//! - Each seeded memory carries an `import_batch:<id>` tag for reviewability
//!   and bulk rollback. A small sidecar ledger under the DB dir records the
//!   stored ids so `init --undo <batch>` removes exactly that set.
//! - Idempotent: a content-equality recall probe skips a candidate whose
//!   redacted content is already stored, so re-running `init` adds nothing.

use anyhow::Context as _;
use rb_proto::Client;
use rb_redact::redact;
use rb_types::{MemoryId, MemoryType};
use std::path::{Path, PathBuf};

/// Tag prefix stamping every seeded memory so a batch is reviewable and
/// bulk-undoable. The full tag is `import_batch:<batch_id>`.
pub const BATCH_TAG_PREFIX: &str = "import_batch:";

/// A single extracted memory candidate (pre-redaction; redaction happens at
/// store time so the dedup probe compares redacted-to-redacted).
#[derive(Debug, Clone, PartialEq)]
pub struct ImportItem {
    pub summary: String,
    pub content: String,
    pub memory_type: MemoryType,
    pub importance: u8,
    /// Human-readable provenance label (file path, "git log", "stdin"). Stored
    /// in the memory's `context` field so it shows in recall.
    pub source: String,
}

/// Counts for an import run.
#[derive(Debug, Default, Clone, Copy)]
pub struct ImportCounts {
    pub new: u64,
    pub skipped_duplicate: u64,
    pub failed: u64,
}

/// Counts for an undo run.
#[derive(Debug, Default, Clone, Copy)]
pub struct UndoCounts {
    /// Successful delete ops issued. Soft-archive (`delete`) is idempotent: a
    /// missing or already-archived id is an Ok no-op server-side, so re-running
    /// an undo re-issues the same ops and reports the same count.
    pub deleted: u64,
}

/// Knobs for a project scan.
#[derive(Debug, Clone, Copy)]
pub struct ScanOptions {
    pub max_files: usize,
    pub max_bytes: usize,
    pub importance: u8,
}

impl ScanOptions {
    /// Conservative defaults: at most 50 files, 64 KiB each, importance 6.
    pub fn defaults(importance: u8) -> Self {
        Self {
            max_files: 50,
            max_bytes: 65_536,
            importance,
        }
    }
}

/// Generate a fresh, filesystem-safe batch id.
pub fn new_batch_id() -> String {
    // Reuse the UUID generator already in the dependency closure (rb-types)
    // rather than pulling in the `uuid` crate directly here.
    format!("import-{}", MemoryId::new())
}

/// Build the full batch tag for a batch id.
pub fn batch_tag(batch_id: &str) -> String {
    format!("{BATCH_TAG_PREFIX}{batch_id}")
}

// ---------------------------------------------------------------------------
// Source adapters (pure; unit-tested)
// ---------------------------------------------------------------------------

/// Detect candidate sources under `root` and extract items. Best-effort: a
/// per-source error (unreadable file, non-UTF-8, git absent) is skipped, never
/// fatal. `root` is normally the git toplevel (falling back to the given dir).
pub fn scan_project(root: &Path, opts: ScanOptions) -> Vec<ImportItem> {
    let scan_root = git_toplevel(root).unwrap_or_else(|| root.to_path_buf());
    let mut items = Vec::new();
    let mut budget = opts.max_files;

    // Well-known root files, in priority order. Each is optional.
    for name in ["CLAUDE.md", "AGENTS.md", "README.md", "CHANGELOG.md"] {
        if budget == 0 {
            return items;
        }
        let path = scan_root.join(name);
        if path.is_file() {
            if let Some(item) = extract_well_known(&path, opts) {
                items.push(item);
                budget -= 1;
            }
        }
    }

    // Markdown under docs/ (recursive, bounded). Hidden dirs and `target/` are
    // skipped so build artifacts and VCS internals never get ingested.
    if budget > 0 {
        let docs = scan_root.join("docs");
        if docs.is_dir() {
            let mut md_files = Vec::new();
            collect_markdown(&docs, &mut md_files);
            md_files.sort();
            for path in md_files {
                if budget == 0 {
                    break;
                }
                if let Some(item) = extract_doc(&path, opts) {
                    items.push(item);
                    budget -= 1;
                }
            }
        }
    }

    // Recent decision-ish commits (bounded, best-effort, never fatal).
    if budget > 0 {
        for commit in recent_decision_commits(&scan_root, budget.min(30)) {
            items.push(commit);
        }
    }

    items
}

/// Extract one item from a file at `path` (used by `import <path>`). Returns
/// `None` if the file cannot be read or yields no text.
pub fn extract_file(
    path: &Path,
    max_bytes: usize,
    memory_type: MemoryType,
    importance: u8,
) -> Option<ImportItem> {
    let text = read_bounded_text(path, max_bytes)?;
    if text.trim().is_empty() {
        return None;
    }
    let source = path.display().to_string();
    Some(extract_text(&source, &text, memory_type, importance))
}

/// Extract one item from in-memory text (stdin or an already-read file). The
/// heuristic: the first heading or non-empty line becomes the summary; the full
/// (bounded) text becomes the content.
pub fn extract_text(
    label: &str,
    text: &str,
    memory_type: MemoryType,
    importance: u8,
) -> ImportItem {
    let summary = derive_summary(text, label);
    ImportItem {
        summary,
        content: text.to_string(),
        memory_type,
        importance,
        source: label.to_string(),
    }
}

fn extract_well_known(path: &Path, opts: ScanOptions) -> Option<ImportItem> {
    let text = read_bounded_text(path, opts.max_bytes)?;
    if text.trim().is_empty() {
        return None;
    }
    let base = path.file_name().and_then(|s| s.to_str()).unwrap_or("file");
    let (memory_type, imp) = well_known_kind(base, opts.importance);
    let source = path.display().to_string();
    Some(extract_text(&source, &text, memory_type, imp))
}

fn extract_doc(path: &Path, opts: ScanOptions) -> Option<ImportItem> {
    let text = read_bounded_text(path, opts.max_bytes)?;
    if text.trim().is_empty() {
        return None;
    }
    let lower = path.to_string_lossy().to_ascii_lowercase();
    let (memory_type, imp) = if lower.contains("adr") || lower.contains("decision") {
        (
            MemoryType::ArchitectureDecision,
            opts.importance.saturating_add(1).min(10),
        )
    } else {
        (MemoryType::Insight, opts.importance)
    };
    let source = path.display().to_string();
    Some(extract_text(&source, &text, memory_type, imp))
}

/// Pick a memory type + importance for a well-known root file.
fn well_known_kind(base: &str, importance: u8) -> (MemoryType, u8) {
    match base {
        "CLAUDE.md" | "AGENTS.md" => (MemoryType::Constraint, importance.saturating_add(1).min(10)),
        "CHANGELOG.md" => (MemoryType::Reference, importance.saturating_sub(1).max(1)),
        _ => (MemoryType::Insight, importance), // README and anything else
    }
}

/// Summary heuristic: first markdown heading, else first non-empty line, else a
/// fallback derived from `label`. Trimmed to a readable width.
fn derive_summary(text: &str, label: &str) -> String {
    for line in text.lines() {
        let t = line.trim();
        if let Some(heading) = t.strip_prefix('#') {
            let heading = heading.trim_start_matches('#').trim();
            if !heading.is_empty() {
                return truncate_summary(heading);
            }
        }
        if !t.is_empty() {
            return truncate_summary(t);
        }
    }
    // Last resort: the source label (filename), cleaned up.
    Path::new(label)
        .file_name()
        .and_then(|s| s.to_str())
        .map(truncate_summary)
        .unwrap_or_else(|| "imported memory".to_string())
}

fn truncate_summary(s: &str) -> String {
    const MAX: usize = 100;
    if s.chars().count() <= MAX {
        return s.trim().to_string();
    }
    let mut out: String = s.chars().take(MAX).collect();
    if let Some(cut) = out.rfind(' ') {
        out.truncate(cut);
    }
    out.push_str("...");
    out
}

/// Read up to `max_bytes` from a file as lossy UTF-8. Returns `None` on error,
/// empty, or binary content (contains a NUL byte).
fn read_bounded_text(path: &Path, max_bytes: usize) -> Option<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).ok()?;
    let mut buf = Vec::with_capacity(max_bytes.min(8 * 1024));
    // `take` caps the read so a giant file is not fully loaded.
    let n = file
        .by_ref()
        .take(max_bytes.try_into().unwrap_or(u64::MAX))
        .read_to_end(&mut buf)
        .ok()?;
    if n == 0 || buf.contains(&0) {
        return None;
    }
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// Recursive, bounded `.md`/`.txt` collector. Skips hidden entries and common
/// noise dirs (`target`, `node_modules`, `.git`).
fn collect_markdown(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if file_type.is_dir() {
            if name.starts_with('.') || matches!(name, "target" | "node_modules") {
                continue;
            }
            collect_markdown(&path, out);
        } else if file_type.is_file()
            && (name.ends_with(".md") || name.ends_with(".txt"))
            && !name.starts_with('.')
        {
            out.push(path);
        }
    }
}

/// Best-effort git toplevel resolution (shells out, like namespace detection).
fn git_toplevel(dir: &Path) -> Option<PathBuf> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&output.stdout);
    let trimmed = s.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(PathBuf::from(trimmed))
    }
}

/// Whether a commit subject looks decision-grade (a durable choice a future
/// session would act on), versus routine churn. Pure so it can be unit-tested.
fn is_decisionish(subject: &str) -> bool {
    const KEYWORDS: &[&str] = &[
        "because",
        "why",
        "rationale",
        "decision",
        "decide",
        "adopt",
        "adopted",
        "deprecate",
        "deprecated",
        "replace",
        "replaced",
        "migrate",
        "switch",
        "switched",
        "should",
        "must",
    ];
    let lower = subject.to_ascii_lowercase();
    KEYWORDS.iter().any(|k| lower.contains(k))
}

/// Recent commits whose subject is decision-grade. Best-effort: git absent or a
/// non-repo dir yields nothing. Bounded by `limit`.
fn recent_decision_commits(root: &Path, limit: usize) -> Vec<ImportItem> {
    let limit_u32 = u32::try_from(limit).unwrap_or(u32::MAX);
    // Subject, body, hash, and record terminator are NUL-separated so commit
    // bodies may contain newlines without splitting records.
    let output = match std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "log",
            &format!("-{limit_u32}"),
            "--format=%s%x00%b%x00%H%x00",
        ])
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let mut items = Vec::new();
    let mut parts = text.split('\0');
    while let (Some(subject), Some(body), Some(hash)) = (parts.next(), parts.next(), parts.next()) {
        let subject = subject.trim();
        let body = body.trim();
        let hash = hash.trim();
        if subject.is_empty() || !is_decisionish(subject) {
            continue;
        }
        let mut content = format!("commit {hash}: {subject}");
        if !body.is_empty() {
            content.push_str("\n\n");
            content.push_str(body);
        }
        items.push(ImportItem {
            summary: truncate_summary(subject),
            content,
            memory_type: MemoryType::Insight,
            importance: 4,
            source: format!("git log {hash}"),
        });
    }
    items
}

// ---------------------------------------------------------------------------
// Store / dedup / undo (async, over the existing wire path)
// ---------------------------------------------------------------------------

/// Store `items` over a single connection. Each item is redacted, then a
/// read-only recall probe skips an exact-content duplicate (so re-running an
/// import adds nothing).
///
/// Returns the per-item outcome (the stored id, or `None` for skipped/failed)
/// and aggregate counts.
pub async fn store_items(
    client: &mut Client,
    items: &[ImportItem],
    tag: &str,
    extra_tags: &[String],
) -> anyhow::Result<(Vec<Option<MemoryId>>, ImportCounts)> {
    let mut ids = Vec::with_capacity(items.len());
    let mut counts = ImportCounts::default();

    for item in items {
        // Redact BEFORE probing/storing so the dedup comparison is
        // redacted-to-redacted (stored content was redacted on the first run).
        let content = redact(&item.content);
        let context = redact(&item.source);

        if content.trim().is_empty() {
            counts.failed += 1;
            ids.push(None);
            continue;
        }

        if is_duplicate(client, &content).await? {
            counts.skipped_duplicate += 1;
            ids.push(None);
            continue;
        }

        let mut tags = Vec::with_capacity(1 + extra_tags.len());
        tags.push(tag.to_string());
        tags.extend(extra_tags.iter().cloned());
        match client
            .remember(
                content,
                Some(context),
                item.memory_type,
                item.importance,
                Vec::new(),
                tags,
                Vec::new(),
                None,
            )
            .await
        {
            Ok(id) => {
                counts.new += 1;
                ids.push(Some(id));
            }
            Err(e) => {
                tracing::warn!(error = %e, source = %item.source, "import: remember failed");
                counts.failed += 1;
                ids.push(None);
            }
        }
    }

    Ok((ids, counts))
}

/// Read-only content-equality probe: `true` if an existing memory's redacted
/// content exactly matches `redacted_content`. Reuses the existing recall path
/// (issues zero writer ops - W1.8), so it is safe to call before every store.
async fn is_duplicate(client: &mut Client, redacted_content: &str) -> anyhow::Result<bool> {
    // Query with a bounded prefix (keeps the embed/FTS cheap); the exact match
    // is confirmed by full-content equality below.
    let query: String = redacted_content.chars().take(500).collect();
    if query.trim().is_empty() {
        return Ok(false);
    }
    let (results, _degraded) = client
        .recall_with_status(query, None, Vec::new(), 20)
        .await?;
    let target = redacted_content.trim();
    Ok(results.iter().any(|r| r.memory.content.trim() == target))
}

/// Persist the undo ledger for a batch (the stored ids). Co-located with the DB
/// so it is naturally isolated per data dir (tests set `XDG_DATA_HOME`).
pub fn write_ledger(db_path: &Path, batch_id: &str, ids: &[MemoryId]) -> anyhow::Result<()> {
    let Some(dir) = ledger_dir(db_path) else {
        anyhow::bail!("cannot resolve imports dir from db path");
    };
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join(safe_ledger_name(batch_id));
    let payload = serde_json::json!({
        "batch": batch_id,
        "ids": ids.iter().map(|id| id.to_string()).collect::<Vec<_>>(),
    });
    // Atomic write: temp file created 0600 on Unix + rename.
    let tmp = path.with_extension("json.tmp");
    write_private(&tmp, &payload.to_string())?;
    std::fs::rename(&tmp, &path)
        .with_context(|| format!("installing ledger {}", path.display()))?;
    Ok(())
}

/// Read the ids recorded for a batch, if a ledger exists.
pub fn read_ledger(db_path: &Path, batch_id: &str) -> Option<Vec<MemoryId>> {
    use std::str::FromStr as _;
    let dir = ledger_dir(db_path)?;
    let path = dir.join(safe_ledger_name(batch_id));
    let text = std::fs::read_to_string(&path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let arr = value.get("ids")?.as_array()?;
    let mut ids = Vec::new();
    for v in arr {
        let s = v.as_str()?;
        ids.push(MemoryId::from_str(s).ok()?);
    }
    Some(ids)
}

/// One undoable import batch, for discoverability (`init --list-batches`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchInfo {
    pub id: String,
    pub count: usize,
}

/// List all import batches with ledgered ids. Sorted by id for stable output.
pub fn list_batches(db_path: &Path) -> Vec<BatchInfo> {
    let Some(dir) = ledger_dir(db_path) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        if id.is_empty() {
            continue;
        }
        let count = read_ledger(db_path, &id).map(|ids| ids.len()).unwrap_or(0);
        out.push(BatchInfo { id, count });
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// Undo a batch: delete each ledgered id over the wire (soft-archive cascades
/// to vectors/FTS server-side, and is idempotent for already-archived ids),
/// then remove the ledger.
pub async fn undo_batch(
    client: &mut Client,
    db_path: &Path,
    batch_id: &str,
) -> anyhow::Result<UndoCounts> {
    let ids = read_ledger(db_path, batch_id).context(
        "no import ledger found for that batch id (run `rusty-brain init` to list batches)",
    )?;
    let mut counts = UndoCounts::default();
    for id in &ids {
        client
            .delete(id.clone())
            .await
            .context("deleting imported memory")?;
        counts.deleted += 1;
    }
    // Best-effort ledger removal; a failure here is not a data problem.
    if let Some(dir) = ledger_dir(db_path) {
        let _ = std::fs::remove_file(dir.join(safe_ledger_name(batch_id)));
    }
    Ok(counts)
}

fn ledger_dir(db_path: &Path) -> Option<PathBuf> {
    db_path.parent().map(|p| p.join("imports"))
}

fn safe_ledger_name(batch_id: &str) -> String {
    // batch ids are already filesystem-safe (`import-<uuid>`); sanitize
    // defensively so a user-supplied `--undo` value cannot escape the dir.
    let cleaned: String = batch_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{cleaned}.json")
}

#[cfg(unix)]
pub fn write_private(path: &Path, content: &str) -> anyhow::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt as _;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("creating {}", path.display()))?;
    f.write_all(content.as_bytes())
        .with_context(|| format!("writing {}", path.display()))
}

#[cfg(not(unix))]
pub fn write_private(path: &Path, content: &str) -> anyhow::Result<()> {
    std::fs::write(path, content).with_context(|| format!("writing {}", path.display()))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn batch_tag_uses_the_prefix() {
        assert_eq!(batch_tag("import-abc"), "import_batch:import-abc");
        assert!(new_batch_id().starts_with("import-"));
    }

    #[test]
    fn well_known_kind_maps_policy_and_changelog() {
        assert_eq!(well_known_kind("CLAUDE.md", 6), (MemoryType::Constraint, 7));
        assert_eq!(
            well_known_kind("AGENTS.md", 10),
            (MemoryType::Constraint, 10)
        );
        assert_eq!(
            well_known_kind("CHANGELOG.md", 6),
            (MemoryType::Reference, 5)
        );
        assert_eq!(well_known_kind("README.md", 6), (MemoryType::Insight, 6));
    }

    #[test]
    fn derive_summary_uses_first_heading_then_first_line() {
        assert_eq!(
            derive_summary("# My Project\n\nbody", "README.md"),
            "My Project"
        );
        assert_eq!(
            derive_summary("no heading here\nmore", "x"),
            "no heading here"
        );
        assert_eq!(derive_summary("\n\n   \n", "docs/x.md"), "x.md");
    }

    #[test]
    fn derive_summary_strips_all_heading_markers() {
        assert_eq!(derive_summary("## Design\nbody", "x"), "Design");
        assert_eq!(
            derive_summary("### Deep Section\nbody", "x"),
            "Deep Section"
        );
    }

    #[test]
    fn truncate_summary_is_idempotent_under_limit_and_adds_ellipsis_over() {
        let short = "a short summary";
        assert_eq!(truncate_summary(short), short);
        let long: String = "word ".repeat(40);
        let out = truncate_summary(&long);
        assert!(out.ends_with("..."));
        assert!(out.chars().count() <= 103);
    }

    #[test]
    fn is_decisionish_matches_rationale_keywords() {
        assert!(is_decisionish("Adopt tokio over async-std"));
        assert!(is_decisionish("refactor: why we moved to sqlite"));
        assert!(!is_decisionish("fix typo in readme"));
        assert!(!is_decisionish("update deps"));
    }

    #[test]
    fn recent_decision_commits_preserves_multiline_bodies() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let status = std::process::Command::new("git")
            .arg("init")
            .arg(root)
            .status()
            .unwrap();
        assert!(status.success());
        for args in [
            ["config", "user.email", "test@example.invalid"],
            ["config", "user.name", "Test User"],
        ] {
            let status = std::process::Command::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .status()
                .unwrap();
            assert!(status.success());
        }
        fs::write(root.join("README.md"), "demo").unwrap();
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["add", "README.md"])
            .status()
            .unwrap();
        assert!(status.success());
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args([
                "commit",
                "-m",
                "Adopt sqlite for storage",
                "-m",
                "First rationale line\n\nSecond rationale line",
            ])
            .status()
            .unwrap();
        assert!(status.success());

        let items = recent_decision_commits(root, 5);

        assert_eq!(items.len(), 1);
        assert!(items[0].content.contains("First rationale line"));
        assert!(items[0].content.contains("Second rationale line"));
    }

    #[test]
    fn safe_ledger_name_neutralizes_path_traversal() {
        assert_eq!(safe_ledger_name("import-abc"), "import-abc.json");
        assert_eq!(
            safe_ledger_name("../../etc/passwd"),
            "______etc_passwd.json"
        );
    }

    #[test]
    fn read_bounded_text_skips_binary_and_empty() {
        let tmp = TempDir::new().unwrap();
        let bin = tmp.path().join("bin");
        fs::write(&bin, b"a\x00b").unwrap();
        assert!(read_bounded_text(&bin, 1024).is_none());

        let empty = tmp.path().join("empty");
        fs::write(&empty, b"").unwrap();
        assert!(read_bounded_text(&empty, 1024).is_none());
    }

    #[test]
    fn read_bounded_text_caps_at_max_bytes() {
        let tmp = TempDir::new().unwrap();
        let big = tmp.path().join("big.txt");
        fs::write(&big, b"abcdef").unwrap();
        let got = read_bounded_text(&big, 3).unwrap();
        assert_eq!(got, "abc");
    }

    #[test]
    fn extract_text_carries_source_as_summary_fallback() {
        let item = extract_text("stdin", "# Decision\nbody", MemoryType::Insight, 5);
        assert_eq!(item.summary, "Decision");
        assert_eq!(item.content, "# Decision\nbody");
        assert_eq!(item.source, "stdin");
        assert_eq!(item.memory_type, MemoryType::Insight);
    }

    #[test]
    fn scan_project_reads_well_known_and_docs_and_skips_target() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::write(
            root.join("README.md"),
            "# Demo\n\nWe adopt sqlite for storage.",
        )
        .unwrap();
        fs::write(root.join("CLAUDE.md"), "# Policy\nNever commit secrets.").unwrap();
        let docs = root.join("docs");
        fs::create_dir_all(docs.join("adr")).unwrap();
        fs::write(
            docs.join("adr").join("0001-vecs.md"),
            "# ADR 1\nWe chose sqlite-vec.",
        )
        .unwrap();
        fs::write(docs.join("notes.md"), "# Notes\nSome notes.").unwrap();
        // Noise that must be ignored.
        fs::create_dir_all(root.join("target")).unwrap();
        fs::write(root.join("target").join("noise.md"), "build junk").unwrap();

        let items = scan_project(root, ScanOptions::defaults(6));
        let sources: Vec<&str> = items.iter().map(|i| i.source.as_str()).collect();
        // README + CLAUDE + 2 docs; target excluded.
        assert!(
            sources.iter().all(|s| !s.contains("target")),
            "target must be skipped: {sources:?}"
        );
        let has_readme = items.iter().any(|i| i.source.ends_with("README.md"));
        let has_claude = items
            .iter()
            .any(|i| i.memory_type == MemoryType::Constraint && i.source.ends_with("CLAUDE.md"));
        let has_adr = items
            .iter()
            .any(|i| i.memory_type == MemoryType::ArchitectureDecision);
        assert!(has_readme, "README ingested: {sources:?}");
        assert!(has_claude, "CLAUDE.md ingested as Constraint");
        assert!(has_adr, "ADR doc ingested as ArchitectureDecision");
    }

    #[test]
    fn write_then_read_ledger_round_trips() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("mem.db");
        let ids = vec![MemoryId::new(), MemoryId::new()];
        write_ledger(&db, "import-xyz", &ids).unwrap();
        let back = read_ledger(&db, "import-xyz").unwrap();
        assert_eq!(back, ids);
    }

    #[test]
    fn read_ledger_returns_none_for_missing_batch() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("mem.db");
        assert!(read_ledger(&db, "import-missing").is_none());
    }
}
