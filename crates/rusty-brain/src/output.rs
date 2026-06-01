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
        out.push_str(&format!(
            "[{:.2}] {} ({}) {}\n",
            r.score,
            r.memory.id,
            r.memory.memory_type.as_str(),
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
        out.push_str(&format!(
            "{} (imp {}, {}) {}\n",
            n.id,
            n.importance,
            n.memory_type.as_str(),
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
        Some(n) => format!(
            "{}\nnamespace: {}\ntype: {}\nimportance: {}\n\n{}",
            n.id,
            n.namespace.as_db_string(),
            n.memory_type.as_str(),
            n.importance,
            n.content
        ),
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
    fn json_recall_is_parseable_array() {
        let n = note("body", 5);
        let results = vec![SearchResult {
            memory: n,
            score: 0.5,
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
    fn delete_json_is_parseable() {
        let out = render_deleted(true);
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["deleted"].as_bool(), Some(true));
        assert_eq!(render_deleted(false), "Deleted");
    }
}
