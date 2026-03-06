//! Memory engine configuration (paths, limits, feature flags).
//!
//! [`MindConfig`] controls the memory engine's runtime behavior. Configuration
//! values are resolved with precedence: environment variable > JSON file >
//! programmatic default. Use [`MindConfig::from_env`] for environment-aware
//! construction or [`MindConfig::default`] for sensible defaults.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::{AgentBrainError, error_codes};

/// Sanitize a platform name for safe use in filesystem paths.
///
/// Keeps ASCII alphanumeric characters plus `-` and `_`, replacing all other
/// characters with `-`.
#[must_use]
pub fn sanitize_platform_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

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
    /// Path to the memvid memory database file. Default: `.rusty-brain/mind.mv2`.
    pub memory_path: PathBuf,
    /// Maximum number of observations included in injected context. Default: 20.
    pub max_context_observations: u32,
    /// Maximum token budget for the injected context payload. Default: 2000.
    pub max_context_tokens: u32,
    /// Reserved compatibility flag for automatic compression. Default: true.
    ///
    /// Current runtime implementations keep full-fidelity observation payloads,
    /// so this flag is retained for forward compatibility.
    pub auto_compress: bool,
    /// Minimum confidence threshold (0.0..=1.0) for memory retrieval. Default: 0.6.
    ///
    /// Applied by `Mind::search` and downstream consumers that rely on it.
    pub min_confidence: f64,
    /// Enable additional debug diagnostics in the memory engine. Default: false.
    pub debug: bool,
}

impl Default for MindConfig {
    fn default() -> Self {
        Self {
            memory_path: PathBuf::from(".rusty-brain/mind.mv2"),
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
    /// 1. `MEMVID_MIND_DEBUG` — accepts `"1"`/`"true"` to enable or `"0"`/`"false"` to disable
    ///    (case-insensitive); empty string is ignored; any other non-empty value is an error.
    /// 2. `MEMVID_PLATFORM_MEMORY_PATH` — optional explicit `memory_path` override.
    /// 3. All other fields use [`MindConfig::default`].
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
                    "0" | "false" => cfg.debug = false,
                    _ => {
                        return Err(AgentBrainError::Configuration {
                            code: error_codes::E_CONFIG_INVALID_VALUE,
                            message: format!(
                                "MEMVID_MIND_DEBUG must be '1', 'true', '0', or 'false' (case-insensitive), got '{raw}'"
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

        cfg.validate()?;
        Ok(cfg)
    }
}
