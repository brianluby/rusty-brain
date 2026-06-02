//! The four capture flows: SessionStart (inject), PostToolUse (capture mutating
//! tools, deduped), Stop (session summary + git-modified files), PreCompact
//! (capture decisions). Every flow returns a `HookResult` with
//! `continue_execution: true`; nothing ever blocks.

use rb_agents::DaemonClient;
use rb_agents::HookResult;
use rb_types::MemoryType;

use crate::dedup::DedupCache;

/// Max characters retained from a tool response before head/tail truncation.
const MAX_RESPONSE_CHARS: usize = 2000;

const TRUNCATION_MARKER: &str = "[...truncated...]";

/// A `HookResult` that injects no message and always continues.
fn continue_only() -> HookResult {
    HookResult {
        system_message: None,
        continue_execution: true,
    }
}

/// Normalize a CLI-reported tool name to its canonical capitalized form.
///
/// Claude reports capitalized names (`Edit`, `Write`, `NotebookEdit`, `Bash`);
/// OpenCode reports lowercase (`edit`, `write`, `bash`, `patch`). Lowercasing
/// first lets us match both. OpenCode's `patch` is treated as an `Edit`.
/// Anything not a recognized mutation tool maps to `""` (the empty canonical),
/// which `is_mutation_tool` reads as "not captured".
fn normalize_tool(tool_name: &str) -> &'static str {
    match tool_name.to_lowercase().as_str() {
        "edit" => "Edit",
        "write" => "Write",
        "notebookedit" => "NotebookEdit",
        "bash" => "Bash",
        "patch" => "Edit",
        _ => "",
    }
}

/// True if `tool_name` is a captured mutation tool (case-insensitive across CLIs).
fn is_mutation_tool(tool_name: &str) -> bool {
    !normalize_tool(tool_name).is_empty()
}

/// Map a tool name to a `MemoryType`: file-mutation tools are code patterns;
/// everything else (Bash, unknown) is a reference observation. Normalizes first
/// so lowercase (OpenCode) names classify identically to capitalized ones.
fn classify_tool(tool_name: &str) -> MemoryType {
    match normalize_tool(tool_name) {
        "Edit" | "Write" | "NotebookEdit" => MemoryType::CodePattern,
        _ => MemoryType::Reference,
    }
}

/// Head/tail truncate to roughly `max_chars`, inserting a marker. UTF-8 safe:
/// boundaries are taken on `char_indices`, never raw byte offsets.
fn truncate_head_tail(content: &str, max_chars: usize) -> String {
    let char_count = content.chars().count();
    if char_count <= max_chars {
        return content.to_string();
    }
    let marker_len = TRUNCATION_MARKER.chars().count();
    let budget = max_chars.saturating_sub(marker_len);
    let head_chars = budget * 60 / 100;
    let tail_chars = budget.saturating_sub(head_chars);
    let head_end = content
        .char_indices()
        .nth(head_chars)
        .map_or(content.len(), |(idx, _)| idx);
    let tail_start = content
        .char_indices()
        .nth(char_count.saturating_sub(tail_chars))
        .map_or(content.len(), |(idx, _)| idx);
    let head = &content[..head_end];
    let tail = &content[tail_start..];
    format!("{head}{TRUNCATION_MARKER}{tail}")
}

/// Pull a string field from a JSON object, defaulting to `"unknown"`.
fn str_field<'a>(input: &'a serde_json::Value, key: &str) -> &'a str {
    input.get(key).and_then(|v| v.as_str()).unwrap_or("unknown")
}

/// Build a concise, human-readable summary of a tool invocation. Normalizes the
/// tool name first so lowercase (OpenCode) `write`/`edit`/`bash`/`patch` produce
/// the same summaries as Claude's capitalized names.
fn summarize_post_tool_use(tool_name: &str, tool_input: &serde_json::Value) -> String {
    match normalize_tool(tool_name) {
        "Edit" => format!("Edited {}", str_field(tool_input, "file_path")),
        "Write" => format!("Wrote {}", str_field(tool_input, "file_path")),
        "NotebookEdit" => format!("Edited notebook {}", str_field(tool_input, "notebook_path")),
        "Bash" => {
            let cmd = str_field(tool_input, "command");
            let truncated = match cmd.char_indices().nth(80) {
                Some((idx, _)) => &cmd[..idx],
                None => cmd,
            };
            format!("Ran command: {truncated}")
        }
        // Unknown tool: normalize yields "", so report the raw name for diagnostics.
        _ => format!("Used {tool_name}"),
    }
}

/// Extract text from a tool response value (string used directly; else JSON).
fn extract_response_text(response: &serde_json::Value) -> String {
    match response {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(s) => s.clone(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

/// PostToolUse capture flow. No-op (continue) for non-mutation tools or
/// deduplicated observations; otherwise builds a summary + truncated context and
/// calls `DaemonClient::remember`. Always returns `continue_execution: true`.
pub async fn post_tool_use(
    client: Option<&mut DaemonClient>,
    dedup: &DedupCache,
    tool_name: &str,
    tool_input: &serde_json::Value,
    tool_response: &serde_json::Value,
) -> HookResult {
    if !is_mutation_tool(tool_name) {
        return continue_only();
    }
    let summary = summarize_post_tool_use(tool_name, tool_input);
    if dedup.is_duplicate(tool_name, &summary) {
        return continue_only();
    }

    let memory_type = classify_tool(tool_name);
    let raw = extract_response_text(tool_response);
    let context = if raw.trim().is_empty() {
        None
    } else {
        Some(truncate_head_tail(&raw, MAX_RESPONSE_CHARS))
    };

    if let Some(client) = client {
        let _ = client
            .remember(
                summary.clone(),
                context,
                memory_type,
                5,
                vec!["hook".to_string(), "post-tool-use".to_string()],
            )
            .await;
    }
    // Record AFTER a (best-effort) store so a failed connect does not poison the
    // dedup window — but record regardless of remember outcome to bound retries.
    dedup.record(tool_name, &summary);
    continue_only()
}

/// Pure: format recent + important memories into a markdown system message.
/// `important` is split into critical (`importance >= 8`) and important
/// (`importance == 7`). Returns a header-only message when everything is empty.
fn format_session_start(
    recent: &[rb_types::MemoryNote],
    important: &[rb_types::MemoryNote],
    total: usize,
) -> String {
    let mut out = String::new();
    out.push_str("# Rusty Brain — Memory Active\n");
    out.push_str(&format!("Total memories in scope: {total}\n"));

    let critical: Vec<&rb_types::MemoryNote> =
        important.iter().filter(|m| m.importance >= 8).collect();
    let merely_important: Vec<&rb_types::MemoryNote> =
        important.iter().filter(|m| m.importance == 7).collect();

    if !critical.is_empty() {
        out.push_str("\n## Critical\n");
        for m in critical {
            out.push_str(&format!("- {}\n", memory_line(m)));
        }
    }
    if !merely_important.is_empty() {
        out.push_str("\n## Important\n");
        for m in merely_important {
            out.push_str(&format!("- {}\n", memory_line(m)));
        }
    }
    if !recent.is_empty() {
        out.push_str("\n## Recent\n");
        for m in recent {
            out.push_str(&format!("- {}\n", memory_line(m)));
        }
    }
    out
}

/// One-line rendering of a memory: prefer its summary, else its content.
fn memory_line(memory: &rb_types::MemoryNote) -> String {
    let text = if memory.summary.trim().is_empty() {
        memory.content.as_str()
    } else {
        memory.summary.as_str()
    };
    format!("[{}] {}", memory.memory_type.as_str(), text.trim())
}

/// SessionStart flow: fetch context and inject a markdown system message.
/// Always continues. With no client (degraded), continues with no message.
pub async fn session_start(client: Option<&mut DaemonClient>) -> HookResult {
    let Some(client) = client else {
        return continue_only();
    };
    match client.context().await {
        Some((recent, important, total)) => {
            let message = format_session_start(&recent, &important, total);
            HookResult {
                system_message: Some(message),
                continue_execution: true,
            }
        }
        None => continue_only(),
    }
}

/// Detect working-tree-modified files via `git diff --name-only HEAD`. Fail-open:
/// returns an empty vec on any failure (git missing, not a repo, non-zero exit,
/// or a hung git killed by the bound). Arguments are hardcoded literals (no
/// shell, no user interpolation).
///
/// Runs the blocking git call via `spawn_blocking` so it leaves the single
/// current-thread runtime free — otherwise a slow git would block the runtime
/// thread and the harness `OVERALL_TIMEOUT` could never fire. `run_git_bounded`
/// additionally kills a git that hangs past its own 2s bound.
async fn git_modified_files(cwd: &std::path::Path) -> Vec<String> {
    let cwd = cwd.to_path_buf();
    let bytes = tokio::task::spawn_blocking(move || {
        rb_agents::run_git_bounded(
            &cwd,
            &["diff", "--name-only", "HEAD"],
            std::time::Duration::from_secs(2),
        )
    })
    .await
    .ok()
    .flatten();
    let Some(bytes) = bytes else {
        return Vec::new();
    };
    let Ok(text) = String::from_utf8(bytes) else {
        return Vec::new();
    };
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

/// Build the Stop session-summary text from the modified-file list.
fn format_stop_summary(modified: &[String]) -> String {
    if modified.is_empty() {
        "Session ended with no file modifications.".to_string()
    } else {
        format!(
            "Session ended. Modified {} file(s): {}",
            modified.len(),
            modified.join(", ")
        )
    }
}

/// Stop flow: record a session summary memory (including git-modified files).
/// Always continues. With no client (degraded), continues with no store.
pub async fn stop(client: Option<&mut DaemonClient>, cwd: &std::path::Path) -> HookResult {
    let modified = git_modified_files(cwd).await;
    let summary = format_stop_summary(&modified);
    if let Some(client) = client {
        let _ = client
            .remember(
                summary,
                None,
                MemoryType::Reference,
                4,
                vec!["hook".to_string(), "session-summary".to_string()],
            )
            .await;
    }
    continue_only()
}

/// Decision marker substrings (lowercased match) used to detect that compaction
/// is about to drop a recorded decision worth persisting.
const DECISION_MARKERS: &[&str] = &[
    "decided",
    "decision",
    "chosen",
    "we will use",
    "approach is",
];

/// True if `text` contains any decision marker (case-insensitive).
fn has_decision_marker(text: &str) -> bool {
    let lower = text.to_lowercase();
    DECISION_MARKERS.iter().any(|m| lower.contains(m))
}

/// PreCompact flow: if the custom instructions reference a decision, capture it
/// as a high-importance architecture decision. Always continues.
pub async fn pre_compact(
    client: Option<&mut DaemonClient>,
    custom_instructions: Option<&str>,
) -> HookResult {
    let Some(text) = custom_instructions else {
        return continue_only();
    };
    if !has_decision_marker(text) {
        return continue_only();
    }
    if let Some(client) = client {
        let _ = client
            .remember(
                format!("Pre-compaction decision snapshot: {}", text.trim()),
                None,
                MemoryType::ArchitectureDecision,
                8,
                vec!["hook".to_string(), "pre-compact".to_string()],
            )
            .await;
    }
    continue_only()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn mutation_tools_are_recognized() {
        for t in ["Edit", "Write", "NotebookEdit", "Bash"] {
            assert!(is_mutation_tool(t), "{t} should be a mutation tool");
        }
    }

    #[test]
    fn discovery_tools_are_not_mutation() {
        for t in ["Read", "Grep", "Glob", "WebFetch", "WebSearch", ""] {
            assert!(!is_mutation_tool(t), "{t} should not be a mutation tool");
        }
    }

    #[test]
    fn classify_file_tools_to_code_pattern() {
        assert_eq!(classify_tool("Edit"), MemoryType::CodePattern);
        assert_eq!(classify_tool("Write"), MemoryType::CodePattern);
        assert_eq!(classify_tool("NotebookEdit"), MemoryType::CodePattern);
    }

    #[test]
    fn classify_bash_to_reference() {
        assert_eq!(classify_tool("Bash"), MemoryType::Reference);
    }

    #[test]
    fn classify_unknown_defaults_to_reference() {
        assert_eq!(classify_tool("SomeFutureTool"), MemoryType::Reference);
    }

    #[test]
    fn lowercase_opencode_tools_are_mutations() {
        // OpenCode reports lowercase tool names; they must be recognized.
        for t in ["write", "edit", "bash", "patch"] {
            assert!(is_mutation_tool(t), "{t} (opencode) should be a mutation");
        }
    }

    #[test]
    fn lowercase_opencode_tools_classify_like_capitalized() {
        assert_eq!(classify_tool("write"), MemoryType::CodePattern);
        assert_eq!(classify_tool("edit"), MemoryType::CodePattern);
        assert_eq!(classify_tool("patch"), MemoryType::CodePattern);
        assert_eq!(classify_tool("bash"), MemoryType::Reference);
    }

    #[test]
    fn lowercase_write_summary_matches_capitalized() {
        let input = serde_json::json!({"file_path": "/src/lib.rs"});
        // OpenCode lowercase `write` yields the same "Wrote ..." summary as Claude.
        assert_eq!(
            summarize_post_tool_use("write", &input),
            "Wrote /src/lib.rs"
        );
    }

    #[tokio::test]
    async fn opencode_lowercase_write_is_captured_end_to_end() {
        // Feed the REAL OpenCode lowercase `write` through the capture flow and
        // prove it is captured (recorded in dedup) rather than silently dropped.
        let tmp = tempfile::tempdir().unwrap();
        let dedup = DedupCache::at(tmp.path().join("d.json"));
        let result = post_tool_use(
            None,
            &dedup,
            "write",
            &serde_json::json!({"file_path": "/src/main.rs"}),
            &serde_json::json!("ok"),
        )
        .await;
        assert!(result.continue_execution);
        // Captured => the canonical summary was recorded in the dedup window.
        assert!(
            dedup.is_duplicate("write", "Wrote /src/main.rs"),
            "lowercase opencode write must be captured (recorded), not dropped"
        );
    }

    #[test]
    fn truncate_passes_through_short_content() {
        let s = "short content";
        assert_eq!(truncate_head_tail(s, 100), s);
    }

    #[test]
    fn truncate_inserts_marker_for_long_content() {
        let s = "a".repeat(5000);
        let out = truncate_head_tail(&s, 100);
        assert!(out.len() < s.len());
        assert!(out.contains("[...truncated...]"));
    }

    #[test]
    fn truncate_is_utf8_safe() {
        let s = "é".repeat(5000);
        let out = truncate_head_tail(&s, 100);
        // Must remain valid UTF-8 (no panic on multibyte boundaries).
        assert!(out.contains("[...truncated...]"));
    }

    #[test]
    fn summarize_edit_includes_tool_and_path() {
        let input = serde_json::json!({"file_path": "/src/main.rs"});
        let summary = summarize_post_tool_use("Edit", &input);
        assert_eq!(summary, "Edited /src/main.rs");
    }

    #[test]
    fn summarize_write_includes_path() {
        let input = serde_json::json!({"file_path": "/src/lib.rs"});
        let summary = summarize_post_tool_use("Write", &input);
        assert_eq!(summary, "Wrote /src/lib.rs");
    }

    #[test]
    fn summarize_bash_includes_truncated_command() {
        let input = serde_json::json!({"command": "cargo test --workspace"});
        let summary = summarize_post_tool_use("Bash", &input);
        assert_eq!(summary, "Ran command: cargo test --workspace");
    }

    #[test]
    fn summarize_bash_truncates_long_command() {
        let input = serde_json::json!({"command": "x".repeat(200)});
        let summary = summarize_post_tool_use("Bash", &input);
        assert!(summary.starts_with("Ran command: "));
        assert!(summary.len() < 200);
    }

    #[test]
    fn summarize_missing_field_uses_unknown() {
        let input = serde_json::json!({});
        assert_eq!(summarize_post_tool_use("Edit", &input), "Edited unknown");
    }

    #[test]
    fn summarize_notebook_edit_uses_path() {
        let input = serde_json::json!({"notebook_path": "/nb.ipynb"});
        let summary = summarize_post_tool_use("NotebookEdit", &input);
        assert_eq!(summary, "Edited notebook /nb.ipynb");
    }

    #[test]
    fn extract_response_text_handles_variants() {
        assert_eq!(extract_response_text(&serde_json::Value::Null), "");
        assert_eq!(extract_response_text(&serde_json::json!("hello")), "hello");
        let obj = extract_response_text(&serde_json::json!({"k": "v"}));
        assert!(obj.contains("\"k\""));
    }

    #[tokio::test]
    async fn post_tool_use_non_mutation_is_noop_continue() {
        let tmp = tempfile::tempdir().unwrap();
        let dedup = DedupCache::at(tmp.path().join("d.json"));
        let result = post_tool_use(
            None,
            &dedup,
            "Read",
            &serde_json::json!({"file_path": "/x"}),
            &serde_json::json!("contents"),
        )
        .await;
        assert!(result.continue_execution);
        assert!(result.system_message.is_none());
        // Non-mutation must not poison the dedup cache.
        assert!(!dedup.is_duplicate("Read", "Read /x"));
    }

    #[tokio::test]
    async fn post_tool_use_records_dedup_for_mutation() {
        let tmp = tempfile::tempdir().unwrap();
        let dedup = DedupCache::at(tmp.path().join("d.json"));
        let result = post_tool_use(
            None,
            &dedup,
            "Edit",
            &serde_json::json!({"file_path": "/src/main.rs"}),
            &serde_json::json!("ok"),
        )
        .await;
        assert!(result.continue_execution);
        assert!(dedup.is_duplicate("Edit", "Edited /src/main.rs"));
    }

    fn sample_note(content: &str, importance: u8) -> rb_types::MemoryNote {
        rb_types::MemoryNote::new(
            rb_types::Namespace::Project("rusty-brain".into()),
            content.to_string(),
            MemoryType::Insight,
            importance,
        )
    }

    #[test]
    fn format_session_start_empty_has_header_and_no_sections() {
        let msg = format_session_start(&[], &[], 0);
        assert!(msg.contains("# Rusty Brain"));
        assert!(!msg.contains("## Critical"));
        assert!(!msg.contains("## Recent"));
    }

    #[test]
    fn format_session_start_splits_critical_and_important() {
        let important = vec![sample_note("crit decision", 9), sample_note("imp note", 7)];
        let msg = format_session_start(&[], &important, 2);
        assert!(msg.contains("## Critical"));
        assert!(msg.contains("crit decision"));
        assert!(msg.contains("## Important"));
        assert!(msg.contains("imp note"));
    }

    #[test]
    fn format_session_start_lists_recent_and_total() {
        let recent = vec![sample_note("did a thing", 5)];
        let msg = format_session_start(&recent, &[], 12);
        assert!(msg.contains("## Recent"));
        assert!(msg.contains("did a thing"));
        assert!(msg.contains("12"), "should mention the total count");
    }

    #[tokio::test]
    async fn session_start_without_client_continues_with_no_message() {
        let result = session_start(None).await;
        assert!(result.continue_execution);
        assert!(result.system_message.is_none());
    }

    #[tokio::test]
    async fn git_modified_files_empty_for_non_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let files = git_modified_files(tmp.path()).await;
        assert!(files.is_empty(), "non-repo must yield empty vec");
    }

    #[tokio::test]
    async fn git_modified_files_empty_for_nonexistent_dir() {
        let files = git_modified_files(std::path::Path::new("/nonexistent/path/xyz")).await;
        assert!(files.is_empty());
    }

    #[tokio::test]
    async fn git_modified_files_detects_change_in_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(tmp.path())
                .output()
        };
        if !run(&["init"]).map(|o| o.status.success()).unwrap_or(false) {
            return; // git unavailable; skip
        }
        let _ = run(&["config", "user.email", "t@t.com"]);
        let _ = run(&["config", "user.name", "T"]);
        std::fs::write(tmp.path().join("f.txt"), "initial").unwrap();
        let _ = run(&["add", "."]);
        let _ = run(&["commit", "-m", "init"]);
        std::fs::write(tmp.path().join("f.txt"), "changed").unwrap();
        let files = git_modified_files(tmp.path()).await;
        assert!(
            files.contains(&"f.txt".to_string()),
            "should detect modified file, got {files:?}"
        );
    }

    #[test]
    fn format_stop_summary_no_files() {
        let summary = format_stop_summary(&[]);
        assert!(summary.to_lowercase().contains("no file"));
    }

    #[test]
    fn format_stop_summary_lists_files() {
        let summary = format_stop_summary(&["a.rs".to_string(), "b.rs".to_string()]);
        assert!(summary.contains("2"));
        assert!(summary.contains("a.rs"));
        assert!(summary.contains("b.rs"));
    }

    #[tokio::test]
    async fn stop_without_client_continues() {
        let tmp = tempfile::tempdir().unwrap();
        let result = stop(None, tmp.path()).await;
        assert!(result.continue_execution);
    }

    #[test]
    fn decision_marker_detected_case_insensitively() {
        assert!(has_decision_marker("We DECIDED to use SQLite."));
        assert!(has_decision_marker("Decision: single writer."));
        assert!(has_decision_marker("the chosen approach is X"));
    }

    #[test]
    fn no_decision_marker_in_plain_text() {
        assert!(!has_decision_marker("just some ordinary notes"));
        assert!(!has_decision_marker(""));
    }

    #[tokio::test]
    async fn pre_compact_without_marker_is_noop_continue() {
        let result = pre_compact(None, Some("ordinary instructions")).await;
        assert!(result.continue_execution);
        assert!(result.system_message.is_none());
    }

    #[tokio::test]
    async fn pre_compact_with_marker_continues() {
        let result = pre_compact(None, Some("Decision: use one DB")).await;
        assert!(result.continue_execution);
    }
}
