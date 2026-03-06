use std::path::Path;

use crate::bootstrap;
use crate::dedup::DedupCache;
use crate::error::HookError;
use crate::truncate::head_tail_truncate;
use types::ObservationType;
use types::hooks::{HookInput, HookOutput};

const MAX_TOKENS: usize = 500;

/// Handle the post-tool-use hook event.
///
/// Extracts tool info, classifies the observation type, deduplicates,
/// truncates large output, and stores the observation in memory.
///
/// # Errors
///
/// Returns `HookError::Platform` (memory-path resolution), `HookError::Mind`,
/// or `HookError::Dedup` on failure.
#[tracing::instrument(skip(input))]
pub fn handle_post_tool_use(input: &HookInput) -> Result<HookOutput, HookError> {
    let cwd = Path::new(&input.cwd);

    if !bootstrap::should_process(input, "PostToolUse") {
        return Ok(HookOutput {
            continue_execution: Some(true),
            ..Default::default()
        });
    }

    // Emit legacy path diagnostics (best-effort, non-blocking)
    for diag in bootstrap::detect_legacy_paths(cwd) {
        match diag.level {
            bootstrap::DiagnosticLevel::Warning => {
                tracing::warn!(diagnostic = %diag.message, "legacy path detected");
            }
            bootstrap::DiagnosticLevel::Info => {
                tracing::info!(diagnostic = %diag.message, "legacy path info");
            }
        }
    }

    let tool_name = input.tool_name.as_deref().unwrap_or("unknown");
    let tool_input = input.tool_input.as_ref();
    let tool_response = input.tool_response.as_ref();

    // Classify tool -> ObservationType
    let obs_type = classify_tool(tool_name);

    // Generate summary from tool input
    let summary = generate_summary(tool_name, tool_input);

    // Dedup check
    let dedup = DedupCache::new(cwd);
    if dedup.is_duplicate(tool_name, &summary) {
        return Ok(HookOutput {
            continue_execution: Some(true),
            ..Default::default()
        });
    }

    // Truncate tool response content
    let content = tool_response
        .map(extract_text)
        .filter(|s| !s.is_empty())
        .map(|s| head_tail_truncate(&s, MAX_TOKENS));

    let mind = bootstrap::open_mind(input, cwd)?;

    // Store observation under cross-process lock
    mind.with_lock(|m| {
        m.remember(obs_type, tool_name, &summary, content.as_deref(), None)?;
        Ok(())
    })?;

    // Record in dedup cache (best-effort)
    let _ = dedup.record(tool_name, &summary);

    Ok(HookOutput {
        continue_execution: Some(true),
        ..Default::default()
    })
}

/// Classify a tool name into an `ObservationType` per data-model.md mapping.
fn classify_tool(tool_name: &str) -> ObservationType {
    match tool_name {
        "Edit" | "Write" | "NotebookEdit" => ObservationType::Feature,
        "Read" | "Bash" | "Grep" | "Glob" | "WebFetch" | "WebSearch" | "NotebookRead" => {
            ObservationType::Discovery
        }
        _ => ObservationType::Discovery,
    }
}

/// Generate a summary string from tool name and input.
fn generate_summary(tool_name: &str, tool_input: Option<&serde_json::Value>) -> String {
    let input = tool_input.unwrap_or(&serde_json::Value::Null);

    match tool_name {
        "Read" => {
            let path = input
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            format!("Read {path}")
        }
        "Edit" => {
            let path = input
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            format!("Edited {path}")
        }
        "Write" => {
            let path = input
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            format!("Wrote {path}")
        }
        "Bash" => {
            let cmd = input
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let truncated = match cmd.char_indices().nth(80) {
                Some((idx, _)) => &cmd[..idx],
                None => cmd,
            };
            format!("Ran command: {truncated}")
        }
        "Grep" => {
            let pattern = input
                .get("pattern")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            format!("Searched for {pattern}")
        }
        "Glob" => {
            let pattern = input
                .get("pattern")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            format!("Searched files: {pattern}")
        }
        "WebFetch" => {
            let url = input
                .get("url")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            format!("Fetched {url}")
        }
        "WebSearch" => {
            let query = input
                .get("query")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            format!("Searched web: {query}")
        }
        _ => format!("Used {tool_name}"),
    }
}

/// Extract text content from a JSON value.
fn extract_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        _ => serde_json::to_string(value).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_input(cwd: &str) -> HookInput {
        serde_json::from_value(serde_json::json!({
            "session_id": "test-session",
            "transcript_path": "/tmp/transcript.jsonl",
            "cwd": cwd,
            "permission_mode": "default",
            "hook_event_name": "PostToolUse",
            "tool_name": "Read",
            "tool_input": {"file_path": "/src/main.rs"},
            "tool_response": "file contents here",
            "tool_use_id": "toolu_01"
        }))
        .expect("valid HookInput JSON")
    }

    // -----------------------------------------------------------------------
    // classify_tool
    // -----------------------------------------------------------------------

    #[test]
    fn classify_tool_edit_returns_feature() {
        assert_eq!(classify_tool("Edit"), ObservationType::Feature);
    }

    #[test]
    fn classify_tool_write_returns_feature() {
        assert_eq!(classify_tool("Write"), ObservationType::Feature);
    }

    #[test]
    fn classify_tool_notebook_edit_returns_feature() {
        assert_eq!(classify_tool("NotebookEdit"), ObservationType::Feature);
    }

    #[test]
    fn classify_tool_read_returns_discovery() {
        assert_eq!(classify_tool("Read"), ObservationType::Discovery);
    }

    #[test]
    fn classify_tool_bash_returns_discovery() {
        assert_eq!(classify_tool("Bash"), ObservationType::Discovery);
    }

    #[test]
    fn classify_tool_grep_returns_discovery() {
        assert_eq!(classify_tool("Grep"), ObservationType::Discovery);
    }

    #[test]
    fn classify_tool_unknown_returns_discovery() {
        assert_eq!(classify_tool("SomeFutureTool"), ObservationType::Discovery);
    }

    // -----------------------------------------------------------------------
    // generate_summary
    // -----------------------------------------------------------------------

    #[test]
    fn generate_summary_read_extracts_file_path() {
        let input = serde_json::json!({"file_path": "/src/lib.rs"});
        let summary = generate_summary("Read", Some(&input));
        assert_eq!(summary, "Read /src/lib.rs");
    }

    #[test]
    fn generate_summary_edit_extracts_file_path() {
        let input = serde_json::json!({"file_path": "/src/main.rs"});
        let summary = generate_summary("Edit", Some(&input));
        assert_eq!(summary, "Edited /src/main.rs");
    }

    #[test]
    fn generate_summary_write_extracts_file_path() {
        let input = serde_json::json!({"file_path": "/tmp/output.txt"});
        let summary = generate_summary("Write", Some(&input));
        assert_eq!(summary, "Wrote /tmp/output.txt");
    }

    #[test]
    fn generate_summary_bash_extracts_command() {
        let input = serde_json::json!({"command": "cargo test"});
        let summary = generate_summary("Bash", Some(&input));
        assert_eq!(summary, "Ran command: cargo test");
    }

    #[test]
    fn generate_summary_bash_truncates_long_command() {
        let long_cmd = "a".repeat(200);
        let input = serde_json::json!({"command": long_cmd});
        let summary = generate_summary("Bash", Some(&input));
        assert!(summary.len() < 200, "summary should truncate long commands");
        assert!(summary.starts_with("Ran command: "));
    }

    #[test]
    fn generate_summary_grep_extracts_pattern() {
        let input = serde_json::json!({"pattern": "fn main"});
        let summary = generate_summary("Grep", Some(&input));
        assert_eq!(summary, "Searched for fn main");
    }

    #[test]
    fn generate_summary_glob_extracts_pattern() {
        let input = serde_json::json!({"pattern": "**/*.rs"});
        let summary = generate_summary("Glob", Some(&input));
        assert_eq!(summary, "Searched files: **/*.rs");
    }

    #[test]
    fn generate_summary_web_fetch_extracts_url() {
        let input = serde_json::json!({"url": "https://example.com"});
        let summary = generate_summary("WebFetch", Some(&input));
        assert_eq!(summary, "Fetched https://example.com");
    }

    #[test]
    fn generate_summary_web_search_extracts_query() {
        let input = serde_json::json!({"query": "rust async"});
        let summary = generate_summary("WebSearch", Some(&input));
        assert_eq!(summary, "Searched web: rust async");
    }

    #[test]
    fn generate_summary_unknown_tool_uses_tool_name() {
        let summary = generate_summary("CustomTool", None);
        assert_eq!(summary, "Used CustomTool");
    }

    #[test]
    fn generate_summary_with_none_input_uses_unknown() {
        let summary = generate_summary("Read", None);
        assert_eq!(summary, "Read unknown");
    }

    // -----------------------------------------------------------------------
    // extract_text
    // -----------------------------------------------------------------------

    #[test]
    fn extract_text_returns_string_value_directly() {
        let value = serde_json::json!("hello world");
        assert_eq!(extract_text(&value), "hello world");
    }

    #[test]
    fn extract_text_serializes_non_string_values() {
        let value = serde_json::json!({"key": "value"});
        let text = extract_text(&value);
        assert!(text.contains("key"));
        assert!(text.contains("value"));
    }

    #[test]
    fn extract_text_handles_null() {
        let value = serde_json::Value::Null;
        let text = extract_text(&value);
        assert_eq!(text, "null");
    }

    // -----------------------------------------------------------------------
    // handle_post_tool_use — requires Mind, so #[ignore]
    // -----------------------------------------------------------------------

    #[test]
    #[ignore = "requires memvid runtime (Mind::open needs valid .mv2 file)"]
    fn handle_post_tool_use_returns_continue_true() {
        let tmp = tempfile::tempdir().expect("failed to create temp dir");
        let input = make_input(tmp.path().to_str().unwrap());
        let result = handle_post_tool_use(&input);
        assert!(result.is_ok());
        let output = result.unwrap();
        assert_eq!(output.continue_execution, Some(true));
    }
}
