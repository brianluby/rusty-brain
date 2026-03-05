//! T045: Invalid environment variable tests.
//!
//! Verifies that `MEMVID_PLATFORM=nonexistent` and other invalid env var values
//! produce clear, structured errors rather than panics or silent failures.

mod common;

use types::MindConfig;
use types::hooks::HookInput;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_input(cwd: &str) -> HookInput {
    serde_json::from_value(serde_json::json!({
        "session_id": "invalid-env-test",
        "transcript_path": "/tmp/transcript.jsonl",
        "cwd": cwd,
        "permission_mode": "default",
        "hook_event_name": "SessionStart"
    }))
    .expect("valid HookInput JSON")
}

// ---------------------------------------------------------------------------
// MEMVID_PLATFORM=nonexistent — detection succeeds but path resolution
// still works (unknown platforms get sanitized names, not errors)
// ---------------------------------------------------------------------------

#[test]
fn nonexistent_platform_detected_as_literal_name() {
    temp_env::with_vars(
        [
            ("MEMVID_PLATFORM", Some("nonexistent")),
            ("OPENCODE", None::<&str>),
        ],
        || {
            let input = make_input("/tmp");
            let name = platforms::detect_platform(&input);
            assert_eq!(
                name, "nonexistent",
                "unknown platform names pass through as-is (lowercased)"
            );
        },
    );
}

#[test]
fn nonexistent_platform_resolve_memory_path_succeeds() {
    let tmp = tempfile::tempdir().expect("tempdir");
    temp_env::with_vars(
        [
            ("MEMVID_PLATFORM", Some("nonexistent")),
            ("OPENCODE", None::<&str>),
            ("MEMVID_PLATFORM_PATH_OPT_IN", None::<&str>),
            ("MEMVID_PLATFORM_MEMORY_PATH", None::<&str>),
            ("MEMVID_MIND_DEBUG", None::<&str>),
        ],
        || {
            let input = make_input(tmp.path().to_str().unwrap());
            let result = hooks::bootstrap::resolve_memory_path(&input, tmp.path());
            // Without opt-in, falls back to legacy path regardless of platform
            assert!(
                result.is_ok(),
                "nonexistent platform should still resolve a memory path: {result:?}"
            );
            let path = result.unwrap();
            assert!(
                path.to_str().unwrap().contains("mind.mv2"),
                "resolved path should contain mind.mv2: {path:?}"
            );
        },
    );
}

#[test]
fn nonexistent_platform_with_opt_in_produces_sanitized_path() {
    let tmp = tempfile::tempdir().expect("tempdir");
    temp_env::with_vars(
        [
            ("MEMVID_PLATFORM", Some("nonexistent")),
            ("OPENCODE", None::<&str>),
            ("MEMVID_PLATFORM_PATH_OPT_IN", Some("1")),
            ("MEMVID_PLATFORM_MEMORY_PATH", None::<&str>),
            ("MEMVID_MIND_DEBUG", None::<&str>),
        ],
        || {
            let input = make_input(tmp.path().to_str().unwrap());
            let result = hooks::bootstrap::resolve_memory_path(&input, tmp.path());
            assert!(
                result.is_ok(),
                "nonexistent platform with opt-in should resolve: {result:?}"
            );
            let path = result.unwrap();
            assert!(
                path.to_str().unwrap().contains("mind-nonexistent.mv2"),
                "opt-in path should use sanitized platform name: {path:?}"
            );
        },
    );
}

// ---------------------------------------------------------------------------
// MEMVID_MIND_DEBUG with invalid values
// ---------------------------------------------------------------------------

#[test]
fn memvid_mind_debug_invalid_value_returns_structured_error() {
    temp_env::with_vars(
        [
            ("MEMVID_MIND_DEBUG", Some("invalid")),
            ("MEMVID_PLATFORM_MEMORY_PATH", None::<&str>),
        ],
        || {
            let result = MindConfig::from_env();
            assert!(result.is_err(), "invalid debug value must be rejected");
            let err = result.unwrap_err();
            let err_msg = err.to_string();
            assert!(
                err_msg.contains("MEMVID_MIND_DEBUG"),
                "error should name the offending env var: {err_msg}"
            );
            assert!(
                err_msg.contains("invalid"),
                "error should include the invalid value: {err_msg}"
            );
        },
    );
}

#[test]
fn memvid_mind_debug_yes_is_invalid() {
    temp_env::with_vars(
        [
            ("MEMVID_MIND_DEBUG", Some("yes")),
            ("MEMVID_PLATFORM_MEMORY_PATH", None::<&str>),
        ],
        || {
            let result = MindConfig::from_env();
            assert!(result.is_err(), "'yes' is not a recognized boolean value");
        },
    );
}

#[test]
fn memvid_mind_debug_random_string_is_invalid() {
    temp_env::with_vars(
        [
            ("MEMVID_MIND_DEBUG", Some("xyz123")),
            ("MEMVID_PLATFORM_MEMORY_PATH", None::<&str>),
        ],
        || {
            let result = MindConfig::from_env();
            assert!(result.is_err(), "random string must be rejected");
            let err_msg = result.unwrap_err().to_string();
            assert!(
                err_msg.contains("xyz123"),
                "error should echo the bad value: {err_msg}"
            );
        },
    );
}

// ---------------------------------------------------------------------------
// MEMVID_PLATFORM with special characters
// ---------------------------------------------------------------------------

#[test]
fn memvid_platform_special_chars_sanitized_in_path() {
    let tmp = tempfile::tempdir().expect("tempdir");
    temp_env::with_vars(
        [
            ("MEMVID_PLATFORM", Some("my.platform!v2")),
            ("OPENCODE", None::<&str>),
            ("MEMVID_PLATFORM_PATH_OPT_IN", Some("1")),
            ("MEMVID_PLATFORM_MEMORY_PATH", None::<&str>),
            ("MEMVID_MIND_DEBUG", None::<&str>),
        ],
        || {
            let input = make_input(tmp.path().to_str().unwrap());
            let result = hooks::bootstrap::resolve_memory_path(&input, tmp.path());
            assert!(
                result.is_ok(),
                "special chars in platform name should be sanitized: {result:?}"
            );
            let path = result.unwrap();
            // Special chars replaced with hyphens
            assert!(
                path.to_str().unwrap().contains("mind-my-platform-v2.mv2"),
                "sanitized name should replace special chars: {path:?}"
            );
        },
    );
}

// ---------------------------------------------------------------------------
// should_process remains fail-open with invalid platforms
// ---------------------------------------------------------------------------

#[test]
fn should_process_fail_open_with_nonexistent_platform() {
    temp_env::with_vars(
        [
            ("MEMVID_PLATFORM", Some("nonexistent")),
            ("OPENCODE", None::<&str>),
        ],
        || {
            let input = make_input("/tmp");
            let result = hooks::bootstrap::should_process(&input, "session_start");
            assert!(
                result,
                "should_process must fail-open even for unknown platforms"
            );
        },
    );
}
