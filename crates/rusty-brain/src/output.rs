//! Pure rendering of results to human text or JSON (stdout).

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
        return "No memories matched.".to_string();
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
                let value = serde_json::json!({
                    "kind": kind,
                    "namespace": evt.namespace.as_db_string(),
                    "id": evt.id.to_string(),
                });
                serde_json::to_string(&value).unwrap_or_else(|e| {
                    tracing::warn!(error = %e, "failed to render change JSON");
                    "{}".to_string()
                })
            } else {
                format!(
                    "{} {} {}",
                    kind.to_lowercase(),
                    evt.namespace.as_db_string(),
                    evt.id
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
        let out = render_recall(&[], false);
        assert!(
            out.to_lowercase().contains("no memories"),
            "empty guidance: {out}"
        );
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
    fn human_change_shows_kind_namespace_and_id() {
        use rb_proto::SubscribeItem;
        use rb_types::{ChangeKind, MemoryChanged, MemoryId, Namespace};
        let id = MemoryId::new();
        let item = SubscribeItem::Change(MemoryChanged {
            id: id.clone(),
            namespace: Namespace::Project("p".into()),
            kind: ChangeKind::Created,
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
