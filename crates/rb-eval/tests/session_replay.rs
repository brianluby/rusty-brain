#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use chrono::{TimeZone, Utc};
use rb_eval::session_replay::{
    build_candidate_dataset, build_inventory_report, events_for_lane, import_claude_jsonl,
    import_opencode_db, DatasetSplit, EventKind, EventRole, EvidenceAuthority, RejectionCategory,
    ReplayLane, DEFAULT_FAKER_SEED,
};

const CLAUDE_FIXTURE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/fixtures/session_replay/claude"
);
const OPENCODE_FIXTURE_SQL: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/fixtures/session_replay/opencode/invented.sql"
));

fn open_invented_opencode_db() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("invented.db");
    let connection = rusqlite::Connection::open(path).unwrap();
    connection.execute_batch(OPENCODE_FIXTURE_SQL).unwrap();
    drop(connection);
    temp
}

#[test]
fn claude_adapter_preserves_order_and_separates_tools_from_dialogue() {
    let output = import_claude_jsonl(Path::new(CLAUDE_FIXTURE_ROOT), DEFAULT_FAKER_SEED).unwrap();
    assert_eq!(output.sessions.len(), 1);
    let session = &output.sessions[0];
    assert!(session
        .events
        .windows(2)
        .all(|window| window[0].timestamp <= window[1].timestamp));
    assert!(session
        .events
        .iter()
        .any(|event| event.kind == EventKind::ToolCall));
    assert!(session
        .events
        .iter()
        .any(|event| event.kind == EventKind::ToolResult));
    assert!(events_for_lane(session, ReplayLane::DialogueOnly)
        .iter()
        .all(|event| event.is_dialogue()));
    assert!(
        events_for_lane(session, ReplayLane::FullEvent).len()
            > events_for_lane(session, ReplayLane::DialogueOnly).len()
    );
}

#[test]
fn claude_adapter_removes_sensitive_fixture_values_and_private_reasoning() {
    let output = import_claude_jsonl(Path::new(CLAUDE_FIXTURE_ROOT), DEFAULT_FAKER_SEED).unwrap();
    let serialized = serde_json::to_string(&output).unwrap();
    for forbidden in [
        "Casey Example",
        "casey@private.test",
        "/Users/casey",
        "build.internal",
        "private reasoning must not be captured",
    ] {
        assert!(
            !serialized.contains(forbidden),
            "leaked invented value: {forbidden}"
        );
    }
    assert!(output
        .stats
        .rejections
        .get(&RejectionCategory::PrivateReasoning)
        .is_some_and(|count| *count >= 1));
}

#[test]
fn opencode_adapter_uses_parts_and_marks_committed_repository_evidence() {
    let temp = open_invented_opencode_db();
    let output = import_opencode_db(&temp.path().join("invented.db"), DEFAULT_FAKER_SEED).unwrap();
    assert_eq!(output.sessions.len(), 1);
    let events = &output.sessions[0].events;
    assert!(events.iter().any(|event| {
        event.kind == EventKind::RepositoryEvidence
            && event.authority == EvidenceAuthority::CommittedRepositoryState
            && event.role == EventRole::Tool
    }));
    assert!(events.iter().any(|event| {
        event.role == EventRole::Assistant
            && event.authority == EvidenceAuthority::AssistantUncorroborated
    }));
}

#[test]
fn chronological_dataset_keeps_whole_sessions_and_excludes_assistant_claims() {
    let claude = import_claude_jsonl(Path::new(CLAUDE_FIXTURE_ROOT), DEFAULT_FAKER_SEED).unwrap();
    let temp = open_invented_opencode_db();
    let opencode =
        import_opencode_db(&temp.path().join("invented.db"), DEFAULT_FAKER_SEED).unwrap();
    let sessions: Vec<_> = claude
        .sessions
        .iter()
        .chain(&opencode.sessions)
        .cloned()
        .collect();
    let boundary = Utc.with_ymd_and_hms(2026, 1, 15, 0, 0, 0).single().unwrap();
    let dataset = build_candidate_dataset(&sessions, boundary);

    assert_eq!(dataset.development_sessions, 1);
    assert_eq!(dataset.holdout_sessions, 1);
    assert_eq!(dataset.crossing_sessions_rejected, 0);
    assert_eq!(dataset.queries.len(), 2);
    assert!(dataset
        .queries
        .iter()
        .any(|query| query.split == DatasetSplit::Development));
    assert!(dataset
        .queries
        .iter()
        .any(|query| query.split == DatasetSplit::Holdout));
    assert!(dataset
        .candidates
        .iter()
        .all(|candidate| candidate.authority != EvidenceAuthority::AssistantUncorroborated));
    assert!(dataset
        .candidates
        .iter()
        .all(|candidate| !candidate.semantic_ground_truth));
    assert!(dataset
        .queries
        .iter()
        .all(|query| !query.semantic_ground_truth && !query.candidate_pool_ids.is_empty()));
    assert!(dataset
        .candidates
        .iter()
        .all(|candidate| !candidate.text.contains("five shards")
            && !candidate.text.contains("volatile mode")));
}

#[test]
fn sessions_crossing_boundary_are_rejected_instead_of_split() {
    let output = import_claude_jsonl(Path::new(CLAUDE_FIXTURE_ROOT), DEFAULT_FAKER_SEED).unwrap();
    let mut session = output.sessions[0].clone();
    let boundary = Utc
        .with_ymd_and_hms(2026, 1, 1, 10, 3, 30)
        .single()
        .unwrap();
    session.started_at = boundary - chrono::Duration::minutes(5);
    session.ended_at = boundary + chrono::Duration::minutes(5);
    let dataset = build_candidate_dataset(&[session], boundary);
    assert_eq!(dataset.crossing_sessions_rejected, 1);
    assert!(dataset.candidates.is_empty() && dataset.queries.is_empty());
}

#[test]
fn imports_and_aggregate_report_are_deterministic_and_content_free() {
    let first = import_claude_jsonl(Path::new(CLAUDE_FIXTURE_ROOT), DEFAULT_FAKER_SEED).unwrap();
    let second = import_claude_jsonl(Path::new(CLAUDE_FIXTURE_ROOT), DEFAULT_FAKER_SEED).unwrap();
    assert_eq!(first, second);

    let boundary = Utc.with_ymd_and_hms(2026, 1, 15, 0, 0, 0).single().unwrap();
    let dataset = build_candidate_dataset(&first.sessions, boundary);
    let report = build_inventory_report(&[&first], &dataset, DEFAULT_FAKER_SEED);
    let serialized = serde_json::to_string(&report).unwrap();
    for forbidden in ["Atlas", "Casey", "/Users", "text", "timestamp"] {
        assert!(
            !serialized.contains(forbidden),
            "aggregate report leaked field/value: {forbidden}"
        );
    }
    assert_eq!(report.semantic_ground_truth_records, 0);
}
