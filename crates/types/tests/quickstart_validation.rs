//! Validates that the code examples from quickstart.md compile and run correctly.

use chrono::Utc;
use types::{
    AgentBrainError, HookInput, MindConfig, Observation, ObservationMetadata, ObservationType,
    error_codes,
};
use uuid::Uuid;

#[test]
fn quickstart_creating_an_observation() {
    let obs = Observation {
        id: Uuid::new_v4(),
        timestamp: Utc::now(),
        obs_type: ObservationType::Discovery,
        tool_name: "Read".to_string(),
        summary: "Found config pattern in main.rs".to_string(),
        content: "The main.rs file uses a builder pattern for config...".to_string(),
        metadata: Some(ObservationMetadata {
            files: vec!["src/main.rs".to_string()],
            platform: "claude".to_string(),
            project_key: "my-project".to_string(),
            compressed: false,
            session_id: Some("session-123".to_string()),
            extra: Default::default(),
        }),
    };

    assert_eq!(obs.obs_type, ObservationType::Discovery);
    assert_eq!(obs.tool_name, "Read");
    assert_eq!(obs.summary, "Found config pattern in main.rs");
    let meta = obs.metadata.as_ref().unwrap();
    assert_eq!(meta.files, vec!["src/main.rs"]);
    assert_eq!(meta.platform, "claude");
    assert_eq!(meta.project_key, "my-project");
    assert!(!meta.compressed);
    assert_eq!(meta.session_id, Some("session-123".to_string()));
}

#[test]
fn quickstart_configuration_with_defaults() {
    let config = MindConfig::default();
    assert_eq!(config.max_context_observations, 20);
    assert_eq!(config.min_confidence, 0.6);

    temp_env::with_vars(
        [
            ("MEMVID_MIND_DEBUG", None::<&str>),
            ("MEMVID_PLATFORM_MEMORY_PATH", None::<&str>),
            ("MEMVID_PLATFORM_PATH_OPT_IN", None::<&str>),
            ("MEMVID_PLATFORM", None::<&str>),
            ("CLAUDE_PROJECT_DIR", None::<&str>),
            ("OPENCODE_PROJECT_DIR", None::<&str>),
        ],
        || {
            let config = MindConfig::from_env().expect("valid config");
            assert_eq!(config.max_context_observations, 20);
            assert_eq!(config.min_confidence, 0.6);
        },
    );
}

#[test]
fn quickstart_json_round_trip() {
    let obs = Observation {
        id: Uuid::new_v4(),
        timestamp: Utc::now(),
        obs_type: ObservationType::Discovery,
        tool_name: "Read".to_string(),
        summary: "Found config pattern in main.rs".to_string(),
        content: "The main.rs file uses a builder pattern for config...".to_string(),
        metadata: Some(ObservationMetadata {
            files: vec!["src/main.rs".to_string()],
            platform: "claude".to_string(),
            project_key: "my-project".to_string(),
            compressed: false,
            session_id: Some("session-123".to_string()),
            extra: Default::default(),
        }),
    };

    let json = serde_json::to_string(&obs).unwrap();
    let deserialized: Observation = serde_json::from_str(&json).unwrap();
    assert_eq!(obs, deserialized);
}

#[test]
fn quickstart_error_handling() {
    let err = AgentBrainError::InvalidInput {
        code: error_codes::E_INPUT_EMPTY_FIELD,
        message: "observation summary cannot be empty".to_string(),
    };
    assert_eq!(err.code(), "E_INPUT_EMPTY_FIELD");
}

#[test]
fn quickstart_hook_input_parsing() {
    let json = r#"{
        "session_id": "abc123",
        "transcript_path": "/path/to/transcript.jsonl",
        "cwd": "/home/user/project",
        "permission_mode": "default",
        "hook_event_name": "PostToolUse",
        "tool_name": "Write",
        "tool_input": {"file_path": "/tmp/test.txt", "content": "hello"},
        "tool_response": {"success": true},
        "tool_use_id": "toolu_01ABC",
        "unknown_future_field": "ignored"
    }"#;

    let input: HookInput = serde_json::from_str(json).unwrap();
    assert_eq!(input.hook_event_name, "PostToolUse");
    assert_eq!(input.tool_name, Some("Write".to_string()));
}
