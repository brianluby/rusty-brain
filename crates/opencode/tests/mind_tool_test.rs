//! Mind tool unit tests (T012).

use opencode::mind_tool::handle_mind_tool;
use opencode::types::MindToolInput;

fn make_input(mode: &str) -> MindToolInput {
    MindToolInput {
        mode: mode.to_string(),
        query: None,
        content: None,
        limit: None,
    }
}

/// Helper: store an observation in a temp directory so search/ask/recent/stats have data.
fn seed_memory(cwd: &std::path::Path) {
    let resolved = platforms::resolve_memory_path(cwd, "opencode", false).unwrap();
    let mut config = types::MindConfig::from_env().unwrap();
    config.memory_path = resolved.path;
    let mind = rusty_brain_core::mind::Mind::open(config).unwrap();
    mind.with_lock(|m| {
        m.remember(
            types::ObservationType::Discovery,
            "test_tool",
            "authentication design decision",
            Some("JWT tokens with refresh flow"),
            None,
        )
    })
    .unwrap();
}

// ---------------------------------------------------------------------------
// SEC-8: Invalid mode returns structured error
// ---------------------------------------------------------------------------

#[test]
fn invalid_mode_returns_error_listing_valid_modes() {
    let dir = tempfile::tempdir().unwrap();
    let input = make_input("invalid_mode");
    let result = handle_mind_tool(&input, dir.path()).unwrap();

    assert!(!result.success);
    let err = result.error.unwrap();
    assert!(
        err.contains("invalid mode"),
        "error should mention invalid mode: {err}"
    );
    assert!(
        err.contains("search"),
        "error should list valid modes: {err}"
    );
}

// ---------------------------------------------------------------------------
// AC-9: Search mode
// ---------------------------------------------------------------------------

#[test]
fn search_returns_matching_observations() {
    let dir = tempfile::tempdir().unwrap();
    seed_memory(dir.path());

    let input = MindToolInput {
        mode: "search".to_string(),
        query: Some("authentication".to_string()),
        content: None,
        limit: None,
    };

    let result = handle_mind_tool(&input, dir.path()).unwrap();
    assert!(result.success, "search should succeed");
    assert!(result.data.is_some());
}

#[test]
fn search_without_query_returns_error() {
    let dir = tempfile::tempdir().unwrap();
    let input = make_input("search");

    let result = handle_mind_tool(&input, dir.path()).unwrap();
    assert!(!result.success);
    assert!(
        result.error.unwrap().contains("query is required"),
        "missing query should return error"
    );
}

// ---------------------------------------------------------------------------
// AC-10: Ask mode
// ---------------------------------------------------------------------------

#[test]
fn ask_returns_answer() {
    let dir = tempfile::tempdir().unwrap();
    seed_memory(dir.path());

    let input = MindToolInput {
        mode: "ask".to_string(),
        query: Some("what was the authentication decision?".to_string()),
        content: None,
        limit: None,
    };

    let result = handle_mind_tool(&input, dir.path()).unwrap();
    assert!(result.success, "ask should succeed");
    assert!(result.data.is_some());
}

#[test]
fn ask_without_query_returns_error() {
    let dir = tempfile::tempdir().unwrap();
    let input = make_input("ask");

    let result = handle_mind_tool(&input, dir.path()).unwrap();
    assert!(!result.success);
    assert!(result.error.unwrap().contains("query is required"));
}

// ---------------------------------------------------------------------------
// AC-11: Recent mode
// ---------------------------------------------------------------------------

#[test]
fn recent_returns_timeline() {
    let dir = tempfile::tempdir().unwrap();
    seed_memory(dir.path());

    let input = make_input("recent");
    let result = handle_mind_tool(&input, dir.path()).unwrap();

    assert!(result.success, "recent should succeed");
    let data = result.data.unwrap();
    let entries = data.as_array().unwrap();
    assert!(!entries.is_empty(), "timeline should have entries");
}

// ---------------------------------------------------------------------------
// AC-12: Stats mode
// ---------------------------------------------------------------------------

#[test]
fn stats_returns_statistics() {
    let dir = tempfile::tempdir().unwrap();
    seed_memory(dir.path());

    let input = make_input("stats");
    let result = handle_mind_tool(&input, dir.path()).unwrap();

    assert!(result.success, "stats should succeed");
    let data = result.data.unwrap();
    assert!(
        data.get("total_observations").is_some(),
        "stats should include total_observations"
    );
    assert!(
        data.get("file_size_bytes").is_some(),
        "stats should include file_size_bytes"
    );
}

// ---------------------------------------------------------------------------
// AC-13: Remember mode
// ---------------------------------------------------------------------------

#[test]
fn remember_stores_observation_and_returns_id() {
    let dir = tempfile::tempdir().unwrap();

    let input = MindToolInput {
        mode: "remember".to_string(),
        query: None,
        content: Some("Important project decision: use JWT for auth".to_string()),
        limit: None,
    };

    let result = handle_mind_tool(&input, dir.path()).unwrap();
    assert!(result.success, "remember should succeed");

    let data = result.data.unwrap();
    let id = data.get("observation_id").unwrap().as_str().unwrap();
    assert!(!id.is_empty(), "observation_id should not be empty");
}

#[test]
fn remember_without_content_returns_error() {
    let dir = tempfile::tempdir().unwrap();
    let input = make_input("remember");

    let result = handle_mind_tool(&input, dir.path()).unwrap();
    assert!(!result.success);
    assert!(result.error.unwrap().contains("content is required"));
}

// ---------------------------------------------------------------------------
// Empty results handled gracefully
// ---------------------------------------------------------------------------

#[test]
fn search_on_empty_memory_returns_empty_results() {
    let dir = tempfile::tempdir().unwrap();

    let input = MindToolInput {
        mode: "search".to_string(),
        query: Some("nonexistent topic".to_string()),
        content: None,
        limit: None,
    };

    let result = handle_mind_tool(&input, dir.path()).unwrap();
    assert!(result.success);
    let data = result.data.unwrap();
    let entries = data.as_array().unwrap();
    assert!(
        entries.is_empty(),
        "search on empty memory should return empty array"
    );
}

#[test]
fn stats_on_empty_memory_returns_zeros() {
    let dir = tempfile::tempdir().unwrap();

    let input = make_input("stats");
    let result = handle_mind_tool(&input, dir.path()).unwrap();

    assert!(result.success);
    let data = result.data.unwrap();
    assert_eq!(data.get("total_observations").unwrap().as_u64().unwrap(), 0);
}

#[test]
fn recent_on_empty_memory_returns_empty_array() {
    let dir = tempfile::tempdir().unwrap();

    let input = make_input("recent");
    let result = handle_mind_tool(&input, dir.path()).unwrap();

    assert!(result.success);
    let data = result.data.unwrap();
    let entries = data.as_array().unwrap();
    assert!(entries.is_empty());
}

// ---------------------------------------------------------------------------
// SEC-7 / M-7: Forward compatibility — unknown JSON fields accepted
// ---------------------------------------------------------------------------

#[test]
fn forward_compat_unknown_fields_in_mind_tool_input() {
    let json = r#"{"mode": "stats", "unknown_field": "future_value", "another": 42}"#;
    let input: MindToolInput = serde_json::from_str(json).unwrap();
    assert_eq!(input.mode, "stats");
    assert!(input.query.is_none());
    assert!(input.content.is_none());
    assert!(input.limit.is_none());
}

#[test]
fn forward_compat_unknown_nested_fields_accepted() {
    let json = r#"{"mode": "search", "query": "test", "future_config": {"nested": true}}"#;
    let input: MindToolInput = serde_json::from_str(json).unwrap();
    assert_eq!(input.mode, "search");
    assert_eq!(input.query.as_deref(), Some("test"));
}

// ---------------------------------------------------------------------------
// Constitution X: Error codes in MindToolOutput error messages
// ---------------------------------------------------------------------------

#[test]
fn invalid_mode_error_includes_error_code() {
    let dir = tempfile::tempdir().unwrap();
    let input = make_input("bogus");
    let result = handle_mind_tool(&input, dir.path()).unwrap();

    assert_eq!(
        result.error_code.as_deref(),
        Some("E_INPUT_INVALID_FORMAT"),
        "invalid mode error should have structured error_code"
    );
    assert!(
        result.error.unwrap().contains("invalid mode"),
        "error message should describe the problem"
    );
}

#[test]
fn missing_query_error_includes_error_code() {
    let dir = tempfile::tempdir().unwrap();
    let input = make_input("search");
    let result = handle_mind_tool(&input, dir.path()).unwrap();

    assert_eq!(
        result.error_code.as_deref(),
        Some("E_INPUT_EMPTY_FIELD"),
        "missing query error should have structured error_code"
    );
    assert!(result.error.unwrap().contains("query is required"));
}

#[test]
fn missing_content_error_includes_error_code() {
    let dir = tempfile::tempdir().unwrap();
    let input = make_input("remember");
    let result = handle_mind_tool(&input, dir.path()).unwrap();

    assert_eq!(
        result.error_code.as_deref(),
        Some("E_INPUT_EMPTY_FIELD"),
        "missing content error should have structured error_code"
    );
    assert!(result.error.unwrap().contains("content is required"));
}

// ---------------------------------------------------------------------------
// AC-16: Plugin manifest validation
// ---------------------------------------------------------------------------

#[test]
fn plugin_manifest_has_required_fields() {
    let manifest_json = include_str!("../../../.claude-plugin/plugin.json");
    let manifest: serde_json::Value =
        serde_json::from_str(manifest_json).expect("plugin.json should be valid JSON");

    assert!(
        manifest.get("name").and_then(|v| v.as_str()).is_some(),
        "manifest must have 'name' field"
    );
    assert!(
        manifest.get("version").and_then(|v| v.as_str()).is_some(),
        "manifest must have 'version' field"
    );
    assert!(
        manifest
            .get("description")
            .and_then(|v| v.as_str())
            .is_some(),
        "manifest must have 'description' field"
    );
}
