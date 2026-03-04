use std::path::Path;

use crate::dedup::DedupCache;
use crate::error::HookError;
use crate::truncate::head_tail_truncate;
use types::hooks::{HookInput, HookOutput};
use types::{MindConfig, ObservationType};

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
pub fn handle_post_tool_use(input: &HookInput) -> Result<HookOutput, HookError> {
    let cwd = Path::new(&input.cwd);

    let tool_name = input.tool_name.as_deref().unwrap_or("unknown");
    let tool_input = input.tool_input.as_ref();
    let tool_response = input.tool_response.as_ref();

    // Classify tool → ObservationType
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

    // Resolve memory path and open Mind
    let platform_name = platforms::detect_platform(input);
    let resolved = platforms::resolve_memory_path(cwd, &platform_name, false).map_err(|e| {
        HookError::Platform {
            message: format!("Failed to resolve memory path: {e}"),
        }
    })?;

    let config = MindConfig {
        memory_path: resolved.path.clone(),
        ..MindConfig::default()
    };

    let mind = rusty_brain_core::mind::Mind::open(config)?;

    // Store observation
    mind.remember(obs_type, tool_name, &summary, content.as_deref(), None)?;

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
