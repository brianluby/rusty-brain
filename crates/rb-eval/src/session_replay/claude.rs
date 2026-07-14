//! Claude Code JSONL adapter.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::common::{
    normalize_session, value_as_text, AdapterError, RawEvent, RawSession, RawToolEvent,
};
use super::schema::{
    AdapterOutput, EventKind, EventRole, EvidenceAuthority, ImportStats, RejectionCategory,
    SourceKind,
};

const ADAPTER_VERSION: &str = "claude-code-jsonl-v1";

/// Normalize all Claude Code JSONL files below `root` without writing raw input.
pub fn import_claude_jsonl(root: &Path, seed: u64) -> Result<AdapterOutput, AdapterError> {
    if !root.is_dir() {
        return Err(AdapterError::SourceUnavailable("Claude Code"));
    }
    let mut files = Vec::new();
    collect_jsonl_files(root, &mut files)?;
    files.sort();

    let mut stats = ImportStats::new(SourceKind::ClaudeCode);
    let mut raw_sessions = BTreeMap::new();
    for path in files {
        stats.source_containers += 1;
        let (session_id, project_id, scanned_records) = file_identity(&path)?;
        let Some(session_id) = session_id else {
            reject_scanned_records(
                &mut stats,
                scanned_records,
                RejectionCategory::MissingSession,
            );
            continue;
        };
        let Some(project_id) = project_id else {
            reject_scanned_records(
                &mut stats,
                scanned_records,
                RejectionCategory::MissingProject,
            );
            continue;
        };
        let locator = path.to_string_lossy().into_owned();
        let key = format!("{locator}\0{session_id}");
        let session = raw_sessions.entry(key).or_insert_with(|| RawSession {
            raw_session_id: session_id.clone(),
            raw_project_id: project_id.clone(),
            raw_source_locator: locator,
            source: SourceKind::ClaudeCode,
            adapter_version: ADAPTER_VERSION,
            events: Vec::new(),
        });
        parse_file(&path, session, &mut stats)?;
    }
    stats.source_sessions = raw_sessions.len() as u64;

    let mut sessions: Vec<_> = raw_sessions
        .into_values()
        .filter_map(|raw| normalize_session(raw, seed, &mut stats))
        .collect();
    sessions.sort_by(|left, right| {
        (left.started_at, &left.session_id).cmp(&(right.started_at, &right.session_id))
    });
    Ok(AdapterOutput { sessions, stats })
}

fn collect_jsonl_files(root: &Path, files: &mut Vec<PathBuf>) -> Result<(), AdapterError> {
    let mut entries: Vec<_> = std::fs::read_dir(root)?.collect::<Result<_, _>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_jsonl_files(&entry.path(), files)?;
        } else if file_type.is_file()
            && entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "jsonl")
        {
            files.push(entry.path());
        }
    }
    Ok(())
}

fn file_identity(path: &Path) -> Result<(Option<String>, Option<String>, u64), AdapterError> {
    let reader = BufReader::new(File::open(path)?);
    let mut session_id = None;
    let mut project_id = None;
    let mut scanned_records = 0u64;
    for line in reader.lines() {
        let line = line?;
        scanned_records += 1;
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        session_id = session_id.or_else(|| string_field(&value, "sessionId"));
        project_id = project_id.or_else(|| string_field(&value, "cwd"));
        if session_id.is_some() && project_id.is_some() {
            break;
        }
    }
    Ok((session_id, project_id, scanned_records))
}

fn reject_scanned_records(
    stats: &mut ImportStats,
    scanned_records: u64,
    category: RejectionCategory,
) {
    stats.source_records += scanned_records;
    for _ in 0..scanned_records {
        stats.reject(category);
    }
}

fn parse_file(
    path: &Path,
    session: &mut RawSession,
    stats: &mut ImportStats,
) -> Result<(), AdapterError> {
    let reader = BufReader::new(File::open(path)?);
    for (line_index, line) in reader.lines().enumerate() {
        stats.source_records += 1;
        let line = line?;
        let value = match serde_json::from_str::<Value>(&line) {
            Ok(value) => value,
            Err(_) => {
                stats.reject(RejectionCategory::MalformedJson);
                continue;
            }
        };
        parse_record(&value, line_index as u64, session, stats);
    }
    Ok(())
}

fn parse_record(value: &Value, line_index: u64, session: &mut RawSession, stats: &mut ImportStats) {
    let Some(record_type) = string_field(value, "type") else {
        stats.reject(RejectionCategory::UnsupportedRecord);
        return;
    };
    if !matches!(record_type.as_str(), "user" | "assistant" | "system") {
        if record_type == "progress" {
            stats.reject(RejectionCategory::PrivateReasoning);
        } else {
            stats.reject(RejectionCategory::UnsupportedRecord);
        }
        return;
    }
    let Some(timestamp) = parse_timestamp(value, stats) else {
        return;
    };
    let raw_id = string_field(value, "uuid")
        .or_else(|| {
            value
                .pointer("/message/id")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| format!("line-{line_index}"));

    if record_type == "system" {
        session.events.push(RawEvent {
            raw_id,
            timestamp,
            ordinal: line_index.saturating_mul(1_000),
            role: EventRole::System,
            kind: EventKind::Lifecycle,
            authority: EvidenceAuthority::SystemMetadata,
            content: None,
            tool: None,
            source_record_index: line_index,
        });
        return;
    }

    let Some(message) = value.get("message") else {
        stats.reject(RejectionCategory::MissingContent);
        return;
    };
    let expected_role = if record_type == "user" {
        EventRole::User
    } else {
        EventRole::Assistant
    };
    let Some(message_role) = message.get("role").and_then(Value::as_str) else {
        stats.reject(RejectionCategory::MissingRole);
        return;
    };
    if !role_matches(expected_role, message_role) {
        stats.reject(RejectionCategory::MissingRole);
        return;
    }
    let Some(content) = message.get("content") else {
        stats.reject(RejectionCategory::MissingContent);
        return;
    };
    match content {
        Value::String(text) => session.events.push(dialogue_event(
            raw_id,
            timestamp,
            line_index.saturating_mul(1_000),
            line_index,
            expected_role,
            text.clone(),
        )),
        Value::Array(parts) => {
            for (part_index, part) in parts.iter().enumerate() {
                parse_part(
                    part,
                    &raw_id,
                    timestamp,
                    line_index,
                    part_index as u64,
                    expected_role,
                    session,
                    stats,
                );
            }
        }
        _ => stats.reject(RejectionCategory::MissingContent),
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "Claude JSONL parts inherit line provenance"
)]
fn parse_part(
    part: &Value,
    raw_id: &str,
    timestamp: DateTime<Utc>,
    line_index: u64,
    part_index: u64,
    message_role: EventRole,
    session: &mut RawSession,
    stats: &mut ImportStats,
) {
    let ordinal = line_index.saturating_mul(1_000).saturating_add(part_index);
    let part_id = part
        .get("id")
        .and_then(Value::as_str)
        .map_or_else(|| format!("{raw_id}-part-{part_index}"), str::to_string);
    match part.get("type").and_then(Value::as_str) {
        Some("text") => {
            let Some(text) = part.get("text").and_then(Value::as_str) else {
                stats.reject(RejectionCategory::MissingContent);
                return;
            };
            session.events.push(dialogue_event(
                part_id,
                timestamp,
                ordinal,
                line_index,
                message_role,
                text.to_string(),
            ));
        }
        Some("tool_use" | "server_tool_use") => {
            let name = part
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("unknown-tool")
                .to_string();
            session.events.push(RawEvent {
                raw_id: part_id,
                timestamp,
                ordinal,
                role: EventRole::Tool,
                kind: EventKind::ToolCall,
                authority: EvidenceAuthority::SystemMetadata,
                content: None,
                tool: Some(RawToolEvent {
                    name,
                    status: Some("requested".to_string()),
                    input: part.get("input").and_then(value_as_text),
                    output: None,
                }),
                source_record_index: line_index,
            });
        }
        Some("tool_result") => {
            let status = if part.get("is_error").and_then(Value::as_bool) == Some(true) {
                "error"
            } else {
                "completed"
            };
            session.events.push(RawEvent {
                raw_id: part_id,
                timestamp,
                ordinal,
                role: EventRole::Tool,
                kind: EventKind::ToolResult,
                authority: EvidenceAuthority::ToolEvidence,
                content: None,
                tool: Some(RawToolEvent {
                    name: "tool-result".to_string(),
                    status: Some(status.to_string()),
                    input: None,
                    output: part.get("content").and_then(value_as_text),
                }),
                source_record_index: line_index,
            });
        }
        Some("thinking" | "redacted_thinking") => {
            stats.reject(RejectionCategory::PrivateReasoning);
        }
        _ => stats.reject(RejectionCategory::UnsupportedRecord),
    }
}

fn dialogue_event(
    raw_id: String,
    timestamp: DateTime<Utc>,
    ordinal: u64,
    source_record_index: u64,
    role: EventRole,
    content: String,
) -> RawEvent {
    RawEvent {
        raw_id,
        timestamp,
        ordinal,
        role,
        kind: EventKind::Dialogue,
        authority: match role {
            EventRole::User => EvidenceAuthority::UserStatement,
            EventRole::Assistant => EvidenceAuthority::AssistantUncorroborated,
            EventRole::Tool | EventRole::System => EvidenceAuthority::SystemMetadata,
        },
        content: Some(content),
        tool: None,
        source_record_index,
    }
}

fn parse_timestamp(value: &Value, stats: &mut ImportStats) -> Option<DateTime<Utc>> {
    let Some(raw) = value.get("timestamp").and_then(Value::as_str) else {
        stats.reject(RejectionCategory::MissingTimestamp);
        return None;
    };
    match DateTime::parse_from_rfc3339(raw) {
        Ok(timestamp) => Some(timestamp.with_timezone(&Utc)),
        Err(_) => {
            stats.reject(RejectionCategory::InvalidTimestamp);
            None
        }
    }
}

fn string_field(value: &Value, field: &str) -> Option<String> {
    value.get(field).and_then(Value::as_str).map(str::to_string)
}

fn role_matches(expected: EventRole, role: &str) -> bool {
    matches!(
        (expected, role),
        (EventRole::User, "user") | (EventRole::Assistant, "assistant")
    )
}
