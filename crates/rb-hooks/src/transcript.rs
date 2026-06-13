//! Bounded, defensive reader for an agent CLI's session transcript (W3.1).
//!
//! Both PreCompact and SessionEnd read the transcript at `transcript_path` to
//! recover decision-grade signal the per-tool scratch can't see: the user's
//! stated goals (user-turn prose) and decision-marker lines from either side's
//! prose. The transcript is the CLI's own JSONL log — UNTRUSTED, possibly
//! large, possibly partially written — so the reader is strictly BOUNDED (reads
//! at most a byte/line budget from the start) and strictly FAIL-OPEN (any
//! IO/parse error yields whatever was parsed so far, never an error or panic).
//!
//! Claude Code writes one JSON object per line. Shapes vary across versions, so
//! extraction is liberal: an entry's text is read from `message.content`
//! (a string, or the `text` blocks of a content array), falling back to a
//! top-level `content`. Non-text blocks (tool_use / tool_result) carry no
//! `text` and are naturally skipped, which filters tool I/O out of the digest.

use std::io::{BufRead, BufReader, Read};
use std::path::Path;

/// Decision-marker substrings (lowercased match): a line containing one is
/// likely recording a decision/constraint worth persisting. Shared by the
/// PreCompact and SessionEnd flows.
pub const DECISION_MARKERS: &[&str] = &[
    "decided",
    "decision",
    "chosen",
    "we will use",
    "approach is",
    "let's use",
    "going with",
    "instead of",
    "constraint",
    "must not",
    "should not",
];

/// True if `text` contains any decision marker (case-insensitive).
pub fn has_decision_marker(text: &str) -> bool {
    let lower = text.to_lowercase();
    DECISION_MARKERS.iter().any(|m| lower.contains(m))
}

/// Hard cap on bytes read from a transcript (bounds work on a huge session;
/// the first slice carries the goal + early decisions, which dominate a
/// decision-grade summary). Reading is line-buffered so a partial trailing line
/// is simply dropped.
const MAX_TRANSCRIPT_BYTES: u64 = 1024 * 1024;
/// Hard cap on transcript lines parsed (a second bound independent of byte size).
const MAX_LINES: usize = 4000;
/// Max user prompts retained (first-seen).
const MAX_PROMPTS: usize = 20;
/// Max decision lines retained (first-seen).
const MAX_DECISIONS: usize = 30;
/// Max characters retained per extracted prompt/decision.
const MAX_ENTRY_CHARS: usize = 1000;

/// What the transcript reader recovers: the user's prompts (goals) and the
/// decision-marker lines from either side's prose, deduped and bounded.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TranscriptDigest {
    pub user_prompts: Vec<String>,
    pub decisions: Vec<String>,
}

impl TranscriptDigest {
    /// True when neither a prompt nor a decision was recovered.
    pub fn is_empty(&self) -> bool {
        self.user_prompts.is_empty() && self.decisions.is_empty()
    }
}

/// Read and digest the transcript at `path`. Strictly bounded + fail-open: a
/// missing/unreadable file yields an empty digest.
pub fn read_digest(path: &Path) -> TranscriptDigest {
    let Ok(file) = std::fs::File::open(path) else {
        return TranscriptDigest::default();
    };
    let reader = BufReader::new(file.take(MAX_TRANSCRIPT_BYTES));
    digest_from_lines(reader.lines().map_while(Result::ok))
}

/// Pure core (testable without a file): fold JSONL `lines` into a digest.
pub fn digest_from_lines<I>(lines: I) -> TranscriptDigest
where
    I: IntoIterator<Item = String>,
{
    let mut digest = TranscriptDigest::default();
    for (count, line) in lines.into_iter().enumerate() {
        if count >= MAX_LINES {
            break;
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let Some((role, text)) = role_and_text(&value) else {
            continue;
        };
        if role == "user" && digest.user_prompts.len() < MAX_PROMPTS {
            let prompt = truncate(text.trim());
            if !prompt.is_empty() && !digest.user_prompts.iter().any(|p| p == &prompt) {
                digest.user_prompts.push(prompt);
            }
        }
        // Decisions can come from either side's prose.
        if digest.decisions.len() < MAX_DECISIONS {
            for raw in text.lines() {
                if digest.decisions.len() >= MAX_DECISIONS {
                    break;
                }
                let cand = raw.trim();
                if cand.is_empty() || !has_decision_marker(cand) {
                    continue;
                }
                let decision = truncate(cand);
                if !digest.decisions.iter().any(|d| d == &decision) {
                    digest.decisions.push(decision);
                }
            }
        }
    }
    digest
}

/// Extract `(role, text)` from one transcript entry, liberal across shapes.
/// Returns `None` when no role or no text content is present (e.g. a tool-only
/// turn, a `summary`/`system` entry, or an unrecognized shape).
fn role_and_text(entry: &serde_json::Value) -> Option<(String, String)> {
    // Prefer the nested `message` object; fall back to the entry itself.
    let message = entry.get("message").unwrap_or(entry);
    let role = message
        .get("role")
        .and_then(serde_json::Value::as_str)
        .or_else(|| entry.get("role").and_then(serde_json::Value::as_str))?;
    let content = message.get("content").or_else(|| entry.get("content"))?;
    let text = content_text(content);
    if text.trim().is_empty() {
        return None;
    }
    Some((role.to_string(), text))
}

/// Flatten a `content` field to text: a bare string is itself; an array keeps
/// only `type: "text"` blocks' `text`, joined with newlines (tool_use /
/// tool_result blocks have no `text` and drop out).
fn content_text(content: &serde_json::Value) -> String {
    match content {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(blocks) => blocks
            .iter()
            .filter(|b| b.get("type").and_then(serde_json::Value::as_str) == Some("text"))
            .filter_map(|b| b.get("text").and_then(serde_json::Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Head-truncate to [`MAX_ENTRY_CHARS`], UTF-8 safe.
fn truncate(value: &str) -> String {
    match value.char_indices().nth(MAX_ENTRY_CHARS) {
        Some((idx, _)) => value[..idx].to_string(),
        None => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn lines(raw: &[&str]) -> Vec<String> {
        raw.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn decision_marker_detected_case_insensitively() {
        assert!(has_decision_marker("We DECIDED to use SQLite."));
        assert!(has_decision_marker("Decision: single writer."));
        assert!(has_decision_marker("the chosen approach is X"));
        assert!(has_decision_marker("let's use porter tokenizer"));
        assert!(has_decision_marker(
            "namespace must not be an auth boundary"
        ));
        assert!(!has_decision_marker("just some ordinary notes"));
        assert!(!has_decision_marker(""));
    }

    #[test]
    fn extracts_user_prompt_from_string_content() {
        let digest = digest_from_lines(lines(&[
            r#"{"type":"user","message":{"role":"user","content":"Add a cosine metric to the store"}}"#,
        ]));
        assert_eq!(
            digest.user_prompts,
            vec!["Add a cosine metric to the store"]
        );
        assert!(digest.decisions.is_empty());
    }

    #[test]
    fn extracts_text_from_content_block_array_and_skips_tool_blocks() {
        let digest = digest_from_lines(lines(&[
            r#"{"message":{"role":"assistant","content":[{"type":"text","text":"We decided to use cosine distance."},{"type":"tool_use","name":"Edit","input":{}}]}}"#,
            // A tool_result-only user turn carries no text → not a prompt.
            r#"{"message":{"role":"user","content":[{"type":"tool_result","content":"ok"}]}}"#,
        ]));
        assert!(
            digest.user_prompts.is_empty(),
            "a tool-result-only user turn is not a prompt: {digest:?}"
        );
        assert_eq!(digest.decisions, vec!["We decided to use cosine distance."]);
    }

    #[test]
    fn collects_decisions_from_either_role_and_dedupes() {
        let digest = digest_from_lines(lines(&[
            r#"{"message":{"role":"user","content":"let's use porter tokenizer instead of unicode61"}}"#,
            r#"{"message":{"role":"assistant","content":"The chosen approach is porter."}}"#,
            // An EXACT repeat of the prior decision line must coalesce.
            r#"{"message":{"role":"assistant","content":"The chosen approach is porter."}}"#,
        ]));
        // The user prompt is captured; decisions from both sides are collected.
        assert_eq!(
            digest.user_prompts,
            vec!["let's use porter tokenizer instead of unicode61"]
        );
        assert!(digest
            .decisions
            .iter()
            .any(|d| d.contains("porter tokenizer instead")));
        assert_eq!(
            digest
                .decisions
                .iter()
                .filter(|d| *d == "The chosen approach is porter.")
                .count(),
            1,
            "an exact-duplicate decision line coalesces to one"
        );
    }

    #[test]
    fn malformed_and_empty_lines_are_skipped_never_panic() {
        let digest = digest_from_lines(lines(&[
            "",
            "   ",
            "{ not valid json",
            r#"{"type":"summary","summary":"older session"}"#,
            r#"{"message":{"role":"system","content":"system prompt text"}}"#,
            r#"{"message":{"role":"user","content":"the real goal"}}"#,
        ]));
        // Only the genuine user turn survives; system/summary/garbage drop out.
        assert_eq!(digest.user_prompts, vec!["the real goal"]);
    }

    #[test]
    fn prompts_and_decisions_are_capped() {
        let mut raw = Vec::new();
        for i in 0..(MAX_PROMPTS + 10) {
            raw.push(format!(
                r#"{{"message":{{"role":"user","content":"goal {i}"}}}}"#
            ));
        }
        let digest = digest_from_lines(raw);
        assert_eq!(digest.user_prompts.len(), MAX_PROMPTS, "prompts capped");
    }

    #[test]
    fn line_budget_bounds_parsing() {
        let mut raw = Vec::new();
        // Far past MAX_LINES; the unique prompt sits beyond the budget and is
        // never reached, proving the bound holds.
        for _ in 0..(MAX_LINES + 50) {
            raw.push(r#"{"message":{"role":"assistant","content":"noise"}}"#.to_string());
        }
        raw.push(r#"{"message":{"role":"user","content":"unreached goal"}}"#.to_string());
        let digest = digest_from_lines(raw);
        assert!(
            !digest.user_prompts.iter().any(|p| p == "unreached goal"),
            "lines past the budget must not be parsed"
        );
    }

    #[test]
    fn read_digest_missing_file_is_empty() {
        let digest = read_digest(Path::new("/nonexistent/transcript/xyz.jsonl"));
        assert!(digest.is_empty());
    }

    #[test]
    fn read_digest_parses_a_real_jsonl_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("transcript.jsonl");
        std::fs::write(
            &path,
            concat!(
                r#"{"message":{"role":"user","content":"Implement W3.1 capture inversion"}}"#,
                "\n",
                r#"{"message":{"role":"assistant","content":"Decided to fold scratch at SessionEnd."}}"#,
                "\n",
            ),
        )
        .unwrap();
        let digest = read_digest(&path);
        assert_eq!(
            digest.user_prompts,
            vec!["Implement W3.1 capture inversion"]
        );
        assert_eq!(
            digest.decisions,
            vec!["Decided to fold scratch at SessionEnd."]
        );
    }
}
