//! Platform detection from environment variables and hook input.

use types::HookInput;

/// Detect which platform is running.
///
/// Priority (FR-006):
/// 1. Explicit `platform` field in hook input (if present, non-empty, non-whitespace)
/// 2. `MEMVID_PLATFORM` environment variable (if set, non-whitespace)
/// 3. Platform-specific indicators: `OPENCODE=1` -> "opencode"
/// 4. Default: "claude"
///
/// Result is always lowercase and trimmed (FR-007).
#[must_use]
pub fn detect_platform(input: &HookInput) -> String {
    // Check explicit platform field
    if let Some(ref p) = input.platform {
        let trimmed = p.trim();
        if !trimmed.is_empty() {
            return trimmed.to_lowercase();
        }
    }

    // Check MEMVID_PLATFORM env var
    if let Ok(val) = std::env::var("MEMVID_PLATFORM") {
        let trimmed = val.trim().to_string();
        if !trimmed.is_empty() {
            return trimmed.to_lowercase();
        }
    }

    // Check OPENCODE indicator (value must be "1" per FR-006)
    if let Ok(val) = std::env::var("OPENCODE") {
        if val == "1" {
            return "opencode".to_string();
        }
    }

    // Default
    "claude".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal valid `HookInput` with the optional `platform` field set.
    ///
    /// Uses JSON deserialization to work around `#[non_exhaustive]` on `HookInput`.
    fn make_input(platform: Option<&str>) -> HookInput {
        let mut json = serde_json::json!({
            "session_id": "test-session",
            "transcript_path": "/tmp/transcript.jsonl",
            "cwd": "/tmp",
            "permission_mode": "default",
            "hook_event_name": "PostToolUse"
        });
        if let Some(p) = platform {
            json["platform"] = serde_json::Value::String(p.to_string());
        }
        serde_json::from_value(json).expect("valid HookInput JSON")
    }

    #[test]
    fn explicit_platform_field_wins() {
        temp_env::with_vars(
            [
                ("MEMVID_PLATFORM", Some("other")),
                ("OPENCODE", None::<&str>),
            ],
            || {
                let input = make_input(Some("CustomPlatform"));
                assert_eq!(detect_platform(&input), "customplatform");
            },
        );
    }

    #[test]
    fn memvid_platform_env_used_when_no_explicit() {
        temp_env::with_vars(
            [
                ("MEMVID_PLATFORM", Some("EnvPlatform")),
                ("OPENCODE", None::<&str>),
            ],
            || {
                let input = make_input(None);
                assert_eq!(detect_platform(&input), "envplatform");
            },
        );
    }

    #[test]
    fn opencode_indicator_detected() {
        temp_env::with_vars(
            [("MEMVID_PLATFORM", None::<&str>), ("OPENCODE", Some("1"))],
            || {
                let input = make_input(None);
                assert_eq!(detect_platform(&input), "opencode");
            },
        );
    }

    #[test]
    fn default_claude_when_nothing_set() {
        temp_env::with_vars(
            [
                ("MEMVID_PLATFORM", None::<&str>),
                ("OPENCODE", None::<&str>),
            ],
            || {
                let input = make_input(None);
                assert_eq!(detect_platform(&input), "claude");
            },
        );
    }

    #[test]
    fn case_normalization_to_lowercase() {
        temp_env::with_vars(
            [
                ("MEMVID_PLATFORM", None::<&str>),
                ("OPENCODE", None::<&str>),
            ],
            || {
                let input = make_input(Some("CLAUDE"));
                assert_eq!(detect_platform(&input), "claude");
            },
        );
    }

    #[test]
    fn whitespace_trimming() {
        temp_env::with_vars(
            [
                ("MEMVID_PLATFORM", None::<&str>),
                ("OPENCODE", None::<&str>),
            ],
            || {
                let input = make_input(Some("  claude  "));
                assert_eq!(detect_platform(&input), "claude");
            },
        );
    }

    #[test]
    fn whitespace_only_memvid_platform_treated_as_absent() {
        temp_env::with_vars(
            [("MEMVID_PLATFORM", Some("   ")), ("OPENCODE", None::<&str>)],
            || {
                let input = make_input(None);
                assert_eq!(detect_platform(&input), "claude");
            },
        );
    }

    #[test]
    fn opencode_non_one_value_treated_as_absent() {
        temp_env::with_vars(
            [("MEMVID_PLATFORM", None::<&str>), ("OPENCODE", Some("0"))],
            || {
                let input = make_input(None);
                assert_eq!(
                    detect_platform(&input),
                    "claude",
                    "OPENCODE=0 must not trigger opencode detection"
                );
            },
        );
    }

    #[test]
    fn opencode_empty_value_treated_as_absent() {
        temp_env::with_vars(
            [("MEMVID_PLATFORM", None::<&str>), ("OPENCODE", Some(""))],
            || {
                let input = make_input(None);
                assert_eq!(
                    detect_platform(&input),
                    "claude",
                    "OPENCODE='' must not trigger opencode detection"
                );
            },
        );
    }

    #[test]
    fn empty_platform_field_treated_as_absent() {
        temp_env::with_vars(
            [
                ("MEMVID_PLATFORM", None::<&str>),
                ("OPENCODE", None::<&str>),
            ],
            || {
                let input = make_input(Some(""));
                // Empty platform field falls through; with no env vars, default is "claude"
                assert_eq!(detect_platform(&input), "claude");
            },
        );
    }
}
