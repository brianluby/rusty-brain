//! Tests for [`types::MindConfig`] — defaults, validation, serde round-trip, and
//! environment-based construction.
//!
//! Moved from `crates/types/src/config.rs` inline tests (RB-ARCH-009).

use std::path::PathBuf;

use types::error::error_codes;
use types::{AgentBrainError, MindConfig};

/// Run `f` with a set of env-var overrides, then restore the previous state.
///
/// Pass `None` as a value to ensure the variable is unset during the closure.
/// Uses `temp_env::with_vars` which serialises env-var mutations internally
/// and restores state even on panic.
fn with_env<F>(vars: &[(&str, Option<&str>)], f: F)
where
    F: FnOnce(),
{
    temp_env::with_vars(vars, f);
}

// -------------------------------------------------------------------------
// Default field values
// -------------------------------------------------------------------------

#[test]
fn default_memory_path_is_rusty_brain_mind_mv2() {
    let cfg = MindConfig::default();
    assert_eq!(cfg.memory_path, PathBuf::from(".rusty-brain/mind.mv2"));
}

#[test]
fn default_max_context_observations_is_20() {
    let cfg = MindConfig::default();
    assert_eq!(cfg.max_context_observations, 20u32);
}

#[test]
fn default_max_context_tokens_is_2000() {
    let cfg = MindConfig::default();
    assert_eq!(cfg.max_context_tokens, 2000u32);
}

#[test]
fn default_auto_compress_is_true() {
    let cfg = MindConfig::default();
    assert!(cfg.auto_compress);
}

#[test]
fn default_min_confidence_is_0_6() {
    let cfg = MindConfig::default();
    assert!((cfg.min_confidence - 0.6f64).abs() < f64::EPSILON);
}

#[test]
fn default_debug_is_false() {
    let cfg = MindConfig::default();
    assert!(!cfg.debug);
}

// -------------------------------------------------------------------------
// validate(): valid configurations pass
// -------------------------------------------------------------------------

#[test]
fn valid_default_config_passes_validation() {
    let cfg = MindConfig::default();
    assert!(cfg.validate().is_ok());
}

#[test]
fn min_confidence_zero_is_valid_edge() {
    let cfg = MindConfig {
        min_confidence: 0.0,
        ..MindConfig::default()
    };
    assert!(cfg.validate().is_ok());
}

#[test]
fn min_confidence_one_is_valid_edge() {
    let cfg = MindConfig {
        min_confidence: 1.0,
        ..MindConfig::default()
    };
    assert!(cfg.validate().is_ok());
}

// -------------------------------------------------------------------------
// validate(): invalid configurations are rejected
// -------------------------------------------------------------------------

#[test]
fn min_confidence_below_zero_is_rejected() {
    let cfg = MindConfig {
        min_confidence: -0.1,
        ..MindConfig::default()
    };
    let err = cfg.validate().unwrap_err();
    assert!(matches!(err, AgentBrainError::Configuration { .. }));
}

#[test]
fn min_confidence_above_one_is_rejected() {
    let cfg = MindConfig {
        min_confidence: 1.1,
        ..MindConfig::default()
    };
    let err = cfg.validate().unwrap_err();
    assert!(matches!(err, AgentBrainError::Configuration { .. }));
}

#[test]
fn min_confidence_nan_is_rejected() {
    let cfg = MindConfig {
        min_confidence: f64::NAN,
        ..MindConfig::default()
    };
    let err = cfg.validate().unwrap_err();
    assert!(matches!(err, AgentBrainError::Configuration { .. }));
}

#[test]
fn max_context_observations_zero_is_rejected() {
    let cfg = MindConfig {
        max_context_observations: 0,
        ..MindConfig::default()
    };
    let err = cfg.validate().unwrap_err();
    assert!(matches!(err, AgentBrainError::Configuration { .. }));
}

#[test]
fn max_context_tokens_zero_is_rejected() {
    let cfg = MindConfig {
        max_context_tokens: 0,
        ..MindConfig::default()
    };
    let err = cfg.validate().unwrap_err();
    assert!(matches!(err, AgentBrainError::Configuration { .. }));
}

// -------------------------------------------------------------------------
// validate(): error codes
// -------------------------------------------------------------------------

#[test]
fn invalid_min_confidence_error_has_config_invalid_value_code() {
    let cfg = MindConfig {
        min_confidence: -1.0,
        ..MindConfig::default()
    };
    let err = cfg.validate().unwrap_err();
    assert_eq!(err.code(), error_codes::E_CONFIG_INVALID_VALUE);
}

#[test]
fn zero_max_context_observations_error_has_config_invalid_value_code() {
    let cfg = MindConfig {
        max_context_observations: 0,
        ..MindConfig::default()
    };
    let err = cfg.validate().unwrap_err();
    assert_eq!(err.code(), error_codes::E_CONFIG_INVALID_VALUE);
}

#[test]
fn zero_max_context_tokens_error_has_config_invalid_value_code() {
    let cfg = MindConfig {
        max_context_tokens: 0,
        ..MindConfig::default()
    };
    let err = cfg.validate().unwrap_err();
    assert_eq!(err.code(), error_codes::E_CONFIG_INVALID_VALUE);
}

// -------------------------------------------------------------------------
// T019: Round-trip serialization tests
// -------------------------------------------------------------------------

#[test]
fn mind_config_json_round_trip_defaults() {
    let original = MindConfig::default();

    let json = serde_json::to_string(&original).expect("serialization must succeed");
    let deserialized: MindConfig =
        serde_json::from_str(&json).expect("deserialization must succeed");

    assert_eq!(
        original, deserialized,
        "default MindConfig must round-trip without data loss"
    );
    assert!(
        json.contains("memoryPath"),
        "JSON must contain camelCase key 'memoryPath', got: {json}"
    );
    assert_eq!(
        deserialized.memory_path,
        PathBuf::from(".rusty-brain/mind.mv2")
    );
    assert_eq!(deserialized.max_context_observations, 20);
    assert_eq!(deserialized.max_context_tokens, 2000);
    assert!(deserialized.auto_compress);
    assert!((deserialized.min_confidence - 0.6f64).abs() < f64::EPSILON);
    assert!(!deserialized.debug);
}

#[test]
fn mind_config_partial_json_applies_defaults() {
    // Provide only a subset of fields; serde(default) must fill in the rest.
    let partial_json = r#"{"maxContextObservations": 50}"#;
    let deserialized: MindConfig =
        serde_json::from_str(partial_json).expect("partial JSON must deserialize with defaults");

    assert_eq!(
        deserialized.max_context_observations, 50,
        "explicitly provided field must be used"
    );
    // Remaining fields must fall back to Default impl values.
    assert_eq!(
        deserialized.memory_path,
        PathBuf::from(".rusty-brain/mind.mv2"),
        "memory_path must default when absent"
    );
    assert_eq!(
        deserialized.max_context_tokens, 2000,
        "max_context_tokens must default when absent"
    );
    assert!(
        deserialized.auto_compress,
        "auto_compress must default to true when absent"
    );
    assert!(
        (deserialized.min_confidence - 0.6f64).abs() < f64::EPSILON,
        "min_confidence must default to 0.6 when absent"
    );
    assert!(
        !deserialized.debug,
        "debug must default to false when absent"
    );
}

#[test]
fn mind_config_empty_json_object_produces_defaults() {
    let deserialized: MindConfig =
        serde_json::from_str("{}").expect("empty JSON object must deserialize with all defaults");

    let expected = MindConfig::default();
    assert_eq!(
        deserialized, expected,
        "empty JSON object must produce identical value to MindConfig::default()"
    );
}

// -------------------------------------------------------------------------
// T025: from_env() -- happy path
// -------------------------------------------------------------------------

#[test]
fn from_env_no_env_vars_returns_defaults() {
    with_env(
        &[
            ("MEMVID_MIND_DEBUG", None),
            ("MEMVID_PLATFORM_MEMORY_PATH", None),
            ("MEMVID_PLATFORM_PATH_OPT_IN", None),
            ("MEMVID_PLATFORM", None),
            ("CLAUDE_PROJECT_DIR", None),
            ("OPENCODE_PROJECT_DIR", None),
        ],
        || {
            let cfg = MindConfig::from_env().expect("from_env must succeed");
            assert_eq!(cfg, MindConfig::default());
        },
    );
}

#[test]
fn from_env_memory_path_override() {
    with_env(
        &[
            ("MEMVID_PLATFORM_MEMORY_PATH", Some("/custom/path.mv2")),
            ("MEMVID_PLATFORM_PATH_OPT_IN", None),
            ("MEMVID_MIND_DEBUG", None),
            ("MEMVID_PLATFORM", None),
            ("CLAUDE_PROJECT_DIR", None),
            ("OPENCODE_PROJECT_DIR", None),
        ],
        || {
            let cfg = MindConfig::from_env().expect("from_env must succeed");
            assert_eq!(cfg.memory_path, PathBuf::from("/custom/path.mv2"));
        },
    );
}

#[test]
fn from_env_debug_enabled_with_one() {
    with_env(
        &[
            ("MEMVID_MIND_DEBUG", Some("1")),
            ("MEMVID_PLATFORM_MEMORY_PATH", None),
            ("MEMVID_PLATFORM_PATH_OPT_IN", None),
            ("MEMVID_PLATFORM", None),
            ("CLAUDE_PROJECT_DIR", None),
            ("OPENCODE_PROJECT_DIR", None),
        ],
        || {
            let cfg = MindConfig::from_env().expect("from_env must succeed");
            assert!(cfg.debug);
        },
    );
}

#[test]
fn from_env_debug_enabled_with_true() {
    with_env(
        &[
            ("MEMVID_MIND_DEBUG", Some("true")),
            ("MEMVID_PLATFORM_MEMORY_PATH", None),
            ("MEMVID_PLATFORM_PATH_OPT_IN", None),
            ("MEMVID_PLATFORM", None),
            ("CLAUDE_PROJECT_DIR", None),
            ("OPENCODE_PROJECT_DIR", None),
        ],
        || {
            let cfg = MindConfig::from_env().expect("from_env must succeed");
            assert!(cfg.debug);
        },
    );
}

#[test]
fn from_env_debug_enabled_with_true_uppercase() {
    with_env(
        &[
            ("MEMVID_MIND_DEBUG", Some("TRUE")),
            ("MEMVID_PLATFORM_MEMORY_PATH", None),
            ("MEMVID_PLATFORM_PATH_OPT_IN", None),
            ("MEMVID_PLATFORM", None),
            ("CLAUDE_PROJECT_DIR", None),
            ("OPENCODE_PROJECT_DIR", None),
        ],
        || {
            let cfg = MindConfig::from_env().expect("from_env must succeed");
            assert!(cfg.debug);
        },
    );
}

#[test]
fn from_env_debug_disabled_with_zero() {
    with_env(
        &[
            ("MEMVID_MIND_DEBUG", Some("0")),
            ("MEMVID_PLATFORM_MEMORY_PATH", None),
            ("MEMVID_PLATFORM_PATH_OPT_IN", None),
            ("MEMVID_PLATFORM", None),
            ("CLAUDE_PROJECT_DIR", None),
            ("OPENCODE_PROJECT_DIR", None),
        ],
        || {
            let cfg = MindConfig::from_env().expect("from_env must succeed");
            assert!(!cfg.debug);
        },
    );
}

#[test]
fn from_env_debug_disabled_with_false() {
    with_env(
        &[
            ("MEMVID_MIND_DEBUG", Some("false")),
            ("MEMVID_PLATFORM_MEMORY_PATH", None),
            ("MEMVID_PLATFORM_PATH_OPT_IN", None),
            ("MEMVID_PLATFORM", None),
            ("CLAUDE_PROJECT_DIR", None),
            ("OPENCODE_PROJECT_DIR", None),
        ],
        || {
            let cfg = MindConfig::from_env().expect("from_env must succeed");
            assert!(!cfg.debug);
        },
    );
}

#[test]
fn from_env_debug_disabled_with_false_uppercase() {
    with_env(
        &[
            ("MEMVID_MIND_DEBUG", Some("FALSE")),
            ("MEMVID_PLATFORM_MEMORY_PATH", None),
            ("MEMVID_PLATFORM_PATH_OPT_IN", None),
            ("MEMVID_PLATFORM", None),
            ("CLAUDE_PROJECT_DIR", None),
            ("OPENCODE_PROJECT_DIR", None),
        ],
        || {
            let cfg = MindConfig::from_env().expect("from_env must succeed");
            assert!(!cfg.debug);
        },
    );
}

#[test]
fn from_env_ignores_platform_detection_for_memory_path() {
    with_env(
        &[
            ("MEMVID_PLATFORM", Some("claude")),
            ("MEMVID_PLATFORM_PATH_OPT_IN", Some("1")),
            ("MEMVID_PLATFORM_MEMORY_PATH", None),
            ("MEMVID_MIND_DEBUG", None),
            ("CLAUDE_PROJECT_DIR", None),
            ("OPENCODE_PROJECT_DIR", None),
        ],
        || {
            let cfg = MindConfig::from_env().expect("from_env must succeed");
            assert_eq!(cfg.memory_path, PathBuf::from(".rusty-brain/mind.mv2"));
        },
    );
}

#[test]
fn from_env_ignores_claude_project_dir_for_memory_path() {
    with_env(
        &[
            ("MEMVID_PLATFORM", Some("  ")),
            ("CLAUDE_PROJECT_DIR", Some("/some/dir")),
            ("MEMVID_PLATFORM_PATH_OPT_IN", Some("1")),
            ("MEMVID_PLATFORM_MEMORY_PATH", None),
            ("MEMVID_MIND_DEBUG", None),
            ("OPENCODE_PROJECT_DIR", None),
        ],
        || {
            let cfg = MindConfig::from_env().expect("from_env must succeed");
            assert_eq!(cfg.memory_path, PathBuf::from(".rusty-brain/mind.mv2"));
        },
    );
}

#[test]
fn from_env_ignores_claude_project_dir_opt_in() {
    with_env(
        &[
            ("CLAUDE_PROJECT_DIR", Some("/some/dir")),
            ("MEMVID_PLATFORM_PATH_OPT_IN", Some("1")),
            ("MEMVID_PLATFORM", None),
            ("MEMVID_PLATFORM_MEMORY_PATH", None),
            ("MEMVID_MIND_DEBUG", None),
            ("OPENCODE_PROJECT_DIR", None),
        ],
        || {
            let cfg = MindConfig::from_env().expect("from_env must succeed");
            assert_eq!(cfg.memory_path, PathBuf::from(".rusty-brain/mind.mv2"));
        },
    );
}

#[test]
fn from_env_ignores_opencode_project_dir_opt_in() {
    with_env(
        &[
            ("OPENCODE_PROJECT_DIR", Some("/some/dir")),
            ("MEMVID_PLATFORM_PATH_OPT_IN", Some("1")),
            ("MEMVID_PLATFORM", None),
            ("MEMVID_PLATFORM_MEMORY_PATH", None),
            ("MEMVID_MIND_DEBUG", None),
            ("CLAUDE_PROJECT_DIR", None),
        ],
        || {
            let cfg = MindConfig::from_env().expect("from_env must succeed");
            assert_eq!(cfg.memory_path, PathBuf::from(".rusty-brain/mind.mv2"));
        },
    );
}

#[test]
fn from_env_memory_path_override_takes_precedence_over_platform() {
    with_env(
        &[
            ("MEMVID_PLATFORM_MEMORY_PATH", Some("/explicit.mv2")),
            ("MEMVID_PLATFORM_PATH_OPT_IN", Some("1")),
            ("MEMVID_PLATFORM", Some("claude")),
            ("MEMVID_MIND_DEBUG", None),
            ("CLAUDE_PROJECT_DIR", None),
            ("OPENCODE_PROJECT_DIR", None),
        ],
        || {
            let cfg = MindConfig::from_env().expect("from_env must succeed");
            assert_eq!(cfg.memory_path, PathBuf::from("/explicit.mv2"));
        },
    );
}

// -------------------------------------------------------------------------
// T026: from_env() -- error paths
// -------------------------------------------------------------------------

#[test]
fn from_env_invalid_debug_value_returns_configuration_error() {
    with_env(
        &[
            ("MEMVID_MIND_DEBUG", Some("banana")),
            ("MEMVID_PLATFORM_MEMORY_PATH", None),
            ("MEMVID_PLATFORM_PATH_OPT_IN", None),
            ("MEMVID_PLATFORM", None),
            ("CLAUDE_PROJECT_DIR", None),
            ("OPENCODE_PROJECT_DIR", None),
        ],
        || {
            let err = MindConfig::from_env().expect_err("must fail for invalid debug value");
            assert!(
                matches!(err, AgentBrainError::Configuration { .. }),
                "expected Configuration error, got: {err:?}"
            );
            assert_eq!(err.code(), error_codes::E_CONFIG_INVALID_VALUE);
        },
    );
}

#[test]
fn from_env_invalid_debug_empty_is_ignored() {
    with_env(
        &[
            ("MEMVID_MIND_DEBUG", Some("")),
            ("MEMVID_PLATFORM_MEMORY_PATH", None),
            ("MEMVID_PLATFORM_PATH_OPT_IN", None),
            ("MEMVID_PLATFORM", None),
            ("CLAUDE_PROJECT_DIR", None),
            ("OPENCODE_PROJECT_DIR", None),
        ],
        || {
            let cfg = MindConfig::from_env().expect("from_env must succeed for empty string");
            assert!(!cfg.debug, "debug should be false when env var is empty");
        },
    );
}

#[test]
fn from_env_does_not_construct_platform_scoped_path() {
    with_env(
        &[
            ("MEMVID_PLATFORM", Some("../../etc/passwd")),
            ("MEMVID_PLATFORM_PATH_OPT_IN", Some("1")),
            ("MEMVID_PLATFORM_MEMORY_PATH", None),
            ("MEMVID_MIND_DEBUG", None),
            ("CLAUDE_PROJECT_DIR", None),
            ("OPENCODE_PROJECT_DIR", None),
        ],
        || {
            let cfg = MindConfig::from_env().expect("from_env must succeed");
            assert_eq!(cfg.memory_path, PathBuf::from(".rusty-brain/mind.mv2"));
        },
    );
}
