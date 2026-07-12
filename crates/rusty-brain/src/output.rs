//! Pure rendering of results to human text or JSON (stdout).

use crate::import::{BatchInfo, ImportCounts, ImportItem, UndoCounts};
use rb_redact::redact;
use rb_types::{MemoryNote, SearchResult};

/// Render recall hits. JSON: the raw `Vec<SearchResult>`. Human: one line per hit.
pub fn render_recall(results: &[SearchResult], json: bool) -> String {
    if json {
        return serde_json::to_string_pretty(results).unwrap_or_else(|e| {
            tracing::warn!(error = %e, "failed to render recall JSON");
            "[]".to_string()
        });
    }
    if results.is_empty() {
        // W1.3 empty state: recall may legitimately return nothing (score
        // floor / empty corpus); mirror the rb-mcp hint wording.
        return "No stored memories match.".to_string();
    }
    let mut out = String::new();
    for r in results {
        let summary = if r.memory.summary.is_empty() {
            r.memory.content.as_str()
        } else {
            r.memory.summary.as_str()
        };
        // Surface the contested flag (Feature C) inline for the human reader.
        let contested = if r.memory.contested {
            " [contested]"
        } else {
            ""
        };
        out.push_str(&format!(
            "[{:.2}] {} ({}){} {}\n",
            r.score,
            r.memory.id,
            r.memory.memory_type.as_str(),
            contested,
            summary
        ));
    }
    out.trim_end().to_string()
}

/// Comma-joined anchor labels (`file:src/a.rs:3-9, commit:abc123`) — the one
/// formatting shared by every human surface that lists anchors.
fn anchor_labels(n: &MemoryNote) -> String {
    let labels: Vec<String> = n.anchors.iter().map(ToString::to_string).collect();
    labels.join(", ")
}

/// Compact one-line anchor suffix for human renderings (typed code anchors,
/// ANC-4: `graph <id>` and friends list anchors): ` [anchors: file:src/a.rs,
/// commit:abc123]`, or empty when the note has none.
fn anchors_suffix(n: &MemoryNote) -> String {
    if n.anchors.is_empty() {
        return String::new();
    }
    format!(" [anchors: {}]", anchor_labels(n))
}

/// Render a list of notes (used by `list`, `graph`, and the `context` halves).
pub fn render_notes(notes: &[MemoryNote], json: bool) -> String {
    if json {
        return serde_json::to_string_pretty(notes).unwrap_or_else(|e| {
            tracing::warn!(error = %e, "failed to render notes JSON");
            "[]".to_string()
        });
    }
    if notes.is_empty() {
        return "No memories.".to_string();
    }
    let mut out = String::new();
    for n in notes {
        let summary = if n.summary.is_empty() {
            n.content.as_str()
        } else {
            n.summary.as_str()
        };
        // Surface the contested flag (Feature C) inline for the human reader.
        let contested = if n.contested { " [contested]" } else { "" };
        out.push_str(&format!(
            "{} (imp {}, {}){} {}{}\n",
            n.id,
            n.importance,
            n.memory_type.as_str(),
            contested,
            summary,
            anchors_suffix(n)
        ));
    }
    out.trim_end().to_string()
}

/// Render a single fetched memory (or a not-found message).
pub fn render_get(memory: &Option<MemoryNote>, json: bool) -> String {
    if json {
        return serde_json::to_string_pretty(memory).unwrap_or_else(|e| {
            tracing::warn!(error = %e, "failed to render memory JSON");
            "null".to_string()
        });
    }
    match memory {
        Some(n) => {
            // Surface the contested flag (Feature C) on the get read path too, so
            // every human surface (recall/list/context/get) marks a contradicted note.
            let contested = if n.contested { " [contested]" } else { "" };
            let anchors = if n.anchors.is_empty() {
                String::new()
            } else {
                format!("\nanchors: {}", anchor_labels(n))
            };
            format!(
                "{}{}\nnamespace: {}\ntype: {}\nimportance: {}{}\n\n{}",
                n.id,
                contested,
                n.namespace.as_db_string(),
                n.memory_type.as_str(),
                n.importance,
                anchors,
                n.content
            )
        }
        None => "Memory not found.".to_string(),
    }
}

/// Render a remembered id (json: an object `{ "id": "<uuid>" }`).
pub fn render_remembered(id: &rb_types::MemoryId, json: bool) -> String {
    if json {
        format!("{{\"id\":\"{id}\"}}")
    } else {
        format!("Remembered {id}")
    }
}

/// Render a bulk-remember acknowledgement (`remember --batch`). json: an object
/// `{ "count": <n> }`; human: `Remembered <n> memories`.
pub fn render_batch_remembered(count: u64, json: bool) -> String {
    if json {
        format!("{{\"count\":{count}}}")
    } else {
        format!("Remembered {count} memories")
    }
}

/// Render the candidate import plan before writes happen.
pub fn render_import_plan(items: &[ImportItem], json: bool) -> String {
    if json {
        let items: Vec<_> = items
            .iter()
            .map(|item| {
                serde_json::json!({
                    "source": redact(&item.source),
                    "summary": redact(&item.summary),
                    "type": item.memory_type.as_str(),
                    "importance": item.importance,
                })
            })
            .collect();
        return serde_json::json!({"planned": items.len(), "items": items}).to_string();
    }
    if items.is_empty() {
        return "No import candidates found.".to_string();
    }
    let mut out = format!("Import plan: {} candidate memories\n", items.len());
    for item in items {
        out.push_str(&format!(
            "- [{} imp {}] {} ({})\n",
            item.memory_type.as_str(),
            item.importance,
            redact(&item.summary),
            redact(&item.source)
        ));
    }
    out.trim_end().to_string()
}

/// Render an import result. `dry_run` means no writes happened; counts still
/// include duplicate probes.
pub fn render_import_result(
    batch_id: &str,
    counts: ImportCounts,
    dry_run: bool,
    json: bool,
) -> String {
    if json {
        return serde_json::json!({
            "batch": batch_id,
            "dry_run": dry_run,
            "new": counts.new,
            "skipped_duplicate": counts.skipped_duplicate,
            "failed": counts.failed,
        })
        .to_string();
    }
    let verb = if dry_run {
        "would remember"
    } else {
        "remembered"
    };
    let mut out = format!(
        "Import batch {batch_id}: {verb} {} memories (skipped_duplicate={} failed={})",
        counts.new, counts.skipped_duplicate, counts.failed
    );
    if !dry_run && counts.new > 0 {
        out.push_str(&format!("\nUndo with: rusty-brain init --undo {batch_id}"));
    }
    out
}

/// Render an import-batch undo acknowledgement.
pub fn render_import_undo(batch_id: &str, counts: UndoCounts, json: bool) -> String {
    if json {
        serde_json::json!({"batch": batch_id, "deleted": counts.deleted}).to_string()
    } else {
        format!(
            "Undid import batch {batch_id}: deleted {} memories",
            counts.deleted
        )
    }
}

/// Render undoable import batches.
pub fn render_import_batches(batches: &[BatchInfo], json: bool) -> String {
    if json {
        let batches: Vec<_> = batches
            .iter()
            .map(|batch| serde_json::json!({"batch": batch.id, "count": batch.count}))
            .collect();
        return serde_json::json!({"batches": batches}).to_string();
    }
    if batches.is_empty() {
        return "No import batches found.".to_string();
    }
    let mut out = String::from("Import batches:\n");
    for batch in batches {
        out.push_str(&format!("- {} ({} memories)\n", batch.id, batch.count));
    }
    out.trim_end().to_string()
}

/// Render a backup result (path written, memory count, pruned count).
pub fn render_backup_result(
    path: &std::path::Path,
    count: usize,
    pruned: usize,
    json: bool,
) -> String {
    if json {
        serde_json::json!({
            "path": path.display().to_string(),
            "memories": count,
            "pruned": pruned,
        })
        .to_string()
    } else {
        let mut out = format!("Backup written: {} ({} memories)", path.display(), count);
        if pruned > 0 {
            out.push_str(&format!(", pruned {pruned} old backup(s)"));
        }
        out
    }
}

/// Render existing backups (human: one line per file; json: array).
pub fn render_backup_list(backups: &[(std::path::PathBuf, u64)], json: bool) -> String {
    if json {
        let items: Vec<_> = backups
            .iter()
            .map(|(p, sz)| {
                serde_json::json!({
                    "path": p.display().to_string(),
                    "bytes": sz,
                })
            })
            .collect();
        serde_json::json!({"backups": items}).to_string()
    } else if backups.is_empty() {
        "No backups found.".to_string()
    } else {
        let mut out = String::from("Backups:\n");
        for (path, size) in backups {
            out.push_str(&format!("- {} ({})\n", path.display(), format_size(*size)));
        }
        out.trim_end().to_string()
    }
}

/// Render a restore dry-run plan.
pub fn render_restore_plan(items: &[ImportItem], json: bool) -> String {
    if json {
        let items: Vec<_> = items
            .iter()
            .map(|item| {
                serde_json::json!({
                    "summary": redact(&item.summary),
                    "type": item.memory_type.as_str(),
                    "importance": item.importance,
                    "source": redact(&item.source),
                })
            })
            .collect();
        serde_json::json!({"planned": items.len(), "items": items}).to_string()
    } else if items.is_empty() {
        "No memories to restore.".to_string()
    } else {
        let mut out = format!("Restore plan: {} memories\n", items.len());
        for item in items {
            out.push_str(&format!(
                "- [{} imp {}] {} ({})\n",
                item.memory_type.as_str(),
                item.importance,
                redact(&item.summary),
                redact(&item.source)
            ));
        }
        out.trim_end().to_string()
    }
}

fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
    }
}

/// Render a successful update acknowledgement.
pub fn render_updated(json: bool) -> String {
    if json {
        "{\"updated\":true}".to_string()
    } else {
        "Updated".to_string()
    }
}

/// Render a successful link acknowledgement.
pub fn render_linked(json: bool) -> String {
    if json {
        "{\"linked\":true}".to_string()
    } else {
        "Linked".to_string()
    }
}

/// Render a successful feedback acknowledgement, echoing the post-nudge trust prior.
pub fn render_feedback(confidence: f32, json: bool) -> String {
    if json {
        format!("{{\"confidence\":{confidence}}}")
    } else {
        format!("Feedback recorded (confidence now {confidence:.2})")
    }
}

/// Render a successful delete acknowledgement.
pub fn render_deleted(json: bool) -> String {
    if json {
        "{\"deleted\":true}".to_string()
    } else {
        "Deleted".to_string()
    }
}

/// Render the `context` payload.
pub fn render_context(
    recent: &[MemoryNote],
    important: &[MemoryNote],
    total: usize,
    json: bool,
) -> String {
    if json {
        let value = serde_json::json!({
            "recent": recent,
            "important": important,
            "total": total,
        });
        return serde_json::to_string_pretty(&value).unwrap_or_else(|e| {
            tracing::warn!(error = %e, "failed to render context JSON");
            "{}".to_string()
        });
    }
    let mut out = format!("Context ({total} memories total)\n\nRecent:\n");
    out.push_str(&render_notes(recent, false));
    out.push_str("\n\nImportant:\n");
    out.push_str(&render_notes(important, false));
    out
}

/// Threshold below which the feedback ratio gets a low-N caveat instead of
/// being presented as a confident signal (doctor/stats PRD risk mitigation).
const FEEDBACK_LOW_N: u64 = 10;

/// Render the `stats` payload: value/health aggregates for one namespace.
/// Counts and ids only — never memory content. JSON: `{stats, provider_model,
/// writer_alive}` with a derived `feedback.net`.
pub fn render_stats(
    stats: &rb_types::MemoryStats,
    provider_model: &str,
    writer_alive: bool,
    json: bool,
) -> String {
    if json {
        let mut value = serde_json::json!({
            "stats": stats,
            "provider_model": provider_model,
            "writer_alive": writer_alive,
        });
        // Derived net trend rides along so scripts need no client-side math.
        if let Some(fb) = value
            .pointer_mut("/stats/feedback")
            .and_then(|v| v.as_object_mut())
        {
            fb.insert("net".to_string(), serde_json::json!(stats.feedback.net()));
        }
        return serde_json::to_string_pretty(&value).unwrap_or_else(|e| {
            tracing::warn!(error = %e, "failed to render stats JSON");
            "{}".to_string()
        });
    }

    let mut out = format!(
        "Memory stats for {} (last {} days)\n\n",
        stats.namespace, stats.window_days
    );
    out.push_str(&format!(
        "corpus: {} live, {} archived, {} vectors\n",
        stats.live, stats.archived, stats.vectors
    ));
    out.push_str(&format!(
        "recall: {} total accesses; {} recalled in window; {} never recalled\n",
        stats.total_accesses, stats.accessed_in_window, stats.never_recalled_live
    ));
    let fb = &stats.feedback;
    if fb.total() == 0 {
        out.push_str(
            "feedback: no feedback recorded yet (use `rusty-brain feedback <id> --kind ...`)\n",
        );
    } else {
        out.push_str(&format!(
            "feedback: {} helpful, {} wrong, {} stale (net {:+})\n",
            fb.helpful,
            fb.wrong,
            fb.stale,
            fb.net()
        ));
        if fb.total() < FEEDBACK_LOW_N {
            out.push_str(&format!(
                "  note: only {} feedback event(s) — low-confidence signal\n",
                fb.total()
            ));
        }
    }
    out.push_str(&format!("contested: {}\n", stats.contested));
    out.push_str(&format!("re-embed backlog: {}\n", stats.reembed_pending));
    // RET-4 visibility: only rendered when a [retention] policy exists /
    // a sweep ever ran — "0 eligible" and "no policy" must not look alike.
    if let Some(eligible) = stats.retention_eligible {
        let last = match stats.last_forget_at {
            Some(ts) => format!("last forget {}", format_ts(ts)),
            None => "last forget never".to_string(),
        };
        out.push_str(&format!(
            "retention: {eligible} eligible to forget; {last}\n"
        ));
    } else if let Some(ts) = stats.last_forget_at {
        out.push_str(&format!(
            "retention: no policy configured; last forget {}\n",
            format_ts(ts)
        ));
    }
    if !stats.top_recalled.is_empty() {
        out.push_str("top recalled:\n");
        for t in &stats.top_recalled {
            out.push_str(&format!("  {}x {}\n", t.access_count, t.id));
        }
    }
    if !stats.created_per_day.is_empty() {
        out.push_str("growth (created per day):\n");
        for b in &stats.created_per_day {
            out.push_str(&format!("  {}: {}\n", b.day, b.count));
        }
    }
    out.trim_end().to_string()
}

/// Everything the extended `status` line-up needs (DOC-1), gathered by the
/// caller: the ping reply, the stats payload, and a client-side stat of the
/// DB file mode (`None` when the file could not be inspected).
pub struct StatusView<'a> {
    pub contract_version: u32,
    pub recall_channels: Option<rb_proto::RecallChannelTotals>,
    pub stats: &'a rb_types::MemoryStats,
    pub provider_model: &'a str,
    pub writer_alive: bool,
    pub db_file_mode: Option<u32>,
}

/// Render the extended `status` payload (daemon up, writer health, embedding
/// identity, DB path/mode, WAL size, corpus counts, namespace).
pub fn render_status(view: &StatusView<'_>, json: bool) -> String {
    let mode_str = view.db_file_mode.map(|m| format!("{m:04o}"));
    if json {
        let value = serde_json::json!({
            "ok": true,
            "contract_version": view.contract_version,
            "writer_alive": view.writer_alive,
            "provider_model": view.provider_model,
            "db": {
                "path": view.stats.db_path,
                "file_mode": mode_str,
                "wal_bytes": view.stats.wal_bytes,
                "embedding_model": view.stats.db_embedding_model,
            },
            "memories": {
                "live": view.stats.live,
                "archived": view.stats.archived,
            },
            "vectors": view.stats.vectors,
            "namespace": view.stats.namespace,
            "recall_channels": view.recall_channels.map(|c| serde_json::json!({
                "recalls": c.recalls,
                "fts_hits": c.fts_hits,
                "vector_hits": c.vector_hits,
                "graph_hits": c.graph_hits,
            })),
        });
        return serde_json::to_string_pretty(&value).unwrap_or_else(|e| {
            tracing::warn!(error = %e, "failed to render status JSON");
            "{}".to_string()
        });
    }

    let mut out = format!("daemon: ok (contract v{})\n", view.contract_version);
    out.push_str(&format!(
        "writer: {}\n",
        if view.writer_alive { "alive" } else { "DEAD" }
    ));
    out.push_str(&format!(
        "embedding: {} (db meta: {})\n",
        view.provider_model,
        view.stats.db_embedding_model.as_deref().unwrap_or("none")
    ));
    out.push_str(&format!(
        "db: {} (mode {})\n",
        view.stats.db_path,
        mode_str.as_deref().unwrap_or("unknown")
    ));
    out.push_str(&format!("wal: {} bytes\n", view.stats.wal_bytes));
    out.push_str(&format!(
        "memories: {} live, {} archived (namespace {})\n",
        view.stats.live, view.stats.archived, view.stats.namespace
    ));
    out.push_str(&format!("vectors: {}\n", view.stats.vectors));
    if let Some(c) = view.recall_channels {
        out.push_str(&format!(
            "recalls={} fts_hits={} vector_hits={} graph_hits={}\n",
            c.recalls, c.fts_hits, c.vector_hits, c.graph_hits
        ));
    }
    out.trim_end().to_string()
}

/// Render one streamed subscribe item (a change event or a lagged notice).
/// JSON: a flat object. Human: a single line.
pub fn render_change(item: &rb_proto::SubscribeItem, json: bool) -> String {
    use rb_proto::SubscribeItem;
    match item {
        SubscribeItem::Change(evt) => {
            let kind = match evt.kind {
                rb_types::ChangeKind::Created => "Created",
                rb_types::ChangeKind::Updated => "Updated",
                rb_types::ChangeKind::Archived => "Archived",
            };
            if json {
                let mut value = serde_json::json!({
                    "kind": kind,
                    "namespace": evt.namespace.as_db_string(),
                    "id": evt.id.to_string(),
                });
                // W2.7: surface the oplog cursor so a consumer can resume with
                // `subscribe --since <seq>`. Omitted (not null) when the
                // daemon predates seq stamping.
                if let (Some(seq), Some(obj)) = (evt.seq, value.as_object_mut()) {
                    obj.insert("seq".to_string(), serde_json::json!(seq));
                }
                serde_json::to_string(&value).unwrap_or_else(|e| {
                    tracing::warn!(error = %e, "failed to render change JSON");
                    "{}".to_string()
                })
            } else {
                let seq = evt.seq.map(|s| format!(" (seq {s})")).unwrap_or_default();
                format!(
                    "{} {} {}{}",
                    kind.to_lowercase(),
                    evt.namespace.as_db_string(),
                    evt.id,
                    seq
                )
            }
        }
        SubscribeItem::Lagged(dropped) => {
            if json {
                format!("{{\"lagged\":true,\"dropped\":{dropped}}}")
            } else {
                format!("lagged: {dropped} change event(s) dropped (subscriber fell behind)")
            }
        }
    }
}

/// Render a forget dry-run plan (retention PRD RET-2). JSON: the raw
/// `ForgetPlan`. Human: one reason-annotated line per candidate — age,
/// effective/author importance, last-recalled, archived state, matched rule —
/// so the user can recognize what a pass would touch before allowing it.
pub fn render_forget_plan(plan: &rb_types::ForgetPlan, json: bool) -> String {
    if json {
        // W2.4 consistency (PR #60 review): the JSON branch redacts stored
        // summaries exactly like the human branch and the sibling renderers —
        // a dry-run listing must never be the channel that leaks a secret.
        let redacted = rb_types::ForgetPlan {
            mode: plan.mode,
            candidates: plan
                .candidates
                .iter()
                .map(|c| rb_types::ForgetCandidate {
                    summary: redact(&c.summary),
                    ..c.clone()
                })
                .collect(),
            total_eligible: plan.total_eligible,
        };
        return serde_json::to_string_pretty(&redacted).unwrap_or_else(|e| {
            tracing::warn!(error = %e, "failed to render forget plan JSON");
            "{}".to_string()
        });
    }
    let action = match plan.mode {
        rb_types::ForgetMode::Apply => "archive",
        rb_types::ForgetMode::Hard => "PURGE",
    };
    if plan.candidates.is_empty() {
        return format!("forget dry-run: nothing eligible to {action}.");
    }
    let mut out = format!(
        "forget dry-run: one pass would {action} {} of {} eligible\n",
        plan.candidates.len(),
        plan.total_eligible
    );
    for c in &plan.candidates {
        let last = match c.last_accessed_at {
            Some(ts) => format!("last-recalled {}", format_ts(ts)),
            None => "never recalled".to_string(),
        };
        let rule = match c.rule {
            rb_types::ForgetRule::ArchiveAge => "archive_after_days",
            rb_types::ForgetRule::MaxAge => "max_age_days",
        };
        let state = if c.archived {
            " [already archived]"
        } else {
            ""
        };
        out.push_str(&format!(
            "  {} imp {}/{} age {}d {} [{rule}]{state} {}\n",
            c.id,
            c.importance,
            c.base_importance,
            c.age_days,
            last,
            redact(&c.summary)
        ));
    }
    out.trim_end().to_string()
}

/// Render a forget execution outcome. JSON: the raw `ForgetOutcome`.
pub fn render_forget_outcome(outcome: &rb_types::ForgetOutcome, json: bool) -> String {
    if json {
        return serde_json::to_string_pretty(outcome).unwrap_or_else(|e| {
            tracing::warn!(error = %e, "failed to render forget outcome JSON");
            "{}".to_string()
        });
    }
    let mut out = format!(
        "forget ({}): archived={} purged={} eligible={} remaining={}",
        outcome.mode.as_str(),
        outcome.archived,
        outcome.purged,
        outcome.total_eligible,
        outcome.remaining
    );
    if let Some(failure) = &outcome.failure {
        // A partial pass is loud: the counts above DID commit durably, and
        // the pass is re-runnable once the cause is fixed.
        out.push_str(&format!(
            "\nFAILED partway (completed work kept): {failure}"
        ));
    }
    out
}

/// Unix seconds to a short UTC date for the human reason lines.
fn format_ts(ts: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp(ts, 0)
        .map(|dt| dt.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| format!("@{ts}"))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use rb_types::{MemoryNote, MemoryType, Namespace, SearchResult};

    fn note(content: &str, importance: u8) -> MemoryNote {
        MemoryNote::new(
            Namespace::Project("p".into()),
            content.to_string(),
            MemoryType::Insight,
            importance,
        )
    }

    #[test]
    fn human_recall_lists_score_and_summary() {
        let mut n = note("one db one transaction", 8);
        n.summary = "one db one transaction".to_string();
        let results = vec![SearchResult {
            memory: n.clone(),
            score: 0.91,
            channels: rb_types::ChannelHits::default(),
        }];
        let out = render_recall(&results, false);
        assert!(out.contains("0.91"), "score shown: {out}");
        assert!(
            out.contains("one db one transaction"),
            "summary shown: {out}"
        );
        assert!(out.contains(&n.id.to_string()), "id shown: {out}");
    }

    #[test]
    fn human_recall_marks_contested_results() {
        let mut n = note("conflicting claim", 5);
        n.contested = true;
        let results = vec![SearchResult {
            memory: n,
            score: 0.8,
            channels: rb_types::ChannelHits::default(),
        }];
        let out = render_recall(&results, false);
        assert!(out.contains("[contested]"), "contested marker shown: {out}");
    }

    #[test]
    fn json_recall_includes_contested_field() {
        let mut n = note("conflicting claim", 5);
        n.contested = true;
        let results = vec![SearchResult {
            memory: n,
            score: 0.8,
            channels: rb_types::ChannelHits::default(),
        }];
        let out = render_recall(&results, true);
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(parsed[0]["memory"]["contested"].as_bool().unwrap());
    }

    #[test]
    fn human_list_marks_contested_notes() {
        let mut n = note("conflicting note", 5);
        n.contested = true;
        let out = render_notes(std::slice::from_ref(&n), false);
        assert!(out.contains("[contested]"), "contested marker shown: {out}");
    }

    #[test]
    fn json_recall_is_parseable_array() {
        let n = note("body", 5);
        let results = vec![SearchResult {
            memory: n,
            score: 0.5,
            channels: rb_types::ChannelHits::default(),
        }];
        let out = render_recall(&results, true);
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(parsed.is_array());
        assert_eq!(parsed[0]["score"].as_f64().unwrap(), 0.5);
    }

    #[test]
    fn human_recall_empty_has_guidance() {
        // W1.3: the CLI empty state mirrors the rb-mcp hint wording.
        let out = render_recall(&[], false);
        assert_eq!(out, "No stored memories match.");
        // JSON mode keeps the raw wire shape: an empty array, no hint object.
        let json_out = render_recall(&[], true);
        let parsed: serde_json::Value = serde_json::from_str(&json_out).unwrap();
        assert!(parsed.as_array().map(Vec::is_empty).unwrap_or(false));
    }

    #[test]
    fn human_list_shows_each_note() {
        let notes = vec![note("alpha", 7), note("beta", 3)];
        let out = render_notes(&notes, false);
        assert!(out.contains("alpha"));
        assert!(out.contains("beta"));
    }

    #[test]
    fn json_list_is_parseable_array() {
        let notes = vec![note("alpha", 7)];
        let out = render_notes(&notes, true);
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed[0]["content"].as_str().unwrap(), "alpha");
    }

    #[test]
    fn batch_remembered_renders_count_in_both_modes() {
        assert_eq!(render_batch_remembered(3, true), "{\"count\":3}");
        assert_eq!(render_batch_remembered(3, false), "Remembered 3 memories");
        // A zero count still renders a valid, truthful object (empty-stdin no-op).
        assert_eq!(render_batch_remembered(0, true), "{\"count\":0}");
        assert_eq!(render_batch_remembered(0, false), "Remembered 0 memories");
        // JSON mode is a parseable object carrying the inserted count.
        let parsed: serde_json::Value =
            serde_json::from_str(&render_batch_remembered(500, true)).unwrap();
        assert_eq!(parsed["count"].as_u64().unwrap(), 500);
    }

    #[test]
    fn import_plan_renders_items_in_both_modes() {
        let items = vec![ImportItem {
            summary: "Use SQLite".to_string(),
            content: "Use SQLite for local-first storage".to_string(),
            memory_type: MemoryType::ArchitectureDecision,
            importance: 8,
            source: "docs/adr.md".to_string(),
            tags: Vec::new(),
            anchors: Vec::new(),
        }];

        let human = render_import_plan(&items, false);
        assert!(human.contains("Use SQLite"), "summary shown: {human}");

        let json = render_import_plan(&items, true);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["planned"].as_u64().unwrap(), 1);
        assert_eq!(
            parsed["items"][0]["type"].as_str().unwrap(),
            "architecture_decision"
        );
    }

    #[test]
    fn import_plan_redacts_preview_metadata() {
        let token = format!("sk-proj-{}", "A".repeat(40));
        let items = vec![ImportItem {
            summary: format!("token {token}"),
            content: "not rendered".to_string(),
            memory_type: MemoryType::Insight,
            importance: 5,
            source: format!("docs/{token}.md"),
            tags: Vec::new(),
            anchors: Vec::new(),
        }];

        let human = render_import_plan(&items, false);
        assert!(
            !human.contains(&token),
            "human preview leaked token: {human}"
        );
        assert!(human.contains("[REDACTED:model-api-key]"), "{human}");

        let json = render_import_plan(&items, true);
        assert!(!json.contains(&token), "json preview leaked token: {json}");
        assert!(json.contains("[REDACTED:model-api-key]"), "{json}");
    }

    #[test]
    fn import_result_includes_undo_hint_only_for_written_batches() {
        let written = render_import_result(
            "import-abc",
            ImportCounts {
                new: 2,
                skipped_duplicate: 1,
                failed: 0,
            },
            false,
            false,
        );
        assert!(written.contains("init --undo import-abc"));

        let dry_run = render_import_result(
            "import-abc",
            ImportCounts {
                new: 2,
                skipped_duplicate: 0,
                failed: 0,
            },
            true,
            false,
        );
        assert!(!dry_run.contains("--undo"));
    }

    #[test]
    fn import_undo_and_batches_render_parseable_json() {
        let undo = render_import_undo("import-abc", UndoCounts { deleted: 2 }, true);
        let parsed: serde_json::Value = serde_json::from_str(&undo).unwrap();
        assert_eq!(parsed["deleted"].as_u64().unwrap(), 2);

        let batches = render_import_batches(
            &[BatchInfo {
                id: "import-abc".to_string(),
                count: 2,
            }],
            true,
        );
        let parsed: serde_json::Value = serde_json::from_str(&batches).unwrap();
        assert_eq!(
            parsed["batches"][0]["batch"].as_str().unwrap(),
            "import-abc"
        );
    }

    #[test]
    fn render_get_some_and_none() {
        let n = note("body", 5);
        let some = render_get(&Some(n.clone()), false);
        assert!(some.contains("body"));
        let none = render_get(&None, false);
        assert!(none.to_lowercase().contains("not found"));
        let json_none = render_get(&None, true);
        assert_eq!(json_none.trim(), "null");
    }

    #[test]
    fn render_surfaces_anchors_on_get_and_notes() {
        // ANC-4: `graph <id>`/`list`/`get` list a memory's anchors.
        let mut n = note("anchored body", 5);
        n.anchors = vec![
            rb_types::MemoryAnchor::parse_file_spec("src/a.rs:3-9").unwrap(),
            rb_types::MemoryAnchor::new(rb_types::AnchorKind::Symbol, "Foo::bar").unwrap(),
        ];
        let got = render_get(&Some(n.clone()), false);
        assert!(
            got.contains("anchors: file:src/a.rs:3-9, symbol:Foo::bar"),
            "{got}"
        );

        let listed = render_notes(&[n], false);
        assert!(
            listed.contains("[anchors: file:src/a.rs:3-9, symbol:Foo::bar]"),
            "{listed}"
        );

        // Anchor-less notes render exactly as before (no empty suffix).
        let plain = render_notes(&[note("plain", 5)], false);
        assert!(!plain.contains("anchors"), "{plain}");
    }

    #[test]
    fn human_get_marks_contested_note() {
        // The get read path must also surface the contested marker (Feature C).
        let mut n = note("body", 5);
        n.contested = true;
        let out = render_get(&Some(n), false);
        assert!(out.contains("[contested]"), "get marks contested: {out}");
    }

    #[test]
    fn delete_json_is_parseable() {
        let out = render_deleted(true);
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["deleted"].as_bool(), Some(true));
        assert_eq!(render_deleted(false), "Deleted");
    }

    #[test]
    fn feedback_renders_confidence_in_both_modes() {
        let json = render_feedback(0.4, true);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!((parsed["confidence"].as_f64().unwrap() - 0.4).abs() < 1e-6);
        assert!(render_feedback(0.4, false).contains("0.40"));
    }

    fn sample_stats() -> rb_types::MemoryStats {
        rb_types::MemoryStats {
            namespace: "project:rusty-brain".to_string(),
            window_days: 30,
            db_path: "/tmp/rb.db".to_string(),
            live: 42,
            archived: 3,
            vectors: 42,
            wal_bytes: 12_288,
            db_embedding_model: Some("deterministic".to_string()),
            feedback: rb_types::FeedbackTotals {
                helpful: 5,
                wrong: 1,
                stale: 2,
            },
            total_accesses: 120,
            accessed_in_window: 7,
            top_recalled: vec![rb_types::TopRecalled {
                id: rb_types::MemoryId::new(),
                access_count: 9,
            }],
            never_recalled_live: 12,
            contested: 1,
            created_per_day: vec![rb_types::GrowthBucket {
                day: "2026-07-10".to_string(),
                count: 2,
            }],
            reembed_pending: 3,
            retention_eligible: Some(4),
            last_forget_at: Some(1_700_000_000),
        }
    }

    #[test]
    fn human_stats_reports_retention_gauges_when_present() {
        // RET-4: eligible-forgetting count and last-forget run are visible in
        // the human rendering; absent gauges (no policy / never swept) leave
        // no confusing zero lines.
        let stats = sample_stats();
        let out = render_stats(&stats, "deterministic", true, false);
        assert!(out.contains("retention: 4 eligible"), "{out}");
        assert!(out.contains("last forget"), "{out}");

        let mut bare = sample_stats();
        bare.retention_eligible = None;
        bare.last_forget_at = None;
        let out = render_stats(&bare, "deterministic", true, false);
        assert!(!out.contains("retention:"), "{out}");
    }

    #[test]
    fn forget_plan_renders_reasons_and_redacts_summaries() {
        let plan = rb_types::ForgetPlan {
            mode: rb_types::ForgetMode::Apply,
            candidates: vec![rb_types::ForgetCandidate {
                id: rb_types::MemoryId::new(),
                summary: "old note with key sk-abc123def456ghi789jkl012mno".to_string(),
                created_at: 1_700_000_000,
                age_days: 60,
                importance: 2,
                base_importance: 3,
                last_accessed_at: None,
                archived: false,
                rule: rb_types::ForgetRule::MaxAge,
            }],
            total_eligible: 5,
        };
        let out = render_forget_plan(&plan, false);
        assert!(out.contains("archive 1 of 5 eligible"), "{out}");
        assert!(out.contains("imp 2/3"), "{out}");
        assert!(out.contains("age 60d"), "{out}");
        assert!(out.contains("never recalled"), "{out}");
        assert!(out.contains("max_age_days"), "{out}");
        // Summaries pass through the shared redactor: a stored secret must
        // not leak via a dry-run listing.
        assert!(!out.contains("sk-abc123def456ghi789jkl012mno"), "{out}");

        let empty = rb_types::ForgetPlan::default();
        assert!(render_forget_plan(&empty, false).contains("nothing eligible"));

        let json_out = render_forget_plan(&plan, true);
        let parsed: rb_types::ForgetPlan = serde_json::from_str(&json_out).unwrap();
        // Structure round-trips; the summary comes back REDACTED (W2.4 —
        // pinned separately in the JSON-redaction test).
        assert_eq!(parsed.total_eligible, plan.total_eligible);
        assert_eq!(parsed.candidates.len(), 1);
        assert_eq!(parsed.candidates[0].id, plan.candidates[0].id);
        assert_eq!(
            parsed.candidates[0].summary,
            redact(&plan.candidates[0].summary)
        );
    }

    #[test]
    fn forget_plan_json_redacts_summaries_like_sibling_renderers() {
        // PR #60 review (Copilot): the JSON branch must not leak a stored
        // secret that the human branch redacts — W2.4 consistency across
        // every renderer.
        let plan = rb_types::ForgetPlan {
            mode: rb_types::ForgetMode::Apply,
            candidates: vec![rb_types::ForgetCandidate {
                id: rb_types::MemoryId::new(),
                summary: "note with key sk-abc123def456ghi789jkl012mno".to_string(),
                created_at: 1_700_000_000,
                age_days: 60,
                importance: 2,
                base_importance: 3,
                last_accessed_at: None,
                archived: false,
                rule: rb_types::ForgetRule::MaxAge,
            }],
            total_eligible: 1,
        };
        let json_out = render_forget_plan(&plan, true);
        assert!(
            !json_out.contains("sk-abc123def456ghi789jkl012mno"),
            "JSON dry-run must redact summaries: {json_out}"
        );
        assert!(json_out.contains("REDACTED"), "{json_out}");
    }

    #[test]
    fn forget_outcome_failure_is_rendered_in_both_modes() {
        // PR #60 review (HIGH): a partial pass must be visible — the counts
        // that committed AND the failure that stopped it.
        let outcome = rb_types::ForgetOutcome {
            mode: rb_types::ForgetMode::Hard,
            archived: 0,
            purged: 2,
            total_eligible: 5,
            remaining: 3,
            failure: Some("purge of abc failed: injected".to_string()),
        };
        let human = render_forget_outcome(&outcome, false);
        assert!(human.contains("purged=2"), "{human}");
        assert!(
            human.contains("FAILED") && human.contains("injected"),
            "partial failure must be loud: {human}"
        );
        let parsed: rb_types::ForgetOutcome =
            serde_json::from_str(&render_forget_outcome(&outcome, true)).unwrap();
        assert_eq!(
            parsed.failure.as_deref(),
            Some("purge of abc failed: injected")
        );
    }

    #[test]
    fn forget_outcome_renders_counts_in_both_modes() {
        let outcome = rb_types::ForgetOutcome {
            mode: rb_types::ForgetMode::Hard,
            archived: 0,
            purged: 2,
            total_eligible: 3,
            remaining: 1,
            failure: None,
        };
        let out = render_forget_outcome(&outcome, false);
        assert!(out.contains("hard"), "{out}");
        assert!(out.contains("purged=2"), "{out}");
        assert!(out.contains("remaining=1"), "{out}");
        let parsed: rb_types::ForgetOutcome =
            serde_json::from_str(&render_forget_outcome(&outcome, true)).unwrap();
        assert_eq!(parsed, outcome);
    }

    #[test]
    fn human_stats_shows_value_signals_and_low_n_caveat() {
        let stats = sample_stats();
        let out = render_stats(&stats, "deterministic", true, false);
        assert!(out.contains("project:rusty-brain"), "namespace: {out}");
        assert!(out.contains("30"), "window: {out}");
        assert!(out.contains("42"), "live count: {out}");
        assert!(out.contains("5 helpful"), "feedback counts: {out}");
        assert!(out.contains("1 wrong"), "{out}");
        assert!(out.contains("2 stale"), "{out}");
        assert!(out.contains("net +2"), "net trend: {out}");
        assert!(out.contains("12"), "never recalled: {out}");
        assert!(out.contains(&stats.top_recalled[0].id.to_string()), "{out}");
        assert!(out.contains("2026-07-10"), "growth bucket: {out}");
        // 8 total events < 10: raw counts get a low-N caveat, not a confident
        // ratio (PRD risk mitigation).
        assert!(
            out.contains("low") || out.contains("few"),
            "low-N caveat: {out}"
        );
        // Counts and ids only — never memory content.
        assert!(!out.contains("content"), "{out}");
    }

    #[test]
    fn human_stats_without_feedback_says_so() {
        let mut stats = sample_stats();
        stats.feedback = rb_types::FeedbackTotals::default();
        let out = render_stats(&stats, "deterministic", true, false);
        assert!(
            out.to_lowercase().contains("no feedback"),
            "empty-feedback state must be explicit: {out}"
        );
    }

    #[test]
    fn json_stats_is_parseable_and_carries_daemon_fields() {
        let stats = sample_stats();
        let out = render_stats(&stats, "deterministic", true, true);
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["stats"]["live"].as_u64(), Some(42));
        assert_eq!(parsed["stats"]["feedback"]["helpful"].as_u64(), Some(5));
        assert_eq!(parsed["stats"]["feedback"]["net"].as_i64(), Some(2));
        assert_eq!(parsed["provider_model"].as_str(), Some("deterministic"));
        assert_eq!(parsed["writer_alive"].as_bool(), Some(true));
    }

    #[test]
    fn status_renders_health_payload_in_both_modes() {
        let stats = sample_stats();
        let view = StatusView {
            contract_version: 2,
            recall_channels: Some(rb_proto::RecallChannelTotals {
                recalls: 4,
                fts_hits: 9,
                vector_hits: 11,
                graph_hits: 2,
            }),
            stats: &stats,
            provider_model: "deterministic",
            writer_alive: true,
            db_file_mode: Some(0o600),
        };
        let human = render_status(&view, false);
        assert!(human.contains("ok (contract v2)"), "{human}");
        assert!(human.contains("writer: alive"), "{human}");
        assert!(human.contains("deterministic"), "{human}");
        assert!(human.contains("/tmp/rb.db"), "{human}");
        assert!(human.contains("0600"), "db file mode: {human}");
        assert!(human.contains("42 live"), "{human}");
        assert!(human.contains("3 archived"), "{human}");
        assert!(human.contains("project:rusty-brain"), "{human}");
        assert!(human.contains("recalls=4"), "{human}");

        let json = render_status(&view, true);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["ok"].as_bool(), Some(true));
        assert_eq!(parsed["contract_version"].as_u64(), Some(2));
        assert_eq!(parsed["writer_alive"].as_bool(), Some(true));
        assert_eq!(parsed["provider_model"].as_str(), Some("deterministic"));
        assert_eq!(parsed["db"]["path"].as_str(), Some("/tmp/rb.db"));
        assert_eq!(parsed["db"]["file_mode"].as_str(), Some("0600"));
        assert_eq!(parsed["db"]["wal_bytes"].as_u64(), Some(12_288));
        assert_eq!(parsed["memories"]["live"].as_u64(), Some(42));
        assert_eq!(parsed["memories"]["archived"].as_u64(), Some(3));
        assert_eq!(parsed["vectors"].as_u64(), Some(42));
        assert_eq!(parsed["namespace"].as_str(), Some("project:rusty-brain"));
        assert_eq!(parsed["recall_channels"]["recalls"].as_u64(), Some(4));
    }

    #[test]
    fn status_marks_a_dead_writer() {
        let stats = sample_stats();
        let view = StatusView {
            contract_version: 2,
            recall_channels: None,
            stats: &stats,
            provider_model: "deterministic",
            writer_alive: false,
            db_file_mode: None,
        };
        let human = render_status(&view, false);
        assert!(
            human.contains("writer: DEAD"),
            "a dead writer must be loud: {human}"
        );
    }

    #[test]
    fn human_change_shows_kind_namespace_and_id() {
        use rb_proto::SubscribeItem;
        use rb_types::{ChangeKind, MemoryChanged, MemoryId, Namespace};
        let id = MemoryId::new();
        let item = SubscribeItem::Change(MemoryChanged {
            id: id.clone(),
            namespace: Namespace::Project("p".into()),
            kind: ChangeKind::Created,
            seq: Some(4),
        });
        let out = render_change(&item, false);
        assert!(out.contains("created"), "kind shown: {out}");
        assert!(out.contains("project:p"), "namespace shown: {out}");
        assert!(out.contains(&id.to_string()), "id shown: {out}");
    }

    #[test]
    fn human_lagged_shows_dropped_count() {
        use rb_proto::SubscribeItem;
        let out = render_change(&SubscribeItem::Lagged(9), false);
        assert!(out.to_lowercase().contains("lagged"), "lagged shown: {out}");
        assert!(out.contains('9'), "dropped count shown: {out}");
    }

    #[test]
    fn json_change_is_parseable_object() {
        use rb_proto::SubscribeItem;
        use rb_types::{ChangeKind, MemoryChanged, MemoryId, Namespace};
        let id = MemoryId::new();
        let item = SubscribeItem::Change(MemoryChanged {
            id: id.clone(),
            namespace: Namespace::Global,
            kind: ChangeKind::Archived,
            seq: None,
        });
        let out = render_change(&item, true);
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["kind"], "Archived");
        assert_eq!(parsed["namespace"], "global");
        assert_eq!(parsed["id"], id.to_string());
    }

    #[test]
    fn json_lagged_is_parseable_object() {
        use rb_proto::SubscribeItem;
        let out = render_change(&SubscribeItem::Lagged(4), true);
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["lagged"], true);
        assert_eq!(parsed["dropped"], 4);
    }
}
