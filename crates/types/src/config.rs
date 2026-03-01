//! Memory engine configuration (paths, limits, feature flags).
//!
//! [`MindConfig`] controls the memory engine's runtime behavior. Configuration
//! values are resolved with precedence: environment variable > JSON file >
//! programmatic default. Use [`MindConfig::from_env`] for environment-aware
//! construction or [`MindConfig::default`] for sensible defaults.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::{AgentBrainError, error_codes};

/// Configuration controlling the memory engine's behavior.
///
/// Supports three resolution sources with precedence:
/// environment variable > JSON file > programmatic default.
/// All fields have sensible defaults via the [`Default`] impl.
/// Serialized with camelCase keys; `#[serde(default)]` allows partial JSON.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(default)]
pub struct MindConfig {
    /// Path to the memvid memory database file. Default: `.agent-brain/mind.mv2`.
    pub memory_path: PathBuf,
    /// Maximum number of observations included in injected context. Default: 20.
    pub max_context_observations: u32,
    /// Maximum token budget for the injected context payload. Default: 2000.
    pub max_context_tokens: u32,
    /// Whether to automatically compress observations before storage. Default: true.
    pub auto_compress: bool,
    /// Minimum confidence threshold (0.0..=1.0) for memory retrieval. Default: 0.6.
    pub min_confidence: f64,
    /// Enable debug logging for the memory engine. Default: false.
    pub debug: bool,
}

impl Default for MindConfig {
    fn default() -> Self {
        Self {
            memory_path: PathBuf::from(".agent-brain/mind.mv2"),
            max_context_observations: 20,
            max_context_tokens: 2000,
            auto_compress: true,
            min_confidence: 0.6,
            debug: false,
        }
    }
}

impl MindConfig {
    /// Validate this configuration's invariants.
    ///
    /// Checks:
    /// - `min_confidence` is in `0.0..=1.0`
    /// - `max_context_observations` is > 0
    /// - `max_context_tokens` is > 0
    ///
    /// # Errors
    ///
    /// Returns [`AgentBrainError::Configuration`] with code
    /// [`error_codes::E_CONFIG_INVALID_VALUE`] if any invariant is violated.
    pub fn validate(&self) -> Result<(), AgentBrainError> {
        if !(0.0..=1.0).contains(&self.min_confidence) {
            return Err(AgentBrainError::Configuration {
                code: error_codes::E_CONFIG_INVALID_VALUE,
                message: format!(
                    "min_confidence must be in 0.0..=1.0, got {}",
                    self.min_confidence
                ),
            });
        }
        if self.max_context_observations == 0 {
            return Err(AgentBrainError::Configuration {
                code: error_codes::E_CONFIG_INVALID_VALUE,
                message: "max_context_observations must be greater than 0".to_string(),
            });
        }
        if self.max_context_tokens == 0 {
            return Err(AgentBrainError::Configuration {
                code: error_codes::E_CONFIG_INVALID_VALUE,
                message: "max_context_tokens must be greater than 0".to_string(),
            });
        }
        Ok(())
    }

    /// Build a [`MindConfig`] from environment variables, falling back to defaults.
    ///
    /// Resolution order (highest precedence first):
    /// 1. `MEMVID_MIND_DEBUG` — accepts `"1"` or `"true"` (case-insensitive); empty string is
    ///    ignored; any other non-empty value is an error.
    /// 2. `MEMVID_PLATFORM_MEMORY_PATH` — overrides `memory_path` directly.
    /// 3. If `MEMVID_PLATFORM_PATH_OPT_IN=1` and `MEMVID_PLATFORM_MEMORY_PATH` is not set,
    ///    auto-detect platform via `MEMVID_PLATFORM`, then `CLAUDE_PROJECT_DIR` presence
    ///    (`"claude"`), then `OPENCODE_PROJECT_DIR` presence (`"opencode"`), and set
    ///    `memory_path = ".agent-brain/mind-{platform}.mv2"`.
    /// 4. All other fields use [`MindConfig::default`].
    ///
    /// # Errors
    ///
    /// Returns [`AgentBrainError::Configuration`] with code
    /// [`error_codes::E_CONFIG_INVALID_VALUE`] if `MEMVID_MIND_DEBUG` is set to an unrecognised
    /// non-empty value, or if the resulting configuration fails [`MindConfig::validate`].
    ///
    /// # Thread Safety
    ///
    /// This function reads process-global environment variables via [`std::env::var`].
    /// It must not be called concurrently with code that mutates environment variables
    /// (e.g. [`std::env::set_var`]). In tests, use [`temp_env::with_vars`] or an
    /// equivalent serializing guard.
    pub fn from_env() -> Result<Self, AgentBrainError> {
        let mut cfg = Self::default();

        // --- MEMVID_MIND_DEBUG ---
        if let Ok(raw) = std::env::var("MEMVID_MIND_DEBUG") {
            if !raw.is_empty() {
                match raw.trim().to_lowercase().as_str() {
                    "1" | "true" => cfg.debug = true,
                    _ => {
                        return Err(AgentBrainError::Configuration {
                            code: error_codes::E_CONFIG_INVALID_VALUE,
                            message: format!(
                                "MEMVID_MIND_DEBUG must be '1' or 'true' (case-insensitive), got '{raw}'"
                            ),
                        });
                    }
                }
            }
        }

        // --- MEMVID_PLATFORM_MEMORY_PATH (explicit override, highest path precedence) ---
        let explicit_path = std::env::var("MEMVID_PLATFORM_MEMORY_PATH").ok();
        if let Some(ref path) = explicit_path {
            cfg.memory_path = PathBuf::from(path);
        }

        // --- Platform auto-detection (only when opt-in and no explicit path) ---
        let opt_in = std::env::var("MEMVID_PLATFORM_PATH_OPT_IN")
            .map(|v| v == "1")
            .unwrap_or(false);

        if opt_in && explicit_path.is_none() {
            let platform: Option<String> = if let Ok(p) = std::env::var("MEMVID_PLATFORM") {
                let p = p.trim().to_lowercase();
                if p.is_empty() { None } else { Some(p) }
            } else if std::env::var("CLAUDE_PROJECT_DIR").is_ok() {
                Some("claude".to_string())
            } else if std::env::var("OPENCODE_PROJECT_DIR").is_ok() {
                Some("opencode".to_string())
            } else {
                None
            };

            if let Some(platform) = platform {
                cfg.memory_path = PathBuf::from(format!(".agent-brain/mind-{platform}.mv2"));
            }
        }

        cfg.validate()?;
        Ok(cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

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

    // T008: Unit tests for MindConfig — RED phase (no implementation yet)

    // -------------------------------------------------------------------------
    // Default field values
    // -------------------------------------------------------------------------

    #[test]
    fn default_memory_path_is_agent_brain_mind_mv2() {
        let cfg = MindConfig::default();
        assert_eq!(cfg.memory_path, PathBuf::from(".agent-brain/mind.mv2"));
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
        use crate::error::error_codes;
        let cfg = MindConfig {
            min_confidence: -1.0,
            ..MindConfig::default()
        };
        let err = cfg.validate().unwrap_err();
        assert_eq!(err.code(), error_codes::E_CONFIG_INVALID_VALUE);
    }

    #[test]
    fn zero_max_context_observations_error_has_config_invalid_value_code() {
        use crate::error::error_codes;
        let cfg = MindConfig {
            max_context_observations: 0,
            ..MindConfig::default()
        };
        let err = cfg.validate().unwrap_err();
        assert_eq!(err.code(), error_codes::E_CONFIG_INVALID_VALUE);
    }

    #[test]
    fn zero_max_context_tokens_error_has_config_invalid_value_code() {
        use crate::error::error_codes;
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
            PathBuf::from(".agent-brain/mind.mv2")
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
        let deserialized: MindConfig = serde_json::from_str(partial_json)
            .expect("partial JSON must deserialize with defaults");

        assert_eq!(
            deserialized.max_context_observations, 50,
            "explicitly provided field must be used"
        );
        // Remaining fields must fall back to Default impl values.
        assert_eq!(
            deserialized.memory_path,
            PathBuf::from(".agent-brain/mind.mv2"),
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
        let deserialized: MindConfig = serde_json::from_str("{}")
            .expect("empty JSON object must deserialize with all defaults");

        let expected = MindConfig::default();
        assert_eq!(
            deserialized, expected,
            "empty JSON object must produce identical value to MindConfig::default()"
        );
    }

    // -------------------------------------------------------------------------
    // T025: from_env() — happy path
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
    fn from_env_platform_detection_claude() {
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
                let path = cfg.memory_path.to_string_lossy();
                assert!(
                    path.contains("mind-claude.mv2"),
                    "expected mind-claude.mv2 in path, got: {path}"
                );
            },
        );
    }

    #[test]
    fn from_env_platform_detection_from_claude_project_dir() {
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
                let path = cfg.memory_path.to_string_lossy();
                assert!(
                    path.contains("mind-claude.mv2"),
                    "expected mind-claude.mv2 in path, got: {path}"
                );
            },
        );
    }

    #[test]
    fn from_env_platform_detection_from_opencode_project_dir() {
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
                let path = cfg.memory_path.to_string_lossy();
                assert!(
                    path.contains("mind-opencode.mv2"),
                    "expected mind-opencode.mv2 in path, got: {path}"
                );
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
    // T026: from_env() — error paths
    // -------------------------------------------------------------------------

    #[test]
    fn from_env_invalid_debug_value_returns_configuration_error() {
        use crate::error::error_codes;
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
}
