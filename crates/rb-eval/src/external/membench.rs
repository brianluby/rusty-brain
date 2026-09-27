//! MemBench adapter (Vikunja #60), MemPalace raw protocol over the
//! `MemData/FirstAgent` files: one document per turn,
//! `[time] [User] u [Assistant] a`, top-5. A target (first element of each
//! `target_step_id` pair) is hit when it equals either a retrieved turn's
//! `sid` or its global position, as in MemPalace; the sid-only match is
//! reported separately because two id spaces add chance hits.

use super::core::{retrieve, Hit, Profile};
use rb_embed::EmbeddingProvider;
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;

pub const CATEGORY_FILES: [(&str, &str); 11] = [
    ("simple", "simple.json"),
    ("highlevel", "highlevel.json"),
    ("knowledge_update", "knowledge_update.json"),
    ("comparative", "comparative.json"),
    ("conditional", "conditional.json"),
    ("noisy", "noisy.json"),
    ("aggregative", "aggregative.json"),
    ("highlevel_rec", "highlevel_rec.json"),
    ("lowlevel_rec", "lowlevel_rec.json"),
    ("RecMultiSession", "RecMultiSession.json"),
    ("post_processing", "post_processing.json"),
];

#[derive(Debug, Clone)]
pub struct Turn {
    pub sid: i64,
    pub global: i64,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct Item {
    pub category: &'static str,
    pub topic: String,
    pub index_in_topic: usize,
    pub tid: Value,
    pub question: String,
    pub targets: BTreeSet<i64>,
    pub turns: Vec<Turn>,
}

fn text_field(turn: &serde_json::Map<String, Value>, a: &str, b: &str) -> String {
    // Python `turn.get(a) or turn.get(b, "")`: a falsy first value falls through.
    let first = turn.get(a).and_then(Value::as_str).unwrap_or("");
    if first.is_empty() {
        turn.get(b)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    } else {
        first.to_string()
    }
}

/// Python `int(x)` for a JSON number: integers exactly, floats truncated.
fn json_int(v: &Value) -> Option<i64> {
    v.as_i64()
        .or_else(|| v.as_u64().and_then(|u| i64::try_from(u).ok()))
        .or_else(|| v.as_f64().map(|f| f as i64))
}

fn turns(message_list: &[Value]) -> Vec<Turn> {
    let sessions: Vec<&[Value]> = match message_list.first() {
        Some(Value::Object(_)) => vec![message_list],
        _ => message_list
            .iter()
            .filter_map(|s| s.as_array().map(Vec::as_slice))
            .collect(),
    };
    let mut out = Vec::new();
    let mut global = 0i64;
    for session in sessions {
        for turn in session.iter().filter_map(Value::as_object) {
            let user = text_field(turn, "user", "user_message");
            let asst = text_field(turn, "assistant", "assistant_message");
            let time = turn.get("time").and_then(Value::as_str).unwrap_or("");
            let mut text = format!("[User] {user} [Assistant] {asst}");
            if !time.is_empty() {
                text = format!("[{time}] {text}");
            }
            // Python `turn.get("sid", turn.get("mid"))`: mid only when sid is absent.
            let raw = if turn.contains_key("sid") {
                turn.get("sid")
            } else {
                turn.get("mid")
            };
            let sid = raw.and_then(json_int).unwrap_or(global);
            out.push(Turn { sid, global, text });
            global += 1;
        }
    }
    out
}

/// Every item across all categories for the given topic plus the
/// role/event-keyed files, as MemPalace's `load_membench(topic=..., limit=0)`.
pub fn load(dir: &Path, topic: &str) -> anyhow::Result<Vec<Item>> {
    let mut items = Vec::new();
    for (category, file) in CATEGORY_FILES {
        let path = dir.join(file);
        if !path.exists() {
            continue;
        }
        let raw: serde_json::Map<String, Value> = serde_json::from_slice(&std::fs::read(&path)?)?;
        for (t, topic_items) in &raw {
            if t != topic && t != "roles" && t != "events" {
                continue;
            }
            for (index_in_topic, item) in topic_items.as_array().into_iter().flatten().enumerate() {
                let message_list = item
                    .get("message_list")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let qa = item.get("QA").and_then(Value::as_object);
                let (Some(qa), false) = (qa, message_list.is_empty()) else {
                    continue;
                };
                if qa.is_empty() {
                    continue;
                }
                let targets = qa
                    .get("target_step_id")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|s| json_int(s.as_array()?.first()?))
                    .collect();
                items.push(Item {
                    category,
                    topic: t.clone(),
                    index_in_topic,
                    tid: item.get("tid").cloned().unwrap_or(Value::Null),
                    question: qa
                        .get("question")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    targets,
                    turns: turns(&message_list),
                });
            }
        }
    }
    Ok(items)
}

#[derive(Debug, Clone, Serialize)]
pub struct ItemRecord {
    pub category: &'static str,
    pub topic: String,
    pub index_in_topic: usize,
    pub tid: Value,
    pub targets: BTreeSet<i64>,
    pub corpus_size: usize,
    pub hit: bool,
    pub hit_sid_only: bool,
    pub ranked: Vec<Hit>,
    pub abstained: Option<String>,
    pub ingest_ms: u128,
    pub query_us: u128,
}

pub async fn run_item<P: EmbeddingProvider>(
    provider: &Arc<P>,
    item: &Item,
    profile: Profile,
    top_k: usize,
) -> rb_types::Result<Option<ItemRecord>> {
    if item.turns.is_empty() {
        return Ok(None);
    }
    let docs: Vec<(String, String)> = item
        .turns
        .iter()
        .map(|t| (format!("g{}", t.global), t.text.clone()))
        .collect();
    let by_id: HashMap<&str, &Turn> = docs
        .iter()
        .map(|(id, _)| id.as_str())
        .zip(&item.turns)
        .collect();
    let r = retrieve(
        provider,
        &docs,
        &item.question,
        profile,
        top_k.min(docs.len()),
    )
    .await?;
    let retrieved: Vec<&Turn> = r
        .ranked
        .iter()
        .filter_map(|h| by_id.get(h.session_id.as_str()).copied())
        .collect();
    let hit_sid_only = retrieved.iter().any(|t| item.targets.contains(&t.sid));
    let hit = hit_sid_only || retrieved.iter().any(|t| item.targets.contains(&t.global));
    Ok(Some(ItemRecord {
        category: item.category,
        topic: item.topic.clone(),
        index_in_topic: item.index_in_topic,
        tid: item.tid.clone(),
        targets: item.targets.clone(),
        corpus_size: docs.len(),
        hit,
        hit_sid_only,
        ranked: r.ranked,
        abstained: r.abstained,
        ingest_ms: r.ingest_ms,
        query_us: r.query_us,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn turns_match_mempalace_text_and_ids() {
        let ml = vec![
            json!([{"sid": 7, "user_message": "hi", "assistant_message": "yo", "time": "t0"}]),
            json!([{"user": "", "user_message": "fallback", "assistant": "a"}]),
        ];
        let t = turns(&ml);
        assert_eq!(t[0].text, "[t0] [User] hi [Assistant] yo");
        assert_eq!((t[0].sid, t[0].global), (7, 0));
        assert_eq!(t[1].text, "[User] fallback [Assistant] a");
        assert_eq!(
            (t[1].sid, t[1].global),
            (1, 1),
            "missing sid falls back to position"
        );
        let flat = vec![json!({"mid": 3, "user": "u", "assistant": "a"})];
        assert_eq!(turns(&flat)[0].sid, 3);
        assert_eq!(
            json_int(&json!(9_007_199_254_740_993_i64)),
            Some(9_007_199_254_740_993)
        );
        assert_eq!(json_int(&json!(4.9)), Some(4));
    }
}
