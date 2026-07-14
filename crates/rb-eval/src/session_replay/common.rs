use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use thiserror::Error;

use super::privacy::StrictRedactor;
use super::schema::{
    EventKind, EventRole, EvidenceAuthority, ImportStats, NormalizedEvent, NormalizedSession,
    Provenance, RedactionCategory, RejectionCategory, SourceKind, ToolEvent,
    SESSION_REPLAY_SCHEMA_VERSION,
};

#[derive(Debug, Error)]
pub enum AdapterError {
    #[error("{0} local source is unavailable")]
    SourceUnavailable(&'static str),
    #[error("local transcript I/O failed")]
    Io(#[source] std::io::Error),
    #[error("OpenCode schema/query failed")]
    Database(#[source] rusqlite::Error),
}

impl From<std::io::Error> for AdapterError {
    fn from(source: std::io::Error) -> Self {
        Self::Io(source)
    }
}

impl From<rusqlite::Error> for AdapterError {
    fn from(source: rusqlite::Error) -> Self {
        Self::Database(source)
    }
}

pub(crate) struct RawToolEvent {
    pub name: String,
    pub status: Option<String>,
    pub input: Option<String>,
    pub output: Option<String>,
}

pub(crate) struct RawEvent {
    pub raw_id: String,
    pub timestamp: DateTime<Utc>,
    pub ordinal: u64,
    pub role: EventRole,
    pub kind: EventKind,
    pub authority: EvidenceAuthority,
    pub content: Option<String>,
    pub tool: Option<RawToolEvent>,
    pub source_record_index: u64,
}

pub(crate) struct RawSession {
    pub raw_session_id: String,
    pub raw_project_id: String,
    pub raw_source_locator: String,
    pub source: SourceKind,
    pub adapter_version: &'static str,
    pub events: Vec<RawEvent>,
}

pub(crate) fn normalize_session(
    mut raw: RawSession,
    seed: u64,
    stats: &mut ImportStats,
) -> Option<NormalizedSession> {
    let session_id = pseudonymous_id("session", seed, &[&raw.raw_session_id]);
    let project_id = pseudonymous_id("project", seed, &[&raw.raw_project_id]);
    let source_locator_id = pseudonymous_id(
        "source",
        seed,
        &[source_name(raw.source), &raw.raw_source_locator],
    );
    let mut redactor = match StrictRedactor::new(seed, &raw.raw_session_id) {
        Ok(redactor) => redactor,
        Err(error) => {
            stats.reject(error.rejection_category());
            return None;
        }
    };

    raw.events.sort_by(|left, right| {
        (left.timestamp, left.ordinal).cmp(&(right.timestamp, right.ordinal))
    });
    let mut events = Vec::with_capacity(raw.events.len());
    for raw_event in raw.events {
        match normalize_event(
            raw_event,
            seed,
            &session_id,
            &project_id,
            &source_locator_id,
            raw.source,
            raw.adapter_version,
            &mut redactor,
        ) {
            Ok(event) => {
                stats.accept_event(&event);
                events.push(event);
            }
            Err(category) => stats.reject(category),
        }
    }

    let (Some(first), Some(last)) = (events.first(), events.last()) else {
        stats.reject(RejectionCategory::EmptySession);
        return None;
    };
    let started_at = first.timestamp;
    let ended_at = last.timestamp;
    stats.accepted_sessions += 1;
    Some(NormalizedSession {
        schema_version: SESSION_REPLAY_SCHEMA_VERSION.to_string(),
        session_id,
        project_id,
        source: raw.source,
        started_at,
        ended_at,
        events,
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "normalization binds all provenance fields"
)]
fn normalize_event(
    raw: RawEvent,
    seed: u64,
    session_id: &str,
    project_id: &str,
    source_locator_id: &str,
    source: SourceKind,
    adapter_version: &'static str,
    redactor: &mut StrictRedactor,
) -> Result<NormalizedEvent, RejectionCategory> {
    let mut redactions = BTreeSet::new();
    let content = sanitize_optional(redactor, raw.content, &mut redactions)?;
    let tool = raw
        .tool
        .map(|tool| sanitize_tool(redactor, tool, &mut redactions))
        .transpose()?;
    if content.as_deref().is_some_and(str::is_empty)
        && tool
            .as_ref()
            .is_none_or(|value| value.input.is_none() && value.output.is_none())
        && raw.kind != EventKind::Lifecycle
    {
        return Err(RejectionCategory::MissingContent);
    }

    let source_record_id = pseudonymous_id(
        "record",
        seed,
        &[source_name(source), source_locator_id, &raw.raw_id],
    );
    let event_id = pseudonymous_id(
        "event",
        seed,
        &[session_id, &raw.raw_id, &raw.ordinal.to_string()],
    );
    Ok(NormalizedEvent {
        schema_version: SESSION_REPLAY_SCHEMA_VERSION.to_string(),
        event_id,
        session_id: session_id.to_string(),
        project_id: project_id.to_string(),
        timestamp: raw.timestamp,
        ordinal: raw.ordinal,
        role: raw.role,
        kind: raw.kind,
        authority: raw.authority,
        provenance: Provenance {
            source,
            source_locator_id: source_locator_id.to_string(),
            source_record_id,
            source_record_index: raw.source_record_index,
            adapter_version: adapter_version.to_string(),
        },
        content,
        tool,
        redactions: redactions.into_iter().collect(),
    })
}

fn sanitize_tool(
    redactor: &mut StrictRedactor,
    raw: RawToolEvent,
    redactions: &mut BTreeSet<RedactionCategory>,
) -> Result<ToolEvent, RejectionCategory> {
    let name = sanitize_required(redactor, raw.name, redactions)?;
    let status = sanitize_optional(redactor, raw.status, redactions)?;
    let input = sanitize_optional(redactor, raw.input, redactions)?;
    let output = sanitize_optional(redactor, raw.output, redactions)?;
    Ok(ToolEvent {
        name,
        status,
        input,
        output,
    })
}

fn sanitize_required(
    redactor: &mut StrictRedactor,
    value: String,
    redactions: &mut BTreeSet<RedactionCategory>,
) -> Result<String, RejectionCategory> {
    let sanitized = redactor
        .sanitize(&value)
        .map_err(|error| error.rejection_category())?;
    redactions.extend(sanitized.categories);
    Ok(sanitized.text)
}

fn sanitize_optional(
    redactor: &mut StrictRedactor,
    value: Option<String>,
    redactions: &mut BTreeSet<RedactionCategory>,
) -> Result<Option<String>, RejectionCategory> {
    value
        .map(|raw| sanitize_required(redactor, raw, redactions))
        .transpose()
}

pub(crate) fn pseudonymous_id(prefix: &str, seed: u64, parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(seed.to_le_bytes());
    hasher.update(prefix.as_bytes());
    for part in parts {
        hasher.update([0]);
        hasher.update(part.as_bytes());
    }
    let digest = hasher.finalize();
    format!("{prefix}-{}", hex_prefix(&digest, 16))
}

fn hex_prefix(bytes: &[u8], byte_count: usize) -> String {
    bytes
        .iter()
        .take(byte_count)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn source_name(source: SourceKind) -> &'static str {
    match source {
        SourceKind::ClaudeCode => "claude_code",
        SourceKind::OpenCode => "opencode",
        SourceKind::Synthetic => "synthetic",
    }
}

pub(crate) fn value_as_text(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(text) => Some(text.clone()),
        serde_json::Value::Null => None,
        other => serde_json::to_string(other).ok(),
    }
}
