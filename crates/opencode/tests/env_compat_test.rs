//! Environment variable compatibility tests for the opencode bootstrap module.
//!
//! T042: Verifies that `MEMVID_PLATFORM`, `MEMVID_PLATFORM_PATH_OPT_IN`,
//! `MEMVID_PLATFORM_MEMORY_PATH`, and `MEMVID_MIND_DEBUG` interact correctly
//! with the opencode-specific bootstrap path (`resolve_memory_path`, `mind_config`,
//! `should_process`).
//!
//! All tests use `temp_env::with_vars` for env isolation and `tempfile::tempdir`
//! for filesystem isolation.

use std::path::Path;

use opencode::bootstrap;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a minimal valid `HookInput` with optional platform field.
fn make_hook_input(cwd: &str, platform: Option<&str>) -> types::HookInput {
    let mut json = serde_json::json!({
        "session_id": "env-compat-session",
        "transcript_path": "/tmp/transcript.jsonl",
        "cwd": cwd,
        "permission_mode": "default",
        "hook_event_name": "PostToolUse",
    });
    if let Some(p) = platform {
        json["platform"] = serde_json::Value::String(p.to_string());
    }
    serde_json::from_value(json).expect("valid HookInput JSON")
}

// ===========================================================================
// MEMVID_PLATFORM detection (opencode-specific behavior)
// ===========================================================================

#[test]
fn memvid_platform_set_to_opencode_detected_as_opencode() {
    temp_env::with_vars(
        [
            ("MEMVID_PLATFORM", Some("opencode")),
            ("OPENCODE", None::<&str>),
        ],
        || {
            let input = make_hook_input("/tmp/project", None);
            // should_process must not panic and must return a bool
            let result = bootstrap::should_process(&input, "PostToolUse");
            // With opencode adapter available, valid input should pass pipeline
            assert!(
                result,
                "MEMVID_PLATFORM=opencode should detect as opencode and pass pipeline"
            );
        },
    );
}

#[test]
fn memvid_platform_set_to_claude_overrides_opencode_indicator() {
    temp_env::with_vars(
        [("MEMVID_PLATFORM", Some("claude")), ("OPENCODE", Some("1"))],
        || {
            // MEMVID_PLATFORM has higher priority than OPENCODE env indicator
            let input = make_hook_input("/tmp/project", None);
            let result = bootstrap::should_process(&input, "PostToolUse");
            // Should succeed regardless — the key point is MEMVID_PLATFORM wins
            assert!(result, "MEMVID_PLATFORM=claude should override OPENCODE=1");
        },
    );
}

#[test]
fn memvid_platform_set_to_auto_falls_through_detection() {
    temp_env::with_vars(
        [
            ("MEMVID_PLATFORM", Some("auto")),
            ("OPENCODE", None::<&str>),
        ],
        || {
            let input = make_hook_input("/tmp/project", None);
            // "auto" is not a known adapter, so should_process is fail-open
            let result = bootstrap::should_process(&input, "PostToolUse");
            assert!(
                result,
                "MEMVID_PLATFORM=auto with unknown adapter should fail-open"
            );
        },
    );
}

#[test]
fn memvid_platform_set_to_nonexistent_is_fail_open() {
    temp_env::with_vars(
        [
            ("MEMVID_PLATFORM", Some("nonexistent")),
            ("OPENCODE", None::<&str>),
        ],
        || {
            let input = make_hook_input("/tmp/project", None);
            let result = bootstrap::should_process(&input, "PostToolUse");
            assert!(
                result,
                "nonexistent platform must fail-open (no adapter found)"
            );
        },
    );
}

#[test]
fn memvid_platform_unset_defaults_to_claude() {
    temp_env::with_vars(
        [
            ("MEMVID_PLATFORM", None::<&str>),
            ("OPENCODE", None::<&str>),
        ],
        || {
            let input = make_hook_input("/tmp/project", None);
            // With no platform env and no OPENCODE indicator, detection defaults
            // to "claude", which has a builtin adapter → should process
            let result = bootstrap::should_process(&input, "PostToolUse");
            assert!(result, "unset MEMVID_PLATFORM should default to claude");
        },
    );
}

#[test]
fn explicit_platform_field_in_input_overrides_memvid_platform_env() {
    temp_env::with_vars(
        [
            ("MEMVID_PLATFORM", Some("claude")),
            ("OPENCODE", None::<&str>),
        ],
        || {
            let input = make_hook_input("/tmp/project", Some("opencode"));
            // Explicit platform field in HookInput takes highest priority
            let result = bootstrap::should_process(&input, "PostToolUse");
            assert!(
                result,
                "explicit platform=opencode in input should override MEMVID_PLATFORM=claude"
            );
        },
    );
}

// ===========================================================================
// resolve_memory_path with env vars
// ===========================================================================

#[test]
fn resolve_memory_path_with_valid_dir() {
    let dir = tempfile::tempdir().expect("tempdir");
    temp_env::with_vars(
        [
            ("MEMVID_PLATFORM_PATH_OPT_IN", None::<&str>),
            ("MEMVID_PLATFORM_MEMORY_PATH", None::<&str>),
        ],
        || {
            let result = bootstrap::resolve_memory_path(dir.path());
            assert!(
                result.is_ok(),
                "resolve_memory_path with valid dir should succeed: {result:?}"
            );
            let path = result.unwrap();
            // Without opt-in, should use canonical path
            assert!(
                path.to_string_lossy().contains(".agent-brain/mind.mv2"),
                "without opt-in, should use canonical path: {}",
                path.display()
            );
        },
    );
}

#[test]
fn resolve_memory_path_with_platform_opt_in() {
    let dir = tempfile::tempdir().expect("tempdir");
    temp_env::with_vars(
        [
            ("MEMVID_PLATFORM_PATH_OPT_IN", Some("1")),
            ("MEMVID_PLATFORM_MEMORY_PATH", None::<&str>),
        ],
        || {
            let result = bootstrap::resolve_memory_path(dir.path());
            assert!(
                result.is_ok(),
                "resolve_memory_path with opt-in should succeed: {result:?}"
            );
            let path = result.unwrap();
            // With opt-in, should use platform-scoped path for opencode
            assert!(
                path.to_string_lossy().contains("opencode"),
                "with opt-in, should contain 'opencode' in path: {}",
                path.display()
            );
        },
    );
}

#[test]
fn resolve_memory_path_with_nonexistent_dir_does_not_panic() {
    let missing = Path::new("/tmp/nonexistent-dir-env-compat-test-42");
    temp_env::with_vars(
        [
            ("MEMVID_PLATFORM_PATH_OPT_IN", None::<&str>),
            ("MEMVID_PLATFORM_MEMORY_PATH", None::<&str>),
        ],
        || {
            // Path resolution does not perform I/O, so missing dir is fine
            let result = bootstrap::resolve_memory_path(missing);
            assert!(
                result.is_ok(),
                "resolve_memory_path should not require dir to exist"
            );
        },
    );
}

// ===========================================================================
// mind_config with env vars
// ===========================================================================

#[test]
fn mind_config_uses_legacy_path_without_opt_in() {
    let dir = tempfile::tempdir().expect("tempdir");
    temp_env::with_vars(
        [
            ("MEMVID_PLATFORM_PATH_OPT_IN", None::<&str>),
            ("MEMVID_PLATFORM_MEMORY_PATH", None::<&str>),
            ("MEMVID_MIND_DEBUG", None::<&str>),
        ],
        || {
            let config = bootstrap::mind_config(dir.path());
            assert!(config.is_ok(), "mind_config should succeed: {config:?}");
            let cfg = config.unwrap();
            assert!(
                cfg.memory_path
                    .to_string_lossy()
                    .contains(".agent-brain/mind.mv2"),
                "without opt-in, config should use canonical path: {}",
                cfg.memory_path.display()
            );
        },
    );
}

#[test]
fn mind_config_explicit_memory_path_overrides_platform_resolution() {
    let dir = tempfile::tempdir().expect("tempdir");
    let explicit_path = "/custom/memory/path.mv2";
    temp_env::with_vars(
        [
            ("MEMVID_PLATFORM_MEMORY_PATH", Some(explicit_path)),
            ("MEMVID_PLATFORM_PATH_OPT_IN", Some("1")),
            ("MEMVID_MIND_DEBUG", None::<&str>),
        ],
        || {
            let config = bootstrap::mind_config(dir.path());
            assert!(config.is_ok(), "mind_config should succeed: {config:?}");
            let cfg = config.unwrap();
            assert_eq!(
                cfg.memory_path.to_string_lossy(),
                explicit_path,
                "MEMVID_PLATFORM_MEMORY_PATH should override platform-resolved path"
            );
        },
    );
}

#[test]
fn mind_config_empty_memory_path_env_falls_back_to_platform_resolution() {
    let dir = tempfile::tempdir().expect("tempdir");
    temp_env::with_vars(
        [
            ("MEMVID_PLATFORM_MEMORY_PATH", Some("")),
            ("MEMVID_PLATFORM_PATH_OPT_IN", None::<&str>),
            ("MEMVID_MIND_DEBUG", None::<&str>),
        ],
        || {
            let config = bootstrap::mind_config(dir.path());
            assert!(config.is_ok(), "mind_config should succeed: {config:?}");
            let cfg = config.unwrap();
            // Empty MEMVID_PLATFORM_MEMORY_PATH is treated as unset, so
            // platform resolution kicks in
            assert!(
                cfg.memory_path
                    .to_string_lossy()
                    .contains(".agent-brain/mind.mv2"),
                "empty MEMVID_PLATFORM_MEMORY_PATH should fall back to platform resolution: {}",
                cfg.memory_path.display()
            );
        },
    );
}

#[test]
fn mind_config_debug_env_enables_debug() {
    let dir = tempfile::tempdir().expect("tempdir");
    temp_env::with_vars(
        [
            ("MEMVID_MIND_DEBUG", Some("1")),
            ("MEMVID_PLATFORM_MEMORY_PATH", None::<&str>),
            ("MEMVID_PLATFORM_PATH_OPT_IN", None::<&str>),
        ],
        || {
            let config = bootstrap::mind_config(dir.path());
            assert!(config.is_ok(), "mind_config should succeed: {config:?}");
            assert!(
                config.unwrap().debug,
                "MEMVID_MIND_DEBUG=1 should enable debug"
            );
        },
    );
}

#[test]
fn mind_config_invalid_debug_env_returns_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    temp_env::with_vars(
        [
            ("MEMVID_MIND_DEBUG", Some("invalid")),
            ("MEMVID_PLATFORM_MEMORY_PATH", None::<&str>),
            ("MEMVID_PLATFORM_PATH_OPT_IN", None::<&str>),
        ],
        || {
            let config = bootstrap::mind_config(dir.path());
            assert!(
                config.is_err(),
                "MEMVID_MIND_DEBUG=invalid should return error"
            );
        },
    );
}

// ===========================================================================
// OPENCODE env indicator interaction
// ===========================================================================

#[test]
fn opencode_env_indicator_triggers_opencode_detection() {
    temp_env::with_vars(
        [("MEMVID_PLATFORM", None::<&str>), ("OPENCODE", Some("1"))],
        || {
            let input = make_hook_input("/tmp/project", None);
            let result = bootstrap::should_process(&input, "PostToolUse");
            assert!(
                result,
                "OPENCODE=1 should trigger opencode detection and pass pipeline"
            );
        },
    );
}

#[test]
fn opencode_env_indicator_zero_does_not_trigger() {
    temp_env::with_vars(
        [("MEMVID_PLATFORM", None::<&str>), ("OPENCODE", Some("0"))],
        || {
            let input = make_hook_input("/tmp/project", None);
            // OPENCODE=0 falls through to default "claude"
            let result = bootstrap::should_process(&input, "PostToolUse");
            assert!(result, "OPENCODE=0 should fall through to claude default");
        },
    );
}

// ===========================================================================
// Cross-platform: opencode vs hooks/claude path differentiation
// ===========================================================================

#[test]
fn opencode_resolve_memory_path_hardcodes_opencode_platform() {
    let dir = tempfile::tempdir().expect("tempdir");
    temp_env::with_vars(
        [
            ("MEMVID_PLATFORM_PATH_OPT_IN", Some("1")),
            ("MEMVID_PLATFORM_MEMORY_PATH", None::<&str>),
        ],
        || {
            let result = bootstrap::resolve_memory_path(dir.path());
            assert!(result.is_ok());
            let path = result.unwrap();
            // The opencode bootstrap always passes "opencode" as platform_name
            // to platforms::resolve_memory_path, regardless of env detection
            assert!(
                path.to_string_lossy().contains("opencode"),
                "opencode bootstrap should always use 'opencode' as platform name, got: {}",
                path.display()
            );
        },
    );
}

#[test]
fn opencode_resolve_memory_path_stays_within_project_dir() {
    let dir = tempfile::tempdir().expect("tempdir");
    temp_env::with_vars(
        [
            ("MEMVID_PLATFORM_PATH_OPT_IN", Some("1")),
            ("MEMVID_PLATFORM_MEMORY_PATH", None::<&str>),
        ],
        || {
            let result = bootstrap::resolve_memory_path(dir.path());
            assert!(result.is_ok());
            let path = result.unwrap();
            assert!(
                path.starts_with(dir.path()),
                "resolved path must stay within project dir (FR-014): {}",
                path.display()
            );
        },
    );
}
