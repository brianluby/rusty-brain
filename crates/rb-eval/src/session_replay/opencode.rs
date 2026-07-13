//! OpenCode SQLite adapter.

use std::collections::BTreeMap;
use std::path::Path;

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;

use super::common::{
    normalize_session, value_as_text, AdapterError, RawEvent, RawSession, RawToolEvent,
};
use super::schema::{
    AdapterOutput, EventKind, EventRole, EvidenceAuthority, ImportStats, RejectionCategory,
    SourceKind,
};

const ADAPTER_VERSION: &str = "opencode-sqlite-v1";

/// Read OpenCode's local database in read-only mode and normalize its sessions.
pub fn import_opencode_db(path: &Path, seed: u64) -> Result<AdapterOutput, AdapterError> {
    if !path.is_file() {
        return Err(AdapterError::SourceUnavailable("OpenCode"));
    }
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.pragma_update(None, "query_only", true)?;

    let mut stats = ImportStats::new(SourceKind::OpenCode);
    stats.source_containers = 1;
    stats.source_records = physical_record_count(&connection)?;
    let locator = path.to_string_lossy().into_owned();
    let mut raw_sessions = load_sessions(&connection, &locator, &mut stats)?;
    load_parts(&connection, &mut raw_sessions, &mut stats)?;
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

fn physical_record_count(connection: &Connection) -> Result<u64, AdapterError> {
    let count: i64 = connection.query_row(
        "SELECT (SELECT COUNT(*) FROM session) + \
                (SELECT COUNT(*) FROM message) + \
                (SELECT COUNT(*) FROM part)",
        [],
        |row| row.get(0),
    )?;
    Ok(count.max(0) as u64)
}

fn load_sessions(
    connection: &Connection,
    locator: &str,
    stats: &mut ImportStats,
) -> Result<BTreeMap<String, RawSession>, AdapterError> {
    let mut statement = connection.prepare(
        "SELECT id, project_id, directory \
         FROM session ORDER BY time_created, id",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    let mut sessions = BTreeMap::new();
    for row in rows {
        let (session_id, project_id, directory) = row?;
        if session_id.trim().is_empty() {
            stats.reject(RejectionCategory::MissingSession);
            continue;
        }
        if project_id.trim().is_empty() || directory.trim().is_empty() {
            stats.reject(RejectionCategory::MissingProject);
            continue;
        }
        sessions.insert(
            session_id.clone(),
            RawSession {
                raw_session_id: session_id,
                raw_project_id: format!("{project_id}\0{directory}"),
                raw_source_locator: locator.to_string(),
                source: SourceKind::OpenCode,
                adapter_version: ADAPTER_VERSION,
                events: Vec::new(),
            },
        );
    }
    Ok(sessions)
}

fn load_parts(
    connection: &Connection,
    sessions: &mut BTreeMap<String, RawSession>,
    stats: &mut ImportStats,
) -> Result<(), AdapterError> {
    let mut statement = connection.prepare(
        "SELECT m.id, m.session_id, m.data, p.id, p.time_created, p.data \
         FROM message m JOIN part p ON p.message_id = m.id \
         ORDER BY m.session_id, m.time_created, m.id, p.time_created, p.id",
    )?;
    let mut rows = statement.query([])?;
    let mut source_index = 0u64;
    while let Some(row) = rows.next()? {
        let message_id: String = row.get(0)?;
        let session_id: String = row.get(1)?;
        let message_data: String = row.get(2)?;
        let part_id: String = row.get(3)?;
        let part_time: i64 = row.get(4)?;
        let part_data: String = row.get(5)?;
        let Some(session) = sessions.get_mut(&session_id) else {
            stats.reject(RejectionCategory::MissingSession);
            source_index += 1;
            continue;
        };
        let message = match serde_json::from_str::<Value>(&message_data) {
            Ok(value) => value,
            Err(_) => {
                stats.reject(RejectionCategory::MalformedJson);
                source_index += 1;
                continue;
            }
        };
        let part = match serde_json::from_str::<Value>(&part_data) {
            Ok(value) => value,
            Err(_) => {
                stats.reject(RejectionCategory::MalformedJson);
                source_index += 1;
                continue;
            }
        };
        let Some(timestamp) = DateTime::<Utc>::from_timestamp_millis(part_time) else {
            stats.reject(RejectionCategory::InvalidTimestamp);
            source_index += 1;
            continue;
        };
        parse_part(
            &message,
            &part,
            &message_id,
            &part_id,
            timestamp,
            source_index,
            session,
            stats,
        );
        source_index += 1;
    }
    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    reason = "OpenCode rows carry message and part provenance"
)]
fn parse_part(
    message: &Value,
    part: &Value,
    message_id: &str,
    part_id: &str,
    timestamp: DateTime<Utc>,
    source_index: u64,
    session: &mut RawSession,
    stats: &mut ImportStats,
) {
    let ordinal = source_index;
    let raw_id = if part_id.trim().is_empty() {
        format!("{message_id}-part-{source_index}")
    } else {
        part_id.to_string()
    };
    let Some(part_type) = part.get("type").and_then(Value::as_str) else {
        stats.reject(RejectionCategory::UnsupportedRecord);
        return;
    };
    match part_type {
        "text" => {
            let Some(role) = message_role(message) else {
                stats.reject(RejectionCategory::MissingRole);
                return;
            };
            let Some(text) = part.get("text").and_then(Value::as_str) else {
                stats.reject(RejectionCategory::MissingContent);
                return;
            };
            session.events.push(RawEvent {
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
                content: Some(text.to_string()),
                tool: None,
                source_record_index: source_index,
            });
        }
        "tool" | "subtask" | "patch" => {
            let name = part
                .get("tool")
                .or_else(|| part.get("agent"))
                .and_then(Value::as_str)
                .unwrap_or(part_type)
                .to_string();
            let input = part
                .pointer("/state/input")
                .and_then(value_as_text)
                .or_else(|| {
                    part.get("prompt")
                        .or_else(|| part.get("command"))
                        .and_then(value_as_text)
                });
            let output = part.pointer("/state/output").and_then(value_as_text);
            let status = part
                .pointer("/state/status")
                .and_then(Value::as_str)
                .map(str::to_string);
            let command = part.pointer("/state/input/command").and_then(Value::as_str);
            let committed =
                is_committed_repository_probe(&name, command, status.as_deref(), output.as_deref());
            session.events.push(RawEvent {
                raw_id,
                timestamp,
                ordinal,
                role: EventRole::Tool,
                kind: if committed {
                    EventKind::RepositoryEvidence
                } else {
                    EventKind::ToolResult
                },
                authority: if committed {
                    EvidenceAuthority::CommittedRepositoryState
                } else {
                    EvidenceAuthority::ToolEvidence
                },
                content: None,
                tool: Some(RawToolEvent {
                    name,
                    status,
                    input,
                    output,
                }),
                source_record_index: source_index,
            });
        }
        "step-start" | "step-finish" | "compaction" => {
            session.events.push(RawEvent {
                raw_id,
                timestamp,
                ordinal,
                role: EventRole::System,
                kind: EventKind::Lifecycle,
                authority: EvidenceAuthority::SystemMetadata,
                content: None,
                tool: None,
                source_record_index: source_index,
            });
        }
        "reasoning" => stats.reject(RejectionCategory::PrivateReasoning),
        _ => stats.reject(RejectionCategory::UnsupportedRecord),
    }
}

fn message_role(message: &Value) -> Option<EventRole> {
    match message.get("role").and_then(Value::as_str) {
        Some("user") => Some(EventRole::User),
        Some("assistant") => Some(EventRole::Assistant),
        _ => None,
    }
}

fn is_committed_repository_probe(
    name: &str,
    command: Option<&str>,
    status: Option<&str>,
    output: Option<&str>,
) -> bool {
    if status != Some("completed")
        || !matches!(name.to_ascii_lowercase().as_str(), "bash" | "shell")
        || output.is_none_or(|value| value.trim().is_empty())
    {
        return false;
    }
    let Some(command) = command else {
        return false;
    };
    let mut words = command.split_whitespace();
    matches!(
        (words.next(), words.next()),
        (Some("git"), Some("show" | "log" | "cat-file" | "rev-parse"))
    )
}
