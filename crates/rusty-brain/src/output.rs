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
            "{} (imp {}, {}){} {}\n",
            n.id,
            n.importance,
            n.memory_type.as_str(),
            contested,
            summary
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
            format!(
                "{}{}\nnamespace: {}\ntype: {}\nimportance: {}\n\n{}",
                n.id,
                contested,
                n.namespace.as_db_string(),
                n.memory_type.as_str(),
                n.importance,
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
