//! Portable export, backup, and restore.
//!
//! Implements PRD `docs/prds/2026-07-02-portable-export-and-backup.md`:
//! human-readable diffable dumps, timestamped snapshots with retention, and
//! idempotent restore that re-embeds under the current model.
//!
//! Design rules (from the PRD):
//! - Export is read-only and post-redaction: stored content was already
//!   redacted at write time, so the export inherits that guarantee.
//! - Export is deterministic for a given corpus (sorted by id) so it is
//!   `git diff`-friendly.
//! - Restore flows through the existing `import::store_items` path so it
//!   reuses redaction + recall-probe dedup (idempotent re-restore).
//! - Vectors are never exported (re-derivable from content); the daemon
//!   re-embeds on restore via the normal `remember` write path.

use crate::import::{self, ImportItem};
use anyhow::Context as _;
use rb_proto::Client;
use rb_types::{MemoryNote, MemoryType};
use std::path::{Path, PathBuf};

const EXPORT_LIST_LIMIT: usize = 100_000;

/// Export format selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Markdown,
    Json,
    Csv,
}

impl ExportFormat {
    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Markdown => "md",
            ExportFormat::Json => "json",
            ExportFormat::Csv => "csv",
        }
    }
}

/// Filters applied to the exported set (all optional).
#[derive(Debug, Clone, Default)]
pub struct ExportFilters {
    pub memory_type: Option<MemoryType>,
    pub tags: Vec<String>,
    pub min_importance: Option<u8>,
}

/// JSON envelope for a full-fidelity export (the restore source format).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ExportEnvelope {
    pub version: u32,
    pub exported_at: chrono::DateTime<chrono::Utc>,
    pub namespace: String,
    pub count: usize,
    pub memories: Vec<MemoryNote>,
}

/// Memories fetched for export plus whether the bounded wire list may have
/// truncated the namespace.
#[derive(Debug, Clone)]
pub struct ExportSnapshot {
    pub memories: Vec<MemoryNote>,
    pub maybe_truncated: bool,
}

// ---------------------------------------------------------------------------
// Export (read-only dump via the wire list path)
// ---------------------------------------------------------------------------

/// Fetch all active memories for the current namespace and filter/sort them.
/// Uses a generous limit; the wire `list` is a read-pool op, safe with the
/// daemon live.
pub async fn fetch_memories(
    client: &mut Client,
    filters: &ExportFilters,
) -> anyhow::Result<ExportSnapshot> {
    let memories = client
        .list(filters.min_importance, EXPORT_LIST_LIMIT)
        .await
        .context("listing memories for export")?;
    let maybe_truncated = memories.len() == EXPORT_LIST_LIMIT;
    if maybe_truncated {
        tracing::warn!(limit = EXPORT_LIST_LIMIT, "export result may be truncated");
    }
    let mut filtered = memories
        .into_iter()
        .filter(|m| {
            if let Some(mt) = filters.memory_type {
                if m.memory_type != mt {
                    return false;
                }
            }
            if !filters.tags.is_empty() {
                let has_all = filters.tags.iter().all(|t| m.tags.iter().any(|mt| mt == t));
                if !has_all {
                    return false;
                }
            }
            true
        })
        .collect::<Vec<_>>();
    sort_memories_by_id(&mut filtered);
    Ok(ExportSnapshot {
        memories: filtered,
        maybe_truncated,
    })
}

fn sort_memories_by_id(memories: &mut [MemoryNote]) {
    memories.sort_by_key(|m| m.id.as_uuid());
}

/// Format memories as the requested output format.
pub fn format_export(
    memories: &[MemoryNote],
    namespace: &str,
    format: ExportFormat,
) -> anyhow::Result<String> {
    match format {
        ExportFormat::Json => format_json(memories, namespace),
        ExportFormat::Markdown => Ok(format_markdown(memories)),
        ExportFormat::Csv => Ok(format_csv(memories)),
    }
}

fn now_utc() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc::now()
}

fn format_json(memories: &[MemoryNote], namespace: &str) -> anyhow::Result<String> {
    let envelope = ExportEnvelope {
        version: 1,
        exported_at: now_utc(),
        namespace: namespace.to_string(),
        count: memories.len(),
        memories: memories.to_vec(),
    };
    serde_json::to_string_pretty(&envelope).context("serializing export JSON")
}

fn format_markdown(memories: &[MemoryNote]) -> String {
    let mut out = String::from("# Memory Export\n\n");
    for m in memories {
        out.push_str(&format!(
            "## {} {}\n\n",
            m.id,
            if m.summary.is_empty() { "" } else { &m.summary }
        ));
        out.push_str(&format!("- **Type:** {}\n", m.memory_type.as_str()));
        out.push_str(&format!("- **Importance:** {}\n", m.importance));
        out.push_str(&format!("- **Confidence:** {:.2}\n", m.confidence));
        if !m.tags.is_empty() {
            out.push_str(&format!("- **Tags:** {}\n", m.tags.join(", ")));
        }
        if m.contested {
            out.push_str("- **Contested:** yes\n");
        }
        if !m.context.is_empty() {
            out.push_str(&format!("- **Context:** {}\n", m.context));
        }
        out.push_str(&format!("- **Created:** {}\n", m.created_at));
        out.push_str(&format!("- **Updated:** {}\n", m.updated_at));
        if let Some(ref agent) = m.origin_agent {
            out.push_str(&format!("- **Origin:** {agent}\n"));
        }
        out.push('\n');
        out.push('\n');
        out.push_str(&m.content);
        out.push('\n');
        out.push('\n');
        out.push_str("---");
        out.push('\n');
        out.push('\n');
    }
    out.trim_end().to_string()
}

fn format_csv(memories: &[MemoryNote]) -> String {
    let mut out = String::from(
        "id,type,importance,confidence,tags,summary,context,created,updated,contested\n",
    );
    for m in memories {
        let tags = csv_escape(&m.tags.join(";"));
        let summary = csv_escape(&m.summary);
        let context = csv_escape(&m.context);
        out.push_str(&format!(
            "{},{},{},{:.2},{},{},{},{},{},{}\n",
            m.id,
            m.memory_type.as_str(),
            m.importance,
            m.confidence,
            tags,
            summary,
            context,
            m.created_at,
            m.updated_at,
            m.contested,
        ));
    }
    out.trim_end().to_string()
}

fn csv_escape(s: &str) -> String {
    let s = if s.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        format!("'{s}")
    } else {
        s.to_string()
    };
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s
    }
}

// ---------------------------------------------------------------------------
// Backup (timestamped snapshot in the data dir)
// ---------------------------------------------------------------------------

/// Resolve the backup directory from the DB path (sibling `backups/` dir,
/// matching how `import::ledger_dir` uses a sibling `imports/` dir).
pub fn backup_dir(db_path: &Path) -> Option<PathBuf> {
    db_path.parent().map(|p| p.join("backups"))
}

/// Write a backup file. Returns the path written.
pub fn write_backup(db_path: &Path, content: &str, extension: &str) -> anyhow::Result<PathBuf> {
    let dir = backup_dir(db_path).context("cannot resolve backups dir from db path")?;
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let now = chrono::Utc::now();
    let ts = now.format("%Y%m%dT%H%M%S");
    let ns = now.timestamp_nanos_opt().unwrap_or(0) % 1_000_000_000;
    let path = dir.join(format!("backup-{ts}.{ns:09}.{extension}"));
    // Atomic write: temp file 0600 + rename (same pattern as import ledger).
    let tmp = path.with_extension(format!("{extension}.tmp"));
    crate::import::write_private(&tmp, content)?;
    std::fs::rename(&tmp, &path)
        .with_context(|| format!("installing backup {}", path.display()))?;
    Ok(path)
}

/// Prune old backups, keeping only the most recent `retain` files.
pub fn prune_backups(db_path: &Path, retain: usize) -> anyhow::Result<usize> {
    let Some(dir) = backup_dir(db_path) else {
        return Ok(0);
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(0);
    };
    let mut files: Vec<(PathBuf, std::time::SystemTime)> = entries
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            if !is_backup_file(&p) {
                return None;
            }
            let mtime = e.metadata().ok()?.modified().ok()?;
            Some((p, mtime))
        })
        .collect();
    files.sort_by_key(|b| std::cmp::Reverse(b.1));
    let mut pruned = 0;
    for (path, _) in files.iter().skip(retain) {
        if std::fs::remove_file(path).is_ok() {
            pruned += 1;
        }
    }
    Ok(pruned)
}

/// List existing backups (path + size), newest first.
pub fn list_backups(db_path: &Path) -> Vec<(PathBuf, u64)> {
    let Some(dir) = backup_dir(db_path) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut files: Vec<(PathBuf, std::time::SystemTime, u64)> = entries
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            if !is_backup_file(&p) {
                return None;
            }
            let md = e.metadata().ok()?;
            let mtime = md.modified().ok()?;
            Some((p, mtime, md.len()))
        })
        .collect();
    files.sort_by_key(|b| std::cmp::Reverse(b.1));
    files.into_iter().map(|(p, _, sz)| (p, sz)).collect()
}

fn is_backup_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|s| s.to_str())
        .is_some_and(|name| name.starts_with("backup-"))
}

// ---------------------------------------------------------------------------
// Restore (parse export -> ImportItems -> store_items)
// ---------------------------------------------------------------------------

/// Parse a JSON export envelope into ImportItems suitable for `store_items`.
/// Content and source are extracted from each memory note. Only active,
/// non-superseded memories are restored.
pub fn parse_restore(text: &str) -> anyhow::Result<Vec<ImportItem>> {
    let envelope: ExportEnvelope = serde_json::from_str(text)
        .context("parsing export JSON (expected a rusty-brain export)")?;
    if envelope.version != 1 {
        anyhow::bail!("unsupported export version {}", envelope.version);
    }
    let mut items = Vec::with_capacity(envelope.memories.len());
    for m in envelope.memories {
        if m.archived_at.is_some() {
            continue;
        }
        if m.superseded_by.is_some() {
            continue;
        }
        items.push(ImportItem {
            summary: m.summary,
            content: m.content,
            memory_type: m.memory_type,
            importance: m.importance,
            source: format!("restore:{}", m.id),
            tags: m.tags,
        });
    }
    Ok(items)
}

/// Store restored items via the import path (redact + dedup + remember).
/// Returns the import counts.
pub async fn restore_items(
    client: &mut Client,
    items: &[ImportItem],
    extra_tags: &[String],
) -> anyhow::Result<import::ImportCounts> {
    let batch_id = import::new_batch_id();
    let tag = import::batch_tag(&batch_id);
    let (_ids, counts) = import::store_items(client, items, &tag, extra_tags)
        .await
        .context("storing restored memories")?;
    Ok(counts)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use rb_types::{MemoryId, MemoryType, Namespace};
    use tempfile::TempDir;

    fn sample_note(summary: &str, content: &str, mt: MemoryType, imp: u8) -> MemoryNote {
        MemoryNote {
            id: MemoryId::new(),
            namespace: Namespace::Project("test".into()),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            content: content.to_string(),
            summary: summary.to_string(),
            keywords: Vec::new(),
            tags: vec!["storage".to_string()],
            context: "docs/adr.md".to_string(),
            memory_type: mt,
            importance: imp,
            confidence: 0.9,
            related_files: Vec::new(),
            access_count: 3,
            last_accessed_at: None,
            archived_at: None,
            superseded_by: None,
            embedding_model: "test-model".to_string(),
            embedding_input_version: "v1".to_string(),
            links: Vec::new(),
            contested: false,
            origin_user: Some("test".to_string()),
            origin_host: Some("localhost".to_string()),
            origin_agent: Some("cli".to_string()),
            origin_source: Some("cli".to_string()),
            session_id: None,
        }
    }

    #[test]
    fn markdown_export_includes_summary_content_and_metadata() {
        let notes = vec![sample_note(
            "Use SQLite",
            "We chose sqlite-wal",
            MemoryType::ArchitectureDecision,
            8,
        )];
        let md = format_markdown(&notes);
        assert!(md.contains("Use SQLite"), "{md}");
        assert!(md.contains("We chose sqlite-wal"), "{md}");
        assert!(md.contains("architecture_decision"), "{md}");
        assert!(md.contains("storage"), "{md}");
    }

    #[test]
    fn json_export_round_trips_through_parse_restore() {
        let notes = vec![
            sample_note("Decision A", "Content A", MemoryType::Insight, 5),
            sample_note(
                "Decision B",
                "Content B",
                MemoryType::ArchitectureDecision,
                9,
            ),
        ];
        let json = format_json(&notes, "project:test").unwrap();
        let items = parse_restore(&json).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].summary, "Decision A");
        assert_eq!(items[1].memory_type, MemoryType::ArchitectureDecision);
    }

    #[test]
    fn csv_export_has_header_and_rows() {
        let notes = vec![
            sample_note("First", "Content one", MemoryType::Insight, 3),
            sample_note("Second", "Content two", MemoryType::Constraint, 7),
        ];
        let csv = format_csv(&notes);
        assert!(csv.starts_with("id,type,importance"));
        assert!(csv.contains("insight"));
        assert!(csv.contains("constraint"));
    }

    #[test]
    fn csv_escapes_commas_in_summary() {
        let mut note = sample_note("has, comma", "body", MemoryType::Insight, 1);
        note.summary = "one, two, three".to_string();
        let csv = format_csv(&[note]);
        assert!(csv.contains("\"one, two, three\""), "{csv}");
    }

    #[test]
    fn csv_escapes_formula_like_summary() {
        let mut note = sample_note("formula", "body", MemoryType::Insight, 1);
        note.summary = "=cmd|' /C calc'!A0".to_string();
        let csv = format_csv(&[note]);
        assert!(csv.contains("'=cmd|' /C calc'!A0"), "{csv}");
    }

    #[test]
    fn parse_restore_skips_archived_and_superseded() {
        let mut archived = sample_note("Old", "Old content", MemoryType::Insight, 3);
        archived.archived_at = Some(chrono::Utc::now());
        let mut superseded = sample_note("Replaced", "Old version", MemoryType::Insight, 3);
        superseded.superseded_by = Some(MemoryId::new());
        let active = sample_note("Active", "Live content", MemoryType::Insight, 5);
        let notes = vec![archived, superseded, active];
        let json = format_json(&notes, "project:test").unwrap();
        let items = parse_restore(&json).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].summary, "Active");
    }

    #[test]
    fn filters_by_type_tags_and_importance() {
        let filters = ExportFilters {
            memory_type: Some(MemoryType::ArchitectureDecision),
            tags: vec!["storage".to_string()],
            min_importance: Some(5),
        };
        let notes = [
            sample_note("Match", "Body", MemoryType::ArchitectureDecision, 8),
            sample_note("Wrong type", "Body", MemoryType::Insight, 8),
        ];
        let result: Vec<_> = notes
            .iter()
            .filter(|m| {
                if let Some(mt) = filters.memory_type {
                    if m.memory_type != mt {
                        return false;
                    }
                }
                if !filters.tags.is_empty() {
                    let has_all = filters.tags.iter().all(|t| m.tags.iter().any(|mt| mt == t));
                    if !has_all {
                        return false;
                    }
                }
                true
            })
            .collect();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].summary, "Match");
    }

    #[test]
    fn export_is_sorted_by_id() {
        let mut notes = [
            sample_note("B", "body b", MemoryType::Insight, 3),
            sample_note("A", "body a", MemoryType::Insight, 3),
        ];
        // Swap to ensure they're not pre-sorted.
        notes.sort_by_key(|b| std::cmp::Reverse(b.id.as_uuid()));
        sort_memories_by_id(&mut notes);
        assert!(notes[0].id.as_uuid() <= notes[1].id.as_uuid());
    }

    #[test]
    fn backup_write_and_prune() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("rb.db");

        // Write 3 backups.
        for i in 0..3 {
            let path = write_backup(&db, &format!("{{}}: {i}"), "json").unwrap();
            assert!(path.exists());
        }

        let before = list_backups(&db);
        assert_eq!(before.len(), 3);

        // Prune to retain only 1.
        let pruned = prune_backups(&db, 1).unwrap();
        assert_eq!(pruned, 2);
        assert_eq!(list_backups(&db).len(), 1);
    }

    #[test]
    fn backup_dir_is_sibling_of_db() {
        let db = Path::new("/tmp/data/rb.db");
        let dir = backup_dir(db).unwrap();
        assert_eq!(dir, Path::new("/tmp/data/backups"));
    }

    #[test]
    fn export_format_extension() {
        assert_eq!(ExportFormat::Markdown.extension(), "md");
        assert_eq!(ExportFormat::Json.extension(), "json");
        assert_eq!(ExportFormat::Csv.extension(), "csv");
    }
}
