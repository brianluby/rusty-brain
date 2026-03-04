//! Chat hook unit tests (T008).

use std::path::Path;

use opencode::chat_hook::handle_chat_hook;
use types::HookInput;

fn make_hook_input(cwd: &str) -> HookInput {
    serde_json::from_value(serde_json::json!({
        "session_id": "test-session-001",
        "transcript_path": "",
        "cwd": cwd,
        "permission_mode": "default",
        "hook_event_name": "chat_start",
        "platform": "opencode"
    }))
    .expect("valid HookInput JSON")
}

/// AC-2: Empty/new memory file returns a valid HookOutput (welcome message).
#[test]
fn new_memory_file_returns_valid_output() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path();
    let input = make_hook_input(&cwd.to_string_lossy());

    let result = handle_chat_hook(&input, cwd);
    assert!(
        result.is_ok(),
        "chat hook should succeed for new memory file"
    );

    let output = result.unwrap();
    assert!(
        output.system_message.is_some(),
        "system_message should be present"
    );
}

/// AC-1: Context injection with known memory file.
#[test]
fn known_memory_returns_context() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path();

    // First, create a memory and store an observation
    {
        let resolved = platforms::resolve_memory_path(cwd, "opencode", false).unwrap();
        let mut config = types::MindConfig::from_env().unwrap();
        config.memory_path = resolved.path;
        let mind = rusty_brain_core::mind::Mind::open(config).unwrap();
        mind.with_lock(|m| {
            m.remember(
                types::ObservationType::Discovery,
                "test_tool",
                "important finding about authentication",
                Some("detailed content here"),
                None,
            )
        })
        .unwrap();
    }

    let input = make_hook_input(&cwd.to_string_lossy());
    let result = handle_chat_hook(&input, cwd);
    assert!(result.is_ok());

    let output = result.unwrap();
    let msg = output.system_message.unwrap();
    assert!(
        msg.contains("Memory Context"),
        "system_message should contain Memory Context header"
    );
}

/// AC-4: Topic-relevant query passes to Mind::get_context(Some(query)).
#[test]
fn topic_query_passes_to_get_context() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path();

    // Store an observation first
    {
        let resolved = platforms::resolve_memory_path(cwd, "opencode", false).unwrap();
        let mut config = types::MindConfig::from_env().unwrap();
        config.memory_path = resolved.path;
        let mind = rusty_brain_core::mind::Mind::open(config).unwrap();
        mind.with_lock(|m| {
            m.remember(
                types::ObservationType::Discovery,
                "test_tool",
                "authentication module design",
                None,
                None,
            )
        })
        .unwrap();
    }

    let mut input = make_hook_input(&cwd.to_string_lossy());
    input.prompt = Some("Tell me about authentication".to_string());

    let result = handle_chat_hook(&input, cwd);
    assert!(result.is_ok(), "chat hook with query should succeed");

    let output = result.unwrap();
    assert!(output.system_message.is_some());
}

/// AC-18: Memory path resolved via resolve_memory_path(cwd, "opencode", false).
#[test]
fn memory_path_uses_legacy_first() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path();
    let input = make_hook_input(&cwd.to_string_lossy());

    let result = handle_chat_hook(&input, cwd);
    assert!(result.is_ok());

    // Verify the legacy path was created
    let legacy_path = cwd.join(".agent-brain").join("mind.mv2");
    assert!(
        legacy_path.exists(),
        "legacy memory path should be created: {}",
        legacy_path.display()
    );
}

/// AC-3, M-5: Error path returns Err (caller wraps in fail-open).
#[test]
fn invalid_cwd_returns_error() {
    let input = make_hook_input("/nonexistent/path");
    let cwd = Path::new("/nonexistent/path/that/does/not/exist");

    let result = handle_chat_hook(&input, cwd);
    // The handler returns Err; the fail-open wrapper in lib.rs handles conversion
    assert!(
        result.is_err(),
        "chat hook should return Err for invalid cwd"
    );
}

/// System message format includes project name.
#[test]
fn system_message_includes_project_name() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path();
    let input = make_hook_input(&cwd.to_string_lossy());

    let result = handle_chat_hook(&input, cwd).unwrap();
    let msg = result.system_message.unwrap();
    assert!(
        msg.contains("Project:"),
        "system_message should include project name"
    );
}

/// hook_specific_output contains structured InjectedContext JSON.
#[test]
fn hook_specific_output_is_injected_context_json() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path();
    let input = make_hook_input(&cwd.to_string_lossy());

    let result = handle_chat_hook(&input, cwd).unwrap();
    assert!(
        result.hook_specific_output.is_some(),
        "hook_specific_output should be present"
    );

    // Should be valid InjectedContext JSON
    let value = result.hook_specific_output.unwrap();
    let ctx: types::InjectedContext = serde_json::from_value(value)
        .expect("hook_specific_output should deserialize as InjectedContext");
    // Verify hook_specific_output deserialized successfully (implicitly validates structure)
    let _ = ctx.token_count;
}
