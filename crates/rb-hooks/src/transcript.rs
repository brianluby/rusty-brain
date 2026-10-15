//! Bounded initial-goal and recent-evidence transcript windows. Text is untrusted.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

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

pub fn has_decision_marker(text: &str) -> bool {
    let lower = text.to_lowercase();
    DECISION_MARKERS.iter().any(|m| lower.contains(m))
}

const MAX_TRANSCRIPT_BYTES: usize = 1024 * 1024;
const HEAD_BYTES: usize = MAX_TRANSCRIPT_BYTES / 4;
const MAX_LINES: usize = 4000;
const MAX_PROMPTS: usize = 20;
const MAX_DECISIONS: usize = 30;
const MAX_ENTRY_CHARS: usize = 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceStatus {
    Confirmed,
    Correction,
    Proposal,
    AssistantClaim,
    ObservedSuccess,
    ObservedFailure,
    ObservedUnknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptEvidence {
    pub text: String,
    pub role: String,
    pub status: EvidenceStatus,
    /// Subject named by an explicit labeled statement or assistant proposal.
    pub subject: Option<String>,
    pub message_id: Option<String>,
    pub source: String,
    pub offset: usize,
    pub text_line: usize,
    /// False when this statement or its source message was truncated.
    pub complete: bool,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TranscriptDigest {
    pub user_prompts: Vec<String>,
    /// Confirmed user statements only, oldest to newest. Proposals live in evidence.
    pub decisions: Vec<String>,
    pub evidence: Vec<TranscriptEvidence>,
    pub incomplete: bool,
    /// Adapter-side omissions cannot be measured from the retained envelope.
    pub adapter_window: bool,
    pub skipped_bytes: usize,
    pub skipped_lines: usize,
    pub window: String,
}

impl TranscriptDigest {
    pub fn is_empty(&self) -> bool {
        self.user_prompts.is_empty() && self.decisions.is_empty() && self.evidence.is_empty()
    }

    pub fn confirmed_evidence(&self) -> impl Iterator<Item = &TranscriptEvidence> {
        self.evidence
            .iter()
            .enumerate()
            .filter(|(index, evidence)| {
                matches!(
                    evidence.status,
                    EvidenceStatus::Confirmed | EvidenceStatus::Correction
                ) && !self.evidence[index + 1..].iter().any(|later| {
                    later.status == EvidenceStatus::Correction
                        && evidence.subject.is_some()
                        && later.subject == evidence.subject
                })
            })
            .map(|(_, evidence)| evidence)
    }
}

/// Reject special files before open (in particular a FIFO without a writer).
/// Filesystem operations can still stall; this is not a wall-clock guarantee.
pub fn read_digest(path: &Path) -> TranscriptDigest {
    let Ok(metadata) = std::fs::metadata(path) else {
        return TranscriptDigest::default();
    };
    if !metadata.is_file() {
        return TranscriptDigest::default();
    }
    let Ok(mut file) = std::fs::File::open(path) else {
        return TranscriptDigest::default();
    };
    let size = usize::try_from(metadata.len()).unwrap_or(usize::MAX);
    let mut head = Vec::new();
    let head_limit = if size > MAX_TRANSCRIPT_BYTES {
        HEAD_BYTES - 1
    } else {
        size
    };
    if (&mut file)
        .take(head_limit as u64)
        .read_to_end(&mut head)
        .is_err()
    {
        return TranscriptDigest::default();
    }
    let source = path.to_string_lossy();
    if size <= MAX_TRANSCRIPT_BYTES {
        return digest_windows(&head, None, size, &source);
    }
    let tail_start = size.saturating_sub(MAX_TRANSCRIPT_BYTES - HEAD_BYTES);
    // Reserve one head byte to inspect the unchanged tail window's boundary.
    if file
        .seek(SeekFrom::Start(tail_start.saturating_sub(1) as u64))
        .is_err()
    {
        let mut digest = digest_windows(&head, None, size, &source);
        digest.incomplete = true;
        return digest;
    }
    let mut tail = Vec::new();
    let _ = file
        .take((MAX_TRANSCRIPT_BYTES - HEAD_BYTES + 1) as u64)
        .read_to_end(&mut tail);
    let boundary_is_newline = tail.first() == Some(&b'\n');
    let tail = tail.get(1..).unwrap_or_default();
    digest_windows(
        &head,
        Some((tail, tail_start, !boundary_is_newline)),
        size,
        &source,
    )
}

/// Inline adapters have a byte-bounded envelope; retain head and tail line windows.
pub fn digest_from_lines<I, S>(lines: I) -> TranscriptDigest
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut bytes = Vec::new();
    let mut capped = false;
    for line in lines {
        let line = line.as_ref();
        if bytes.len().saturating_add(line.len()).saturating_add(1) > MAX_TRANSCRIPT_BYTES {
            capped = true;
            break;
        }
        bytes.extend_from_slice(line.as_bytes());
        bytes.push(b'\n');
    }
    let mut digest = digest_windows(&bytes, None, bytes.len(), "inline transcript");
    digest.incomplete |= capped;
    digest
}

fn digest_windows(
    head: &[u8],
    tail: Option<(&[u8], usize, bool)>,
    size: usize,
    source: &str,
) -> TranscriptDigest {
    let mut records = Vec::new();
    fn records_in<'a>(
        records: &mut Vec<(usize, &'a [u8])>,
        bytes: &'a [u8],
        base: usize,
        drop_first: bool,
        complete: bool,
    ) {
        let mut offset = base;
        for (index, line) in bytes.split_inclusive(|b| *b == b'\n').enumerate() {
            if !(drop_first && index == 0) && (line.ends_with(b"\n") || complete) {
                records.push((offset, line));
            }
            offset += line.len();
        }
    }
    records_in(
        &mut records,
        head,
        0,
        false,
        tail.is_none() && head.len() == size,
    );
    if let Some((bytes, offset, partial_first)) = tail {
        records_in(&mut records, bytes, offset, partial_first, true);
    }
    let skipped_lines = records.len().saturating_sub(MAX_LINES);
    if records.len() > MAX_LINES {
        let tail = records.split_off(records.len() - (MAX_LINES * 3 / 4));
        records.truncate(MAX_LINES / 4);
        records.extend(tail);
    }
    let retained_bytes: usize = records.iter().map(|(_, line)| line.len()).sum();
    let mut digest = TranscriptDigest {
        incomplete: tail.is_some() || skipped_lines > 0 || retained_bytes < size,
        skipped_bytes: size.saturating_sub(retained_bytes),
        skipped_lines,
        window: if tail.is_some() || skipped_lines > 0 {
            "initial + recent; middle omitted"
        } else {
            "complete"
        }
        .into(),
        ..Default::default()
    };
    for (offset, line) in records {
        let Ok(entry) = serde_json::from_slice::<serde_json::Value>(line) else {
            digest.incomplete = true;
            continue;
        };
        if let Some(window) = entry.get("capture_window") {
            digest.adapter_window = true;
            digest.incomplete |= window
                .get("incomplete")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true);
            digest.window = format!("adapter window: {window}");
            continue;
        }
        let message = entry.get("message").unwrap_or(&entry);
        let Some(role) = message.get("role").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let id = entry
            .get("uuid")
            .or_else(|| entry.get("id"))
            .or_else(|| message.get("id"))
            .and_then(serde_json::Value::as_str)
            .filter(|s| s.len() <= MAX_ENTRY_CHARS)
            .map(str::to_string);
        let position = entry
            .get("capture_order")
            .and_then(serde_json::Value::as_u64)
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or(offset);
        let source = if entry.get("capture_order").is_some() {
            format!("{source}; branch_entry")
        } else {
            format!("{source}; byte_offset")
        };
        if role == "toolResult" || role == "tool" {
            let status = result_status(message);
            add_evidence(
                &mut digest,
                TranscriptEvidence {
                    text: "Tool result observed; raw output omitted".into(),
                    role: role.into(),
                    status,
                    subject: None,
                    message_id: id,
                    source: source.clone(),
                    offset: position,
                    text_line: 0,
                    complete: true,
                },
            );
            continue;
        }
        if role != "user" && role != "assistant" {
            continue;
        }
        let content = message.get("content").or_else(|| entry.get("content"));
        let Some(content) = content else { continue };
        // Tool-result blocks are observations, never user confirmations.
        if let Some(blocks) = content.as_array() {
            for block in blocks.iter().filter(|b| {
                b.get("type").and_then(serde_json::Value::as_str) == Some("tool_result")
            }) {
                add_evidence(
                    &mut digest,
                    TranscriptEvidence {
                        text: "Tool result observed; raw output omitted".into(),
                        role: "tool_result".into(),
                        status: result_status(block),
                        subject: None,
                        message_id: block
                            .get("tool_use_id")
                            .and_then(serde_json::Value::as_str)
                            .filter(|s| s.len() <= MAX_ENTRY_CHARS)
                            .map(str::to_string),
                        source: source.clone(),
                        offset: position,
                        text_line: 0,
                        complete: true,
                    },
                );
            }
        }
        let text = content_text(content);
        if role == "user" && digest.user_prompts.len() < MAX_PROMPTS && !text.trim().is_empty() {
            let prompt = truncate(text.trim()).to_string();
            if text.trim().chars().count() > MAX_ENTRY_CHARS {
                digest.incomplete = true;
            }
            if !digest.user_prompts.contains(&prompt) {
                digest.user_prompts.push(prompt);
            }
        }
        let message_complete = entry
            .get("content_complete")
            .and_then(serde_json::Value::as_bool)
            != Some(false);
        let mut fence = None;
        for (text_line, raw) in text.lines().enumerate() {
            let raw = raw.trim();
            let delimiter = raw.as_bytes().first().copied();
            let fence_width = raw
                .bytes()
                .take_while(|byte| Some(*byte) == delimiter)
                .count();
            if let Some((open, width)) = fence {
                if delimiter == Some(open)
                    && fence_width >= width
                    && raw[fence_width..].trim().is_empty()
                {
                    fence = None;
                }
                continue;
            }
            if matches!(delimiter, Some(b'`' | b'~')) && fence_width >= 3 {
                fence = delimiter.map(|delimiter| (delimiter, fence_width));
                continue;
            }
            let complete = message_complete && raw.chars().count() <= MAX_ENTRY_CHARS;
            let lower = raw.to_lowercase();
            // Scoped adoption names exactly one retained assistant proposal.
            let adopted_subject = lower
                .strip_prefix("adopt proposal:")
                .or_else(|| lower.strip_prefix("yes, use that for "))
                .map(str::trim);
            if role == "user" && complete {
                if let Some(subject) = adopted_subject {
                    let proposals: Vec<_> = digest
                        .evidence
                        .iter()
                        .filter(|e| {
                            e.role == "assistant"
                                && e.status == EvidenceStatus::Proposal
                                && e.subject.as_deref() == Some(subject)
                        })
                        .collect();
                    if proposals.len() == 1 && proposals[0].complete {
                        let proposal = proposals[0];
                        let adopted = TranscriptEvidence {
                            text: format!("{raw}\nAdopted proposal: {}", proposal.text),
                            role: role.into(),
                            status: EvidenceStatus::Confirmed,
                            subject: Some(subject.into()),
                            message_id: id.clone(),
                            source: format!(
                                "{source}; adopted_from={} message={:?} position={}",
                                proposal.source, proposal.message_id, proposal.offset
                            ),
                            offset: position,
                            text_line,
                            complete: true,
                        };
                        add_evidence(&mut digest, adopted);
                        continue;
                    }
                }
            }
            let correction = lower.starts_with("correction:") || lower.starts_with("reversal:");
            let labeled = [
                "decision:",
                "decided:",
                "constraint:",
                "preference:",
                "correction:",
                "reversal:",
            ]
            .iter()
            .any(|prefix| lower.starts_with(prefix));
            let explicit = labeled
                || ["we decided ", "i decided ", "i prefer ", "i confirm "]
                    .iter()
                    .any(|prefix| lower.starts_with(prefix));
            let planning = raw.ends_with('?')
                || raw.starts_with('>')
                || raw.starts_with('"')
                || raw.starts_with('\'')
                || adopted_subject.is_some()
                || (!explicit
                    && [
                        "might ",
                        "maybe ",
                        "could ",
                        "should we ",
                        "what if ",
                        "not decided",
                        "haven't decided",
                        "do not adopt",
                        "don't adopt",
                        "assistant said",
                        "assistant decided",
                    ]
                    .iter()
                    .any(|m| lower.contains(m)))
                || ["propose ", "i propose ", "we propose "]
                    .iter()
                    .any(|phrase| lower.starts_with(phrase))
                || [
                    "reject proposal",
                    "reject that proposal",
                    "reject the proposal",
                ]
                .iter()
                .any(|phrase| {
                    lower
                        .strip_prefix("no, ")
                        .unwrap_or(&lower)
                        .starts_with(phrase)
                });
            let status = if role == "user" && explicit && !planning {
                if correction {
                    EvidenceStatus::Correction
                } else {
                    EvidenceStatus::Confirmed
                }
            } else if explicit || has_decision_marker(raw) || planning || lower.contains("proposal")
            {
                EvidenceStatus::Proposal
            } else if role == "assistant"
                && ["passed", "succeeded", "verified"]
                    .iter()
                    .any(|m| lower.contains(m))
            {
                EvidenceStatus::AssistantClaim
            } else {
                continue;
            };
            let subject = if labeled || lower.starts_with("proposal:") {
                raw.split_once(':')
                    .and_then(|(_, rest)| rest.split_once(':'))
                    .map(|(s, _)| s.trim().to_lowercase())
                    .filter(|s| !s.is_empty())
            } else {
                None
            };
            // A truncated statement cannot safely become a durable constraint.
            let status = if !complete {
                digest.incomplete = true;
                EvidenceStatus::Proposal
            } else {
                status
            };
            add_evidence(
                &mut digest,
                TranscriptEvidence {
                    text: truncate(raw).into(),
                    role: role.into(),
                    status,
                    subject,
                    message_id: id.clone(),
                    source: source.clone(),
                    offset: position,
                    text_line,
                    complete,
                },
            );
        }
    }
    digest.decisions = digest
        .confirmed_evidence()
        .map(|e| e.text.clone())
        .collect();
    digest
}

fn result_status(value: &serde_json::Value) -> EvidenceStatus {
    if value
        .get("is_error")
        .or_else(|| value.get("isError"))
        .and_then(serde_json::Value::as_bool)
        == Some(true)
        || value
            .get("exit_code")
            .or_else(|| value.get("exitCode"))
            .and_then(serde_json::Value::as_i64)
            .is_some_and(|n| n != 0)
    {
        EvidenceStatus::ObservedFailure
    } else if value
        .get("is_error")
        .or_else(|| value.get("isError"))
        .and_then(serde_json::Value::as_bool)
        == Some(false)
        || value
            .get("exit_code")
            .or_else(|| value.get("exitCode"))
            .and_then(serde_json::Value::as_i64)
            == Some(0)
    {
        EvidenceStatus::ObservedSuccess
    } else {
        EvidenceStatus::ObservedUnknown
    }
}

fn add_evidence(digest: &mut TranscriptDigest, evidence: TranscriptEvidence) {
    if evidence.text.is_empty() {
        return;
    }
    if let Some(index) = digest.evidence.iter().position(|e| {
        e.text == evidence.text && e.role == evidence.role && e.status == evidence.status
    }) {
        digest.evidence.remove(index);
    }
    let confirmed = |status| {
        matches!(
            status,
            EvidenceStatus::Confirmed | EvidenceStatus::Correction
        )
    };
    if digest
        .evidence
        .iter()
        .filter(|e| confirmed(e.status) == confirmed(evidence.status))
        .count()
        == MAX_DECISIONS
    {
        if let Some(index) = digest
            .evidence
            .iter()
            .position(|e| confirmed(e.status) == confirmed(evidence.status))
        {
            digest.evidence.remove(index);
            digest.incomplete = true;
        }
    }
    digest.evidence.push(evidence);
}

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

fn truncate(value: &str) -> &str {
    value
        .char_indices()
        .nth(MAX_ENTRY_CHARS)
        .map_or(value, |(idx, _)| &value[..idx])
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn turn(role: &str, content: &str) -> String {
        serde_json::json!({"uuid": "actual-message", "message": {"role": role, "content": content}})
            .to_string()
    }

    #[test]
    fn roles_adoption_rejection_negation_and_claims() {
        let digest = digest_from_lines([
            turn("assistant", "Decision: storage: use PostgreSQL"),
            turn("user", "No, reject that proposal"),
            turn("user", "yes"),
            turn("user", "Maybe we will use PostgreSQL"),
            turn("user", "Decision: storage: use SQLite"),
            turn("user", "Constraint: logging: must not log tokens"),
            turn("assistant", "Tests passed and verified"),
        ]);
        assert_eq!(digest.decisions.len(), 2);
        assert!(!digest.decisions.iter().any(|d| d.contains("PostgreSQL")));
        assert_eq!(
            digest
                .evidence
                .iter()
                .find(|e| e.text.contains("SQLite"))
                .unwrap()
                .message_id
                .as_deref(),
            Some("actual-message")
        );
        assert!(digest
            .evidence
            .iter()
            .any(|e| e.status == EvidenceStatus::AssistantClaim));
    }

    #[test]
    fn large_and_many_line_transcripts_retain_late_reversal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("transcript.jsonl");
        let mut text = format!("{}\n", turn("user", "Decision: storage: use PostgreSQL"));
        for _ in 0..5000 {
            text.push_str(&format!(
                "{}\n",
                turn("assistant", &"ordinary activity ".repeat(20))
            ));
        }
        text.push_str(&format!(
            "{}\n",
            turn(
                "user",
                "Correction: storage: use SQLite instead of PostgreSQL"
            )
        ));
        assert!(text.len() > MAX_TRANSCRIPT_BYTES);
        std::fs::write(&path, text).unwrap();
        let digest = read_digest(&path);
        assert!(digest.incomplete && digest.skipped_bytes > 0);
        assert!(digest
            .decisions
            .iter()
            .any(|s| s.contains("Correction: storage: use SQLite")));
        assert!(digest.user_prompts[0].contains("PostgreSQL"));
        assert_eq!(
            digest.evidence.last().unwrap().subject.as_deref(),
            Some("storage")
        );
    }

    #[test]
    fn line_cap_keeps_head_and_tail_and_reports_exact_skips() {
        let mut lines = vec![turn("user", "Decision: storage: use PostgreSQL")];
        lines.extend((0..5000).map(|_| turn("assistant", "activity")));
        lines.push(turn("user", "Correction: storage: use SQLite"));
        let digest = digest_from_lines(lines);
        assert_eq!(digest.skipped_lines, 1002);
        assert!(digest.incomplete);
        assert!(digest.decisions.last().unwrap().contains("SQLite"));
    }

    #[test]
    fn observed_results_are_not_assistant_assertions() {
        let digest = digest_from_lines([
            r#"{"message":{"role":"assistant","content":"cargo test passed"}}"#,
            r#"{"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"real-tool","is_error":true,"content":"tests failed"}]}}"#,
            r#"{"message":{"role":"toolResult","isError":false,"content":[{"type":"text","text":"ok"}]}}"#,
            r#"{"message":{"role":"tool","content":"unknown outcome"}}"#,
        ]);
        assert!(digest.decisions.is_empty());
        assert_eq!(
            digest.evidence.iter().map(|e| e.status).collect::<Vec<_>>(),
            vec![
                EvidenceStatus::AssistantClaim,
                EvidenceStatus::ObservedFailure,
                EvidenceStatus::ObservedSuccess,
                EvidenceStatus::ObservedUnknown
            ]
        );
    }

    #[test]
    fn scoped_adoption_and_ambiguous_confirmation() {
        let adopted = digest_from_lines([
            turn("assistant", "Decision: storage: use SQLite"),
            turn("user", "yes, use that for storage"),
        ]);
        assert_eq!(adopted.decisions.len(), 1);
        assert!(adopted.decisions[0].contains("Adopted proposal: Decision: storage: use SQLite"));
        assert!(adopted
            .evidence
            .last()
            .unwrap()
            .source
            .contains("adopted_from="));
        for user in [
            "yes",
            "yes, use that for cache",
            "Reject proposal: storage",
            "Should we use SQLite?",
            "\"We decided to use SQLite\"",
        ] {
            let digest = digest_from_lines([
                turn("assistant", "Decision: storage: use SQLite"),
                turn("user", user),
            ]);
            assert!(digest.decisions.is_empty(), "{user}");
        }
        let ambiguous = digest_from_lines([
            turn("assistant", "Decision: storage: use SQLite"),
            turn("assistant", "Decision: storage: use PostgreSQL"),
            turn("user", "Adopt proposal: storage"),
        ]);
        assert!(ambiguous.decisions.is_empty());
    }

    #[test]
    fn rejection_actions_remain_confirmed_and_explicit_proposals_can_be_adopted() {
        let correction = digest_from_lines([
            turn("user", "Decided: auth: allow anonymous requests"),
            turn("user", "Correction: auth: reject anonymous requests"),
        ]);
        assert_eq!(
            correction.decisions,
            vec!["Correction: auth: reject anonymous requests"]
        );
        assert_eq!(
            correction.confirmed_evidence().next().unwrap().status,
            EvidenceStatus::Correction
        );
        for rejection in [
            "Reject proposal: anonymous access",
            "No, reject that proposal",
            "Reject the proposal",
        ] {
            assert!(digest_from_lines([turn("user", rejection)])
                .decisions
                .is_empty());
        }
        let policy = digest_from_lines([turn(
            "user",
            "Constraint: review: reject proposals that include secrets",
        )]);
        assert_eq!(
            policy.decisions,
            vec!["Constraint: review: reject proposals that include secrets"]
        );
        let adopted = digest_from_lines([
            turn("assistant", "Proposal: storage: use SQLite only in tests"),
            turn("user", "Adopt proposal: storage"),
        ]);
        assert_eq!(
            adopted.decisions,
            vec!["Adopt proposal: storage\nAdopted proposal: Proposal: storage: use SQLite only in tests"]
        );
        assert_eq!(adopted.evidence[0].status, EvidenceStatus::Proposal);
    }

    #[test]
    fn labeled_facts_with_proposal_terminology_keep_confirmation() {
        for label in [
            "Decision",
            "Constraint",
            "Preference",
            "Correction",
            "Reversal",
        ] {
            let statement = format!("{label}: deployment: use the proposed blue/green rollout");
            let digest = digest_from_lines([
                turn("user", "Decision: deployment: use rolling releases"),
                turn("user", &statement),
            ]);
            assert!(digest.decisions.contains(&statement), "{label}");
            if matches!(label, "Correction" | "Reversal") {
                assert_eq!(digest.decisions, vec![statement]);
            }
        }
        for proposal in [
            "Propose a blue/green rollout",
            "I propose a blue/green rollout",
            "We propose a blue/green rollout",
        ] {
            let digest = digest_from_lines([turn("user", proposal)]);
            assert!(digest.decisions.is_empty(), "{proposal}");
            assert_eq!(digest.evidence[0].status, EvidenceStatus::Proposal);
        }
    }

    #[test]
    fn labeled_error_text_is_not_generic_planning() {
        for value in [
            "preserve the \"could not connect\" error text",
            "display \"not decided\" for an unset status",
            "keep \"assistant said\" as quoted diagnostic output",
        ] {
            let statement = format!("Correction: diagnostics: {value}");
            let digest = digest_from_lines([
                turn("user", "Decision: diagnostics: omit connection errors"),
                turn("user", &statement),
            ]);
            assert_eq!(digest.decisions, vec![statement]);
            assert_eq!(
                digest.confirmed_evidence().next().unwrap().status,
                EvidenceStatus::Correction
            );
        }
    }

    #[test]
    fn explicit_confirmation_preserves_quoted_planning_words() {
        let statement = "I confirm the error text is \"could not connect\"";
        let digest = digest_from_lines([turn("user", statement)]);
        assert_eq!(digest.decisions, vec![statement]);
        assert_eq!(digest.evidence[0].status, EvidenceStatus::Confirmed);
    }

    #[test]
    fn one_off_task_instructions_do_not_become_confirmed_policy() {
        let digest = digest_from_lines([
            turn("user", "Use ripgrep to find the failing call"),
            turn("user", "Use cargo test to verify it"),
            turn("user", "Adopt the patch for this task"),
            turn("user", "Fix the parser; it must not panic on empty input"),
            turn("user", "This patch should not add a dependency"),
            turn("user", "Actually, use MySQL for another component"),
            turn("user", "Decision: storage: use SQLite"),
            turn("user", "Constraint: parser: must not panic on empty input"),
        ]);
        assert_eq!(
            digest.decisions,
            vec![
                "Decision: storage: use SQLite",
                "Constraint: parser: must not panic on empty input"
            ]
        );
        assert!(digest
            .evidence
            .iter()
            .filter(|e| e.text.contains("patch") || e.text.starts_with("Fix the parser"))
            .all(|e| e.status == EvidenceStatus::Proposal));
    }

    #[test]
    fn same_message_correction_has_actual_line_order_and_partial_turn_is_not_confirmed() {
        let digest = digest_from_lines([turn(
            "user",
            "Decision: storage: use PostgreSQL\nCorrection: storage: use SQLite",
        )]);
        assert_eq!(digest.decisions, vec!["Correction: storage: use SQLite"]);
        assert_eq!(digest.evidence[0].offset, digest.evidence[1].offset);
        assert!(digest.evidence[0].text_line < digest.evidence[1].text_line);
        let partial = digest_from_lines([
            r#"{"content_complete":false,"id":"real-id","message":{"role":"user","content":"Decision: storage: use SQLite"}}"#,
        ]);
        assert!(partial.incomplete && partial.decisions.is_empty());
    }

    #[test]
    fn fences_exclude_confirmations_and_scoped_adoption() {
        for fence in ["```", "~~~"] {
            let quoted_adoption = digest_from_lines([
                turn("assistant", "Decision: storage: use SQLite"),
                turn(
                    "user",
                    &format!("{fence}\nAdopt proposal: storage\n{fence}"),
                ),
            ]);
            assert!(quoted_adoption.decisions.is_empty(), "{fence}");
            let digest = digest_from_lines([
                turn("assistant", &format!("{fence}text\nDecision: quoted: use Redis\n{fence}\nDecision: storage: use SQLite")),
                turn("user", &format!("{fence}\nDecision: logging: log tokens\nAdopt proposal: storage\n{fence}\nAdopt proposal: quoted\nConstraint: logging: must not log tokens")),
                turn("user", "Adopt proposal: storage"),
            ]);
            assert_eq!(digest.decisions.len(), 2, "{fence}");
            assert_eq!(
                digest.decisions[0],
                "Constraint: logging: must not log tokens"
            );
            assert!(digest.decisions[1].contains("Adopted proposal: Decision: storage: use SQLite"));
            assert!(!digest.evidence.iter().any(|e| e.text.contains("Redis")));
        }
        let unclosed = digest_from_lines([turn(
            "user",
            "Decision: outside: retain\n````\nDecision: inside: discard\n```\nDecision: still_inside: discard",
        )]);
        assert_eq!(unclosed.decisions, vec!["Decision: outside: retain"]);
    }

    #[test]
    fn adoption_requires_complete_proposal_not_complete_window() {
        for proposal in [
            turn(
                "assistant",
                &format!("Decision: storage: {}", "x".repeat(MAX_ENTRY_CHARS)),
            ),
            serde_json::json!({
                "content_complete": false,
                "message": {"role": "assistant", "content": "Decision: storage: use SQLite"}
            })
            .to_string(),
        ] {
            let digest = digest_from_lines([proposal, turn("user", "Adopt proposal: storage")]);
            assert!(digest.incomplete && digest.decisions.is_empty());
            assert!(!digest.evidence[0].complete);
        }
        let digest = digest_from_lines([
            r#"{"capture_window":{"incomplete":true}}"#.to_string(),
            turn("assistant", "Decision: storage: use SQLite"),
            turn("user", "Adopt proposal: storage"),
        ]);
        assert!(digest.incomplete);
        assert_eq!(digest.decisions.len(), 1);
        assert!(digest.evidence.iter().all(|e| e.complete));
    }

    #[test]
    fn exact_tail_boundary_retains_correction_and_discards_fragment() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("boundary.jsonl");
        let head = format!("{}\n", turn("user", "Decision: storage: use PostgreSQL"));
        let correction = format!("{}\n", turn("user", "Correction: storage: use SQLite"));
        let tail_bytes = MAX_TRANSCRIPT_BYTES - HEAD_BYTES;
        let padding = tail_bytes - correction.len() - turn("assistant", "").len() - 1;
        let tail = format!("{correction}{}\n", turn("assistant", &"x".repeat(padding)));
        assert_eq!(tail.len(), tail_bytes);
        for aligned in [true, false] {
            let mut prefix = head.clone();
            prefix.push_str(&"x".repeat(MAX_TRANSCRIPT_BYTES - prefix.len() - 1));
            prefix.push(if aligned { '\n' } else { 'x' });
            let text = format!("{prefix}{tail}");
            std::fs::write(&path, &text).unwrap();
            let digest = read_digest(&path);
            let retained = head.len() + tail_bytes - if aligned { 0 } else { correction.len() };
            assert_eq!(digest.skipped_bytes, text.len() - retained);
            assert!(digest.incomplete);
            assert!(retained < MAX_TRANSCRIPT_BYTES);
            if aligned {
                assert_eq!(digest.decisions, vec!["Correction: storage: use SQLite"]);
                assert_eq!(
                    digest.confirmed_evidence().next().unwrap().offset,
                    prefix.len()
                );
            } else {
                assert_eq!(digest.decisions, vec!["Decision: storage: use PostgreSQL"]);
                assert!(!digest
                    .evidence
                    .iter()
                    .any(|e| e.status == EvidenceStatus::Correction));
            }
        }
        assert_eq!((HEAD_BYTES - 1) + 1 + tail_bytes, MAX_TRANSCRIPT_BYTES);
    }

    #[test]
    fn missing_and_special_files_fail_open() {
        assert!(read_digest(Path::new("/nonexistent/transcript.jsonl")).is_empty());
        assert!(read_digest(Path::new("/dev/null")).is_empty());
    }
}
