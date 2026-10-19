//! LongMemEval turn-granularity oracle mode, replicating the `mind` project's
//! harness (`bench/longmemeval.py` at commit `91cbbf0`, SHA-256 `6f105c6a…`)
//! so rusty-brain can be scored on exactly its questions and metrics.
//!
//! Definitions copied from that harness:
//! - one record per turn (user AND assistant), identical texts collapsed;
//! - evidence = turns marked `has_answer`, or every turn of an answer session
//!   that has no marked turn;
//! - the query is the question text only, against a fresh per-question store;
//! - metrics: evidence at rank 1, evidence in the top k, and the normalized
//!   answer string inside any top-k record text.
//!
//! Unlike the session adapter, `has_answer` is deserialized here, but only to
//! score: it never enters stored text. [`RecordFormat::Mind`] reproduces
//! mind's record text, which embeds the date, question id and session id
//! (oracle session ids start with `answer_`, a label leak shared by every
//! engine scored that way); [`RecordFormat::Clean`] stores `role: content`.

use super::core::{retrieve, Hit, Profile};
use rb_embed::EmbeddingProvider;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Candidate depth requested from rusty-brain, as in the session adapter.
pub const TOP_N: usize = 50;
/// mind's `top_k` for the reported subset.
pub const TOP_K: usize = 5;

#[derive(Debug, Clone, Deserialize)]
pub struct OracleTurn {
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub has_answer: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OracleEntry {
    pub question_id: String,
    pub question_type: String,
    pub question: String,
    #[serde(default)]
    pub answer: serde_json::Value,
    #[serde(default)]
    pub haystack_session_ids: Vec<String>,
    #[serde(default)]
    pub haystack_dates: Vec<String>,
    pub haystack_sessions: Vec<Vec<OracleTurn>>,
    #[serde(default)]
    pub answer_session_ids: Vec<String>,
}

impl OracleEntry {
    /// mind's `should_skip_abstention`: ids ending in `_abs`.
    pub fn is_abstention(&self) -> bool {
        self.question_id.ends_with("_abs")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum RecordFormat {
    /// mind's exact record text: `[date] question <qid> session <sid> <role>: <content>`.
    Mind,
    /// `role: content` only; no date, question id or session id.
    Clean,
}

/// Turn documents `(id, text)` in first-seen order plus the evidence ids,
/// following mind's `remember_instance` for `granularity == "turn"`.
pub fn turn_documents(
    entry: &OracleEntry,
    format: RecordFormat,
) -> (Vec<(String, String)>, HashSet<String>) {
    let answer_sessions: HashSet<&str> = entry
        .answer_session_ids
        .iter()
        .map(String::as_str)
        .collect();
    let mut docs: Vec<(String, String)> = Vec::new();
    let mut by_text: HashMap<String, String> = HashMap::new();
    let mut evidence = HashSet::new();
    for (index, turns) in entry.haystack_sessions.iter().enumerate() {
        let session_id = entry
            .haystack_session_ids
            .get(index)
            .cloned()
            .unwrap_or_else(|| index.to_string());
        let date = entry
            .haystack_dates
            .get(index)
            .filter(|d| !d.is_empty())
            .map_or("unknown-date", String::as_str);
        let session_is_evidence = answer_sessions.contains(session_id.as_str());
        let session_has_marked_turn = turns.iter().any(|t| t.has_answer);
        for turn in turns {
            let role = turn
                .role
                .as_deref()
                .filter(|r| !r.is_empty())
                .unwrap_or("unknown-role");
            let text = match format {
                RecordFormat::Mind => format!(
                    "[{date}] question {} session {session_id} {role}: {}",
                    entry.question_id, turn.content
                ),
                RecordFormat::Clean => format!("{role}: {}", turn.content),
            };
            let id = by_text
                .entry(text.clone())
                .or_insert_with(|| {
                    let id = format!("t{}", docs.len());
                    docs.push((id.clone(), text));
                    id
                })
                .clone();
            if turn.has_answer || (session_is_evidence && !session_has_marked_turn) {
                evidence.insert(id);
            }
        }
    }
    (docs, evidence)
}

/// mind's `norm`: lowercase, collapse whitespace; lists join their items;
/// `None` is empty. Non-string scalars use Python's `str()` spelling.
pub fn norm(value: &serde_json::Value) -> String {
    let raw = match value {
        serde_json::Value::Null => return String::new(),
        serde_json::Value::Array(items) => {
            return items.iter().map(norm).collect::<Vec<_>>().join(" ");
        }
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Bool(b) => if *b { "True" } else { "False" }.to_string(),
        other => other.to_string(),
    };
    raw.to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// mind's `answer_in_text`: a non-empty normalized answer inside the
/// normalized record text.
pub fn answer_in_text(answer: &serde_json::Value, text: &str) -> bool {
    let answer = norm(answer);
    !answer.is_empty() && norm(&serde_json::Value::String(text.to_string())).contains(&answer)
}

#[derive(Debug, Clone, Serialize)]
pub struct TurnRecord {
    pub question_id: String,
    pub question_type: String,
    pub records: usize,
    pub evidence: usize,
    pub ranked: Vec<Hit>,
    pub abstained: Option<String>,
    pub evidence_at_1: bool,
    pub evidence_at_k: bool,
    pub answer_string_at_k: bool,
    pub ingest_ms: u128,
    pub query_us: u128,
}

/// Score one question; `None` when it has no evidence turn (mind skips those).
pub async fn run_question<P: EmbeddingProvider>(
    provider: &Arc<P>,
    entry: &OracleEntry,
    profile: Profile,
    format: RecordFormat,
) -> rb_types::Result<Option<TurnRecord>> {
    let (docs, evidence) = turn_documents(entry, format);
    if evidence.is_empty() {
        return Ok(None);
    }
    let text_of: HashMap<&str, &str> = docs
        .iter()
        .map(|(id, text)| (id.as_str(), text.as_str()))
        .collect();
    let r = retrieve(provider, &docs, &entry.question, profile, TOP_N).await?;
    let top: Vec<&str> = r
        .ranked
        .iter()
        .take(TOP_K)
        .map(|h| h.session_id.as_str())
        .collect();
    Ok(Some(TurnRecord {
        question_id: entry.question_id.clone(),
        question_type: entry.question_type.clone(),
        records: docs.len(),
        evidence: evidence.len(),
        evidence_at_1: top.first().is_some_and(|id| evidence.contains(*id)),
        evidence_at_k: top.iter().any(|id| evidence.contains(*id)),
        answer_string_at_k: top.iter().any(|id| {
            text_of
                .get(id)
                .is_some_and(|t| answer_in_text(&entry.answer, t))
        }),
        ranked: r.ranked,
        abstained: r.abstained,
        ingest_ms: r.ingest_ms,
        query_us: r.query_us,
    }))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use serde_json::json;

    fn entry() -> OracleEntry {
        serde_json::from_value(json!({
            "question_id": "q1",
            "question_type": "single-session-user",
            "question": "What color is the bike?",
            "answer": "Red",
            "haystack_session_ids": ["answer_a", "s2", "answer_c"],
            "haystack_dates": ["2023/05/01 (Mon) 10:00", "", "2023/05/03"],
            "haystack_sessions": [
                [
                    {"role": "user", "content": "I bought a bike", "has_answer": false},
                    {"role": "assistant", "content": "Nice! It is red", "has_answer": true}
                ],
                [
                    {"role": "user", "content": "hello"},
                    {"role": "user", "content": "hello"}
                ],
                [
                    {"role": "user", "content": "unmarked evidence turn"}
                ]
            ],
            "answer_session_ids": ["answer_a", "answer_c"]
        }))
        .unwrap()
    }

    #[test]
    fn mind_format_matches_the_harness_record_text() {
        let (docs, _) = turn_documents(&entry(), RecordFormat::Mind);
        assert_eq!(
            docs[0].1,
            "[2023/05/01 (Mon) 10:00] question q1 session answer_a user: I bought a bike"
        );
        assert_eq!(
            docs[2].1,
            "[unknown-date] question q1 session s2 user: hello"
        );
        let (clean, _) = turn_documents(&entry(), RecordFormat::Clean);
        assert_eq!(clean[1].1, "assistant: Nice! It is red");
    }

    #[test]
    fn evidence_is_marked_turns_or_whole_unmarked_answer_sessions() {
        let (docs, evidence) = turn_documents(&entry(), RecordFormat::Clean);
        let text = |id: &String| docs.iter().find(|(i, _)| i == id).unwrap().1.clone();
        let mut got: Vec<String> = evidence.iter().map(text).collect();
        got.sort();
        // Session a has a marked turn, so only that turn counts; session c has
        // none, so its every turn counts; session s2 is not an answer session.
        assert_eq!(
            got,
            vec!["assistant: Nice! It is red", "user: unmarked evidence turn"]
        );
    }

    #[test]
    fn identical_texts_collapse_into_one_record() {
        let (docs, _) = turn_documents(&entry(), RecordFormat::Clean);
        assert_eq!(docs.iter().filter(|(_, t)| t == "user: hello").count(), 1);
        assert_eq!(docs.len(), 4);
    }

    #[test]
    fn answer_matching_follows_the_harness_norm() {
        assert!(answer_in_text(&json!("Red"), "It is   RED today"));
        assert!(answer_in_text(&json!(3), "I have 3 bikes"));
        assert!(answer_in_text(&json!(["two", "Bikes"]), "two bikes"));
        assert!(!answer_in_text(&json!(""), "anything"));
        assert!(!answer_in_text(&serde_json::Value::Null, "anything"));
        assert_eq!(norm(&json!(true)), "true");
    }

    #[test]
    fn abstention_ids_end_with_abs() {
        let mut e = entry();
        assert!(!e.is_abstention());
        e.question_id = "q1_abs".into();
        assert!(e.is_abstention());
    }
}
