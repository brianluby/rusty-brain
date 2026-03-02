//! T021: Cross-module integration round-trip test.
//!
//! Constructs a fully nested `InjectedContext` containing `Observation`s (with
//! metadata) and a `SessionSummary`, serialises the entire structure to JSON,
//! deserialises it back, and asserts full equality — exercising every layer of
//! the type hierarchy in a single pass.

use std::collections::HashMap;

use chrono::{Duration, Utc};
use types::context::InjectedContext;
use types::observation::{Observation, ObservationMetadata, ObservationType};
use types::session::SessionSummary;

/// Build an `Observation` with all fields populated, including nested metadata.
fn make_full_observation(obs_type: ObservationType, summary: &str, content: &str) -> Observation {
    let mut extra: HashMap<String, serde_json::Value> = HashMap::new();
    extra.insert("confidence".to_string(), serde_json::json!(0.91));
    extra.insert(
        "tags".to_string(),
        serde_json::json!(["rust", "types", "memory"]),
    );
    extra.insert(
        "diagnostics".to_string(),
        serde_json::json!({"cpu": 0.15, "mem_mb": 64}),
    );

    let metadata = ObservationMetadata {
        files: vec![
            "crates/types/src/lib.rs".to_string(),
            "src/main.rs".to_string(),
        ],
        platform: "darwin".to_string(),
        project_key: "rusty-brain".to_string(),
        compressed: false,
        session_id: Some("int-test-session-001".to_string()),
        extra,
    };

    Observation::new(
        obs_type,
        "integration_test_tool".to_string(),
        summary.to_string(),
        Some(content.to_string()),
        Some(metadata),
    )
    .expect("valid observation must construct without error")
}

/// Build a `SessionSummary` with all fields populated.
fn make_full_session_summary() -> SessionSummary {
    let start = Utc::now() - Duration::hours(1);
    let end = Utc::now();

    SessionSummary::new(
        "int-test-session-001".to_string(),
        start,
        end,
        3,
        vec![
            "adopt serde rename_all camelCase".to_string(),
            "pin memvid version".to_string(),
        ],
        vec![
            "crates/types/src/observation.rs".to_string(),
            "crates/types/src/session.rs".to_string(),
            "crates/types/src/context.rs".to_string(),
        ],
        "Implemented the full type system for rusty-brain.".to_string(),
    )
    .expect("valid session summary must construct without error")
}

#[test]
fn injected_context_full_nested_json_round_trip() {
    // Arrange: construct a richly nested InjectedContext.
    let recent_obs1 = make_full_observation(
        ObservationType::Discovery,
        "Discovered serde flatten behaviour",
        "The #[serde(flatten)] attribute merges extra keys into the parent object.",
    );
    let recent_obs2 = make_full_observation(
        ObservationType::Decision,
        "Decided to use camelCase for JSON keys",
        "All structs use #[serde(rename_all = \"camelCase\")] for agent-friendly output.",
    );
    let memory_obs = make_full_observation(
        ObservationType::Pattern,
        "Pattern: validate at construction time",
        "All types enforce invariants in their ::new() constructors, returning Result.",
    );
    let session = make_full_session_summary();

    let original = InjectedContext {
        recent_observations: vec![recent_obs1, recent_obs2],
        relevant_memories: vec![memory_obs],
        session_summaries: vec![session],
        token_count: 2_048,
    };

    // Act: serialize to JSON then deserialize back.
    let json =
        serde_json::to_string(&original).expect("InjectedContext serialization must succeed");
    let deserialized: InjectedContext =
        serde_json::from_str(&json).expect("InjectedContext deserialization must succeed");

    // Assert: full equality — no data loss at any nesting level.
    assert_eq!(
        original, deserialized,
        "full nested InjectedContext must round-trip without data loss"
    );

    // Spot-check nested fields to produce clear failure messages if something breaks.
    assert_eq!(deserialized.recent_observations.len(), 2);
    assert_eq!(deserialized.relevant_memories.len(), 1);
    assert_eq!(deserialized.session_summaries.len(), 1);
    assert_eq!(deserialized.token_count, 2_048);

    let obs = &deserialized.recent_observations[0];
    assert_eq!(obs.obs_type, ObservationType::Discovery);
    assert_eq!(obs.tool_name, "integration_test_tool");

    let meta = obs
        .metadata
        .as_ref()
        .expect("metadata must be Some after round-trip");
    assert_eq!(meta.platform, "darwin");
    assert_eq!(meta.project_key, "rusty-brain");
    assert_eq!(meta.files.len(), 2);
    assert_eq!(meta.session_id.as_deref(), Some("int-test-session-001"));
    assert_eq!(
        meta.extra.get("confidence"),
        Some(&serde_json::json!(0.91)),
        "nested extra field must survive round-trip"
    );

    let sess = &deserialized.session_summaries[0];
    assert_eq!(sess.id, "int-test-session-001");
    assert_eq!(sess.observation_count, 3);
    assert_eq!(sess.key_decisions.len(), 2);
    assert_eq!(sess.modified_files.len(), 3);
}
