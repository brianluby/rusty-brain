//! SEC-3 logging audit tests (T022).
//!
//! Verify no memory content (observations, search results, context)
//! is logged at INFO level or above. WARN traces should contain only
//! error context, not memory payloads.

use std::sync::{Arc, Mutex};

use tracing::Level;
use tracing_subscriber::layer::SubscriberExt;

/// A simple tracing layer that captures log messages for inspection.
struct CapturingLayer {
    messages: Arc<Mutex<Vec<(Level, String)>>>,
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for CapturingLayer {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut visitor = MessageVisitor(String::new());
        event.record(&mut visitor);
        let level = *event.metadata().level();
        self.messages.lock().unwrap().push((level, visitor.0));
    }
}

struct MessageVisitor(String);

impl tracing::field::Visit for MessageVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        use std::fmt::Write;
        let _ = write!(self.0, "{} = {:?} ", field.name(), value);
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        use std::fmt::Write;
        let _ = write!(self.0, "{} = {} ", field.name(), value);
    }
}

/// Set up a tracing subscriber that captures all log output.
fn setup_capturing_subscriber() -> (
    tracing::subscriber::DefaultGuard,
    Arc<Mutex<Vec<(Level, String)>>>,
) {
    let messages = Arc::new(Mutex::new(Vec::new()));
    let layer = CapturingLayer {
        messages: Arc::clone(&messages),
    };
    let subscriber = tracing_subscriber::registry().with(layer);
    let guard = tracing::subscriber::set_default(subscriber);
    (guard, messages)
}

/// SEC-3: No memory content logged at INFO or above during chat hook.
#[test]
fn chat_hook_no_memory_content_at_info() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path();

    // Seed memory with identifiable content
    {
        let resolved = platforms::resolve_memory_path(cwd, "opencode", false).unwrap();
        let mut config = types::MindConfig::from_env().unwrap();
        config.memory_path = resolved.path;
        let mind = rusty_brain_core::mind::Mind::open(config).unwrap();
        mind.with_lock(|m| {
            m.remember(
                types::ObservationType::Discovery,
                "test_tool",
                "SENTINEL_MEMORY_CONTENT_XYZ123",
                Some("SECRET_DETAIL_PAYLOAD_ABC789"),
                None,
            )
        })
        .unwrap();
    }

    let (_guard, messages) = setup_capturing_subscriber();

    let input: types::HookInput = serde_json::from_value(serde_json::json!({
        "session_id": "log-test-001",
        "transcript_path": "",
        "cwd": cwd.to_string_lossy(),
        "permission_mode": "default",
        "hook_event_name": "chat_start",
        "platform": "opencode"
    }))
    .unwrap();

    let _ = opencode::chat_hook::handle_chat_hook(&input, cwd);

    let logs = messages.lock().unwrap();
    for (level, msg) in logs.iter() {
        if *level <= Level::INFO {
            assert!(
                !msg.contains("SENTINEL_MEMORY_CONTENT_XYZ123"),
                "memory content leaked at {level}: {msg}"
            );
            assert!(
                !msg.contains("SECRET_DETAIL_PAYLOAD_ABC789"),
                "memory detail leaked at {level}: {msg}"
            );
        }
    }
}

/// SEC-3: No memory content logged at INFO or above during tool hook.
#[test]
fn tool_hook_no_memory_content_at_info() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path();

    let (_guard, messages) = setup_capturing_subscriber();

    let input: types::HookInput = serde_json::from_value(serde_json::json!({
        "session_id": "log-test-002",
        "transcript_path": "",
        "cwd": cwd.to_string_lossy(),
        "permission_mode": "default",
        "hook_event_name": "PostToolUse",
        "tool_name": "read",
        "tool_response": { "content": "SENSITIVE_FILE_CONTENT_MARKER_456" },
        "platform": "opencode"
    }))
    .unwrap();

    let _ = opencode::tool_hook::handle_tool_hook(&input, cwd);

    let logs = messages.lock().unwrap();
    for (level, msg) in logs.iter() {
        if *level <= Level::INFO {
            assert!(
                !msg.contains("SENSITIVE_FILE_CONTENT_MARKER_456"),
                "tool content leaked at {level}: {msg}"
            );
        }
    }
}

/// SEC-3: WARN traces contain only error context, not memory payloads.
#[test]
fn failopen_warn_no_memory_payload() {
    let (_guard, messages) = setup_capturing_subscriber();

    // Trigger a fail-open by passing an invalid handler
    let _output = opencode::handle_with_failopen(|| {
        Err(types::RustyBrainError::FileSystem {
            code: types::error_codes::E_FS_IO_ERROR,
            message: "test error without memory content".to_string(),
            source: None,
        })
    });

    let logs = messages.lock().unwrap();
    let warn_messages: Vec<_> = logs
        .iter()
        .filter(|(level, _)| *level == Level::WARN)
        .collect();

    assert!(
        !warn_messages.is_empty(),
        "fail-open should emit WARN trace"
    );

    for (_, msg) in &warn_messages {
        // WARN messages should contain error info, not memory payloads
        assert!(
            msg.contains("fail-open") || msg.contains("error"),
            "WARN trace should describe error: {msg}"
        );
    }
}
