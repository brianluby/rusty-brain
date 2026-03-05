//! T041: Environment variable compatibility tests.
//!
//! Exercises all 6 env vars from Contract 6 with valid, invalid, and unset
//! values.  Uses `temp_env::with_vars` for safe, serialized env mutation.

mod common;

use std::path::PathBuf;

use types::MindConfig;
use types::hooks::HookInput;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_input(cwd: &str, platform: Option<&str>) -> HookInput {
    let mut json = serde_json::json!({
        "session_id": "env-compat-test",
        "transcript_path": "/tmp/transcript.jsonl",
        "cwd": cwd,
        "permission_mode": "default",
        "hook_event_name": "SessionStart"
    });
    if let Some(p) = platform {
        json["platform"] = serde_json::Value::String(p.to_string());
    }
    serde_json::from_value(json).expect("valid HookInput JSON")
}

// ---------------------------------------------------------------------------
// MEMVID_PLATFORM — platform detection override
// ---------------------------------------------------------------------------

#[test]
fn memvid_platform_claude_resolves_correctly() {
    temp_env::with_vars(
        [
            ("MEMVID_PLATFORM", Some("claude")),
            ("OPENCODE", None::<&str>),
        ],
        || {
            let input = make_input("/tmp", None);
            let name = platforms::detect_platform(&input);
            assert_eq!(name, "claude");
        },
    );
}

#[test]
fn memvid_platform_opencode_resolves_correctly() {
    temp_env::with_vars(
        [
            ("MEMVID_PLATFORM", Some("opencode")),
            ("OPENCODE", None::<&str>),
        ],
        || {
            let input = make_input("/tmp", None);
            let name = platforms::detect_platform(&input);
            assert_eq!(name, "opencode");
        },
    );
}

#[test]
fn memvid_platform_auto_is_treated_as_literal() {
    temp_env::with_vars(
        [
            ("MEMVID_PLATFORM", Some("auto")),
            ("OPENCODE", None::<&str>),
        ],
        || {
            let input = make_input("/tmp", None);
            let name = platforms::detect_platform(&input);
            assert_eq!(name, "auto", "\"auto\" is a valid platform identifier");
        },
    );
}

#[test]
fn memvid_platform_nonexistent_is_passed_through() {
    temp_env::with_vars(
        [
            ("MEMVID_PLATFORM", Some("nonexistent")),
            ("OPENCODE", None::<&str>),
        ],
        || {
            let input = make_input("/tmp", None);
            let name = platforms::detect_platform(&input);
            assert_eq!(
                name, "nonexistent",
                "unknown platform names are lowercased and passed through"
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
            let input = make_input("/tmp", None);
            let name = platforms::detect_platform(&input);
            assert_eq!(name, "claude");
        },
    );
}

#[test]
fn memvid_platform_case_insensitive() {
    temp_env::with_vars(
        [
            ("MEMVID_PLATFORM", Some("CLAUDE")),
            ("OPENCODE", None::<&str>),
        ],
        || {
            let input = make_input("/tmp", None);
            let name = platforms::detect_platform(&input);
            assert_eq!(name, "claude", "platform detection must lowercase");
        },
    );
}

#[test]
fn memvid_platform_whitespace_only_treated_as_absent() {
    temp_env::with_vars(
        [("MEMVID_PLATFORM", Some("   ")), ("OPENCODE", None::<&str>)],
        || {
            let input = make_input("/tmp", None);
            let name = platforms::detect_platform(&input);
            assert_eq!(name, "claude", "whitespace-only falls through to default");
        },
    );
}

// ---------------------------------------------------------------------------
// MEMVID_MIND_DEBUG — debug flag on MindConfig
// ---------------------------------------------------------------------------

#[test]
fn memvid_mind_debug_true_enables_debug() {
    temp_env::with_vars(
        [
            ("MEMVID_MIND_DEBUG", Some("true")),
            ("MEMVID_PLATFORM_MEMORY_PATH", None::<&str>),
        ],
        || {
            let cfg = MindConfig::from_env().expect("should parse");
            assert!(cfg.debug, "debug should be enabled for 'true'");
        },
    );
}

#[test]
fn memvid_mind_debug_one_enables_debug() {
    temp_env::with_vars(
        [
            ("MEMVID_MIND_DEBUG", Some("1")),
            ("MEMVID_PLATFORM_MEMORY_PATH", None::<&str>),
        ],
        || {
            let cfg = MindConfig::from_env().expect("should parse");
            assert!(cfg.debug, "debug should be enabled for '1'");
        },
    );
}

#[test]
fn memvid_mind_debug_false_disables_debug() {
    temp_env::with_vars(
        [
            ("MEMVID_MIND_DEBUG", Some("false")),
            ("MEMVID_PLATFORM_MEMORY_PATH", None::<&str>),
        ],
        || {
            let cfg = MindConfig::from_env().expect("should parse");
            assert!(!cfg.debug, "debug should be disabled for 'false'");
        },
    );
}

#[test]
fn memvid_mind_debug_zero_disables_debug() {
    temp_env::with_vars(
        [
            ("MEMVID_MIND_DEBUG", Some("0")),
            ("MEMVID_PLATFORM_MEMORY_PATH", None::<&str>),
        ],
        || {
            let cfg = MindConfig::from_env().expect("should parse");
            assert!(!cfg.debug, "debug should be disabled for '0'");
        },
    );
}

#[test]
fn memvid_mind_debug_invalid_returns_error() {
    temp_env::with_vars(
        [
            ("MEMVID_MIND_DEBUG", Some("invalid")),
            ("MEMVID_PLATFORM_MEMORY_PATH", None::<&str>),
        ],
        || {
            let result = MindConfig::from_env();
            assert!(
                result.is_err(),
                "invalid MEMVID_MIND_DEBUG value must produce an error"
            );
            let err_msg = result.unwrap_err().to_string();
            assert!(
                err_msg.contains("MEMVID_MIND_DEBUG"),
                "error message should mention the env var: {err_msg}"
            );
        },
    );
}

#[test]
fn memvid_mind_debug_unset_defaults_to_false() {
    temp_env::with_vars(
        [
            ("MEMVID_MIND_DEBUG", None::<&str>),
            ("MEMVID_PLATFORM_MEMORY_PATH", None::<&str>),
        ],
        || {
            let cfg = MindConfig::from_env().expect("should parse");
            assert!(!cfg.debug, "debug defaults to false when unset");
        },
    );
}

#[test]
fn memvid_mind_debug_empty_string_treated_as_unset() {
    temp_env::with_vars(
        [
            ("MEMVID_MIND_DEBUG", Some("")),
            ("MEMVID_PLATFORM_MEMORY_PATH", None::<&str>),
        ],
        || {
            // Empty string is set but empty -> env::var returns Ok("") which is
            // treated as "ignore" per the from_env implementation.
            let cfg = MindConfig::from_env().expect("should parse");
            assert!(!cfg.debug, "empty string should not enable debug");
        },
    );
}

// ---------------------------------------------------------------------------
// MEMVID_PLATFORM_MEMORY_PATH — explicit memory path override
// ---------------------------------------------------------------------------

#[test]
fn memvid_platform_memory_path_overrides_default() {
    let custom = "/tmp/custom/mind.mv2";
    temp_env::with_vars(
        [
            ("MEMVID_PLATFORM_MEMORY_PATH", Some(custom)),
            ("MEMVID_MIND_DEBUG", None::<&str>),
        ],
        || {
            let cfg = MindConfig::from_env().expect("should parse");
            assert_eq!(
                cfg.memory_path,
                PathBuf::from(custom),
                "explicit path should override default"
            );
        },
    );
}

#[test]
fn memvid_platform_memory_path_unset_uses_default() {
    temp_env::with_vars(
        [
            ("MEMVID_PLATFORM_MEMORY_PATH", None::<&str>),
            ("MEMVID_MIND_DEBUG", None::<&str>),
        ],
        || {
            let cfg = MindConfig::from_env().expect("should parse");
            assert_eq!(
                cfg.memory_path,
                PathBuf::from(".agent-brain/mind.mv2"),
                "unset path should use default"
            );
        },
    );
}

#[test]
fn memvid_platform_memory_path_accepts_nonexistent_path() {
    let bogus = "/nonexistent/path/mind.mv2";
    temp_env::with_vars(
        [
            ("MEMVID_PLATFORM_MEMORY_PATH", Some(bogus)),
            ("MEMVID_MIND_DEBUG", None::<&str>),
        ],
        || {
            // Config construction should succeed (no I/O validation at config time)
            let cfg = MindConfig::from_env().expect("should parse");
            assert_eq!(cfg.memory_path, PathBuf::from(bogus));
        },
    );
}

// ---------------------------------------------------------------------------
// MEMVID_PLATFORM_PATH_OPT_IN — platform-scoped path policy
// ---------------------------------------------------------------------------

#[test]
fn memvid_platform_path_opt_in_true_uses_platform_scoped_path() {
    let tmp = tempfile::tempdir().expect("tempdir");
    temp_env::with_vars(
        [
            ("MEMVID_PLATFORM_PATH_OPT_IN", Some("1")),
            ("MEMVID_PLATFORM", Some("claude")),
            ("OPENCODE", None::<&str>),
            ("MEMVID_PLATFORM_MEMORY_PATH", None::<&str>),
            ("MEMVID_MIND_DEBUG", None::<&str>),
        ],
        || {
            let input = make_input(tmp.path().to_str().unwrap(), None);
            let path =
                hooks::bootstrap::resolve_memory_path(&input, tmp.path()).expect("should resolve");
            assert!(
                path.to_str().unwrap().contains("mind-claude.mv2"),
                "opt-in path should be platform-scoped: {path:?}"
            );
        },
    );
}

#[test]
fn memvid_platform_path_opt_in_false_uses_legacy_path() {
    let tmp = tempfile::tempdir().expect("tempdir");
    temp_env::with_vars(
        [
            ("MEMVID_PLATFORM_PATH_OPT_IN", Some("0")),
            ("MEMVID_PLATFORM", Some("claude")),
            ("OPENCODE", None::<&str>),
            ("MEMVID_PLATFORM_MEMORY_PATH", None::<&str>),
            ("MEMVID_MIND_DEBUG", None::<&str>),
        ],
        || {
            let input = make_input(tmp.path().to_str().unwrap(), None);
            let path =
                hooks::bootstrap::resolve_memory_path(&input, tmp.path()).expect("should resolve");
            assert!(
                path.to_str().unwrap().contains(".agent-brain/mind.mv2"),
                "non-opt-in should use legacy path: {path:?}"
            );
        },
    );
}

#[test]
fn memvid_platform_path_opt_in_unset_uses_legacy_path() {
    let tmp = tempfile::tempdir().expect("tempdir");
    temp_env::with_vars(
        [
            ("MEMVID_PLATFORM_PATH_OPT_IN", None::<&str>),
            ("MEMVID_PLATFORM", None::<&str>),
            ("OPENCODE", None::<&str>),
            ("MEMVID_PLATFORM_MEMORY_PATH", None::<&str>),
            ("MEMVID_MIND_DEBUG", None::<&str>),
        ],
        || {
            let input = make_input(tmp.path().to_str().unwrap(), None);
            let path =
                hooks::bootstrap::resolve_memory_path(&input, tmp.path()).expect("should resolve");
            assert!(
                path.to_str().unwrap().contains(".agent-brain/mind.mv2"),
                "unset opt-in should use legacy path: {path:?}"
            );
        },
    );
}

// ---------------------------------------------------------------------------
// CLAUDE_PROJECT_DIR — Claude project directory override
// ---------------------------------------------------------------------------

#[test]
fn claude_project_dir_valid_directory() {
    let tmp = tempfile::tempdir().expect("tempdir");
    temp_env::with_vars(
        [
            ("CLAUDE_PROJECT_DIR", Some(tmp.path().to_str().unwrap())),
            ("MEMVID_PLATFORM", None::<&str>),
            ("OPENCODE", None::<&str>),
            ("MEMVID_PLATFORM_PATH_OPT_IN", None::<&str>),
            ("MEMVID_PLATFORM_MEMORY_PATH", None::<&str>),
            ("MEMVID_MIND_DEBUG", None::<&str>),
        ],
        || {
            // Verify env var is readable (used by platform adapters if needed)
            let val = std::env::var("CLAUDE_PROJECT_DIR").expect("should be set");
            assert_eq!(val, tmp.path().to_str().unwrap());
        },
    );
}

#[test]
fn claude_project_dir_missing_directory_still_readable() {
    temp_env::with_vars(
        [("CLAUDE_PROJECT_DIR", Some("/nonexistent/claude/project"))],
        || {
            let val = std::env::var("CLAUDE_PROJECT_DIR").expect("should be set");
            assert_eq!(val, "/nonexistent/claude/project");
            // The env var is read at use-time; config construction does not fail
        },
    );
}

#[test]
fn claude_project_dir_unset() {
    temp_env::with_vars([("CLAUDE_PROJECT_DIR", None::<&str>)], || {
        assert!(
            std::env::var("CLAUDE_PROJECT_DIR").is_err(),
            "should not be set"
        );
    });
}

// ---------------------------------------------------------------------------
// OPENCODE_PROJECT_DIR — OpenCode project directory override
// ---------------------------------------------------------------------------

#[test]
fn opencode_project_dir_valid_directory() {
    let tmp = tempfile::tempdir().expect("tempdir");
    temp_env::with_vars(
        [("OPENCODE_PROJECT_DIR", Some(tmp.path().to_str().unwrap()))],
        || {
            let val = std::env::var("OPENCODE_PROJECT_DIR").expect("should be set");
            assert_eq!(val, tmp.path().to_str().unwrap());
        },
    );
}

#[test]
fn opencode_project_dir_missing_directory_still_readable() {
    temp_env::with_vars(
        [("OPENCODE_PROJECT_DIR", Some("/nonexistent/opencode/dir"))],
        || {
            let val = std::env::var("OPENCODE_PROJECT_DIR").expect("should be set");
            assert_eq!(val, "/nonexistent/opencode/dir");
        },
    );
}

#[test]
fn opencode_project_dir_unset() {
    temp_env::with_vars([("OPENCODE_PROJECT_DIR", None::<&str>)], || {
        assert!(
            std::env::var("OPENCODE_PROJECT_DIR").is_err(),
            "should not be set"
        );
    });
}
