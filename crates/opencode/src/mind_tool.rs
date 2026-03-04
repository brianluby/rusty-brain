//! Native mind tool handler with 5 modes (US3).
//!
//! Dispatches to `Mind::search`, `ask`, `timeline`, `stats`, or `remember`
//! based on the `mode` field of `MindToolInput`.

use std::path::Path;

use rusty_brain_core::mind::Mind;
use types::{MindConfig, ObservationType, RustyBrainError};

use crate::types::{MindToolInput, MindToolOutput, VALID_MODES};

/// Process a mind tool invocation from `OpenCode`.
///
/// Validates mode against `VALID_MODES` whitelist (SEC-8), then dispatches
/// to the appropriate `Mind` API method.
///
/// # Errors
///
/// Returns `RustyBrainError` if memory path resolution, Mind opening,
/// or the dispatched operation fails.
pub fn handle_mind_tool(
    input: &MindToolInput,
    cwd: &Path,
) -> Result<MindToolOutput, RustyBrainError> {
    // SEC-8: Validate mode against whitelist
    if !VALID_MODES.contains(&input.mode.as_str()) {
        return Ok(MindToolOutput::error_with_code(
            types::error_codes::E_INPUT_INVALID_FORMAT,
            format!(
                "invalid mode '{}'; valid modes: {}",
                input.mode,
                VALID_MODES.join(", ")
            ),
        ));
    }

    match input.mode.as_str() {
        "search" => handle_search(input, cwd),
        "ask" => handle_ask(input, cwd),
        "recent" => handle_recent(input, cwd),
        "stats" => handle_stats(cwd),
        "remember" => handle_remember(input, cwd),
        _ => unreachable!("mode validated above"),
    }
}

fn open_mind_read_only(cwd: &Path) -> Result<Mind, RustyBrainError> {
    let resolved = platforms::resolve_memory_path(cwd, "opencode", false)?;
    let path = resolved.path;

    let config = {
        let mut c = MindConfig::from_env()?;
        c.memory_path.clone_from(&path);
        c
    };

    // Try read-only first; if file doesn't exist, open read-write to auto-create
    Mind::open_read_only(config).or_else(|e| {
        if path.exists() {
            Err(e)
        } else {
            let mut config2 = MindConfig::from_env()?;
            config2.memory_path = path;
            Mind::open(config2)
        }
    })
}

fn open_mind_read_write(cwd: &Path) -> Result<Mind, RustyBrainError> {
    let resolved = platforms::resolve_memory_path(cwd, "opencode", false)?;
    let mut config = MindConfig::from_env()?;
    config.memory_path = resolved.path;
    Mind::open(config)
}

fn handle_search(input: &MindToolInput, cwd: &Path) -> Result<MindToolOutput, RustyBrainError> {
    let query = match &input.query {
        Some(q) if !q.trim().is_empty() => q,
        _ => {
            return Ok(MindToolOutput::error_with_code(
                types::error_codes::E_INPUT_EMPTY_FIELD,
                "query is required for search mode",
            ));
        }
    };

    let limit = input.limit.unwrap_or(10);
    let mind = open_mind_read_only(cwd)?;
    let results = mind.with_lock(|m: &Mind| m.search(query, Some(limit)))?;

    let data: Vec<serde_json::Value> = results
        .iter()
        .map(|r| {
            serde_json::json!({
                "obs_type": r.obs_type.to_string(),
                "summary": r.summary,
                "content_excerpt": r.content_excerpt,
                "timestamp": r.timestamp.to_rfc3339(),
                "score": r.score,
                "tool_name": r.tool_name,
            })
        })
        .collect();

    Ok(MindToolOutput::success(serde_json::json!(data)))
}

fn handle_ask(input: &MindToolInput, cwd: &Path) -> Result<MindToolOutput, RustyBrainError> {
    let question = match &input.query {
        Some(q) if !q.trim().is_empty() => q,
        _ => {
            return Ok(MindToolOutput::error_with_code(
                types::error_codes::E_INPUT_EMPTY_FIELD,
                "query is required for ask mode",
            ));
        }
    };

    let mind = open_mind_read_only(cwd)?;
    let answer = mind.with_lock(|m: &Mind| m.ask(question))?;

    Ok(MindToolOutput::success(serde_json::json!({
        "answer": answer,
    })))
}

fn handle_recent(input: &MindToolInput, cwd: &Path) -> Result<MindToolOutput, RustyBrainError> {
    let limit = input.limit.unwrap_or(10);
    let mind = open_mind_read_only(cwd)?;
    let entries = mind.with_lock(|m: &Mind| m.timeline(limit, true))?;

    let data: Vec<serde_json::Value> = entries
        .iter()
        .map(|e| {
            serde_json::json!({
                "obs_type": e.obs_type.to_string(),
                "summary": e.summary,
                "timestamp": e.timestamp.to_rfc3339(),
                "tool_name": e.tool_name,
            })
        })
        .collect();

    Ok(MindToolOutput::success(serde_json::json!(data)))
}

fn handle_stats(cwd: &Path) -> Result<MindToolOutput, RustyBrainError> {
    let mind = open_mind_read_only(cwd)?;
    let stats = mind.with_lock(|m: &Mind| m.stats())?;

    let type_breakdown: serde_json::Value = stats
        .type_counts
        .iter()
        .map(|(k, v)| (k.to_string(), serde_json::json!(v)))
        .collect::<serde_json::Map<String, serde_json::Value>>()
        .into();

    let date_range = serde_json::json!({
        "oldest": stats.oldest_memory.map(|t| t.to_rfc3339()),
        "newest": stats.newest_memory.map(|t| t.to_rfc3339()),
    });

    Ok(MindToolOutput::success(serde_json::json!({
        "total_observations": stats.total_observations,
        "total_sessions": stats.total_sessions,
        "date_range": date_range,
        "file_size_bytes": stats.file_size_bytes,
        "type_breakdown": type_breakdown,
    })))
}

fn handle_remember(input: &MindToolInput, cwd: &Path) -> Result<MindToolOutput, RustyBrainError> {
    let content = match &input.content {
        Some(c) if !c.trim().is_empty() => c,
        _ => {
            return Ok(MindToolOutput::error_with_code(
                types::error_codes::E_INPUT_EMPTY_FIELD,
                "content is required for remember mode",
            ));
        }
    };

    let mind = open_mind_read_write(cwd)?;
    let id = mind.with_lock(|m: &Mind| {
        m.remember(ObservationType::Discovery, "user", content, None, None)
    })?;

    Ok(MindToolOutput::success(serde_json::json!({
        "observation_id": id,
    })))
}
