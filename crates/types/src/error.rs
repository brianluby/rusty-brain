//! Unified error type and stable, machine-parseable error code constants.
//!
//! Every error produced by rusty-brain carries a stable string code from the
//! [`error_codes`] module, enabling downstream consumers (agents, CLIs, tests)
//! to match on error identity without parsing human-readable messages.

/// Stable error code constants used across all rusty-brain crates.
///
/// Each constant is a `&'static str` suitable for machine parsing. Codes are
/// grouped by subsystem prefix: `E_FS_*` (filesystem), `E_CONFIG_*`
/// (configuration), `E_SER_*` (serialization), `E_LOCK_*` (locking),
/// `E_MEM_*` (memory integrity), `E_INPUT_*` (input validation), and
/// `E_PLATFORM_*` (platform adapter).
pub mod error_codes {
    /// File or directory not found.
    pub const E_FS_NOT_FOUND: &str = "E_FS_NOT_FOUND";
    /// Insufficient permissions to access a file or directory.
    pub const E_FS_PERMISSION_DENIED: &str = "E_FS_PERMISSION_DENIED";
    /// General I/O error during filesystem operations.
    pub const E_FS_IO_ERROR: &str = "E_FS_IO_ERROR";

    /// A configuration value is outside its valid range or type.
    pub const E_CONFIG_INVALID_VALUE: &str = "E_CONFIG_INVALID_VALUE";
    /// A required configuration field is absent.
    pub const E_CONFIG_MISSING_FIELD: &str = "E_CONFIG_MISSING_FIELD";
    /// Configuration file could not be parsed (syntax error).
    pub const E_CONFIG_PARSE_ERROR: &str = "E_CONFIG_PARSE_ERROR";

    /// Serialization to JSON (or another format) failed.
    pub const E_SER_SERIALIZE_FAILED: &str = "E_SER_SERIALIZE_FAILED";
    /// Deserialization from JSON (or another format) failed.
    pub const E_SER_DESERIALIZE_FAILED: &str = "E_SER_DESERIALIZE_FAILED";

    /// Could not acquire a file or resource lock.
    pub const E_LOCK_ACQUISITION_FAILED: &str = "E_LOCK_ACQUISITION_FAILED";
    /// Lock acquisition timed out.
    pub const E_LOCK_TIMEOUT: &str = "E_LOCK_TIMEOUT";

    /// Memory index is corrupted or structurally invalid.
    pub const E_MEM_CORRUPTED_INDEX: &str = "E_MEM_CORRUPTED_INDEX";
    /// Checksum verification failed on a memory blob.
    pub const E_MEM_INVALID_CHECKSUM: &str = "E_MEM_INVALID_CHECKSUM";

    /// A required field is empty or whitespace-only.
    pub const E_INPUT_EMPTY_FIELD: &str = "E_INPUT_EMPTY_FIELD";
    /// A numeric or temporal value is outside the allowed range.
    pub const E_INPUT_OUT_OF_RANGE: &str = "E_INPUT_OUT_OF_RANGE";
    /// Input string does not match the expected format.
    pub const E_INPUT_INVALID_FORMAT: &str = "E_INPUT_INVALID_FORMAT";

    /// Event contract version is incompatible (major version mismatch).
    pub const E_PLATFORM_INCOMPATIBLE_CONTRACT: &str = "E_PLATFORM_INCOMPATIBLE_CONTRACT";
    /// Event contract version string could not be parsed.
    pub const E_PLATFORM_INVALID_CONTRACT_VERSION: &str = "E_PLATFORM_INVALID_CONTRACT_VERSION";
    /// Hook input lacks a required session ID.
    pub const E_PLATFORM_MISSING_SESSION_ID: &str = "E_PLATFORM_MISSING_SESSION_ID";
    /// Project identity could not be resolved from context.
    pub const E_PLATFORM_MISSING_PROJECT_IDENTITY: &str = "E_PLATFORM_MISSING_PROJECT_IDENTITY";
    /// Resolved memory path escapes the project directory.
    pub const E_PLATFORM_PATH_TRAVERSAL: &str = "E_PLATFORM_PATH_TRAVERSAL";
    /// No adapter registered for the requested platform name.
    pub const E_PLATFORM_ADAPTER_NOT_FOUND: &str = "E_PLATFORM_ADAPTER_NOT_FOUND";
}

/// Unified error type for all rusty-brain operations.
///
/// Each variant carries a stable `code` from [`error_codes`] and a
/// human-readable `message`. Some variants also wrap an underlying `source`
/// error for `Error::source()` chaining. The enum is `#[non_exhaustive]` so
/// new variants can be added without a breaking change.
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum AgentBrainError {
    /// Filesystem I/O failure (read, write, path resolution).
    #[error("[{code}] {message}")]
    FileSystem {
        /// Stable error code (e.g. [`error_codes::E_FS_NOT_FOUND`]).
        code: &'static str,
        /// Human-readable description of the failure.
        message: String,
        /// Optional underlying [`std::io::Error`].
        #[source]
        source: Option<std::io::Error>,
    },
    /// Configuration loading or validation failure.
    #[error("[{code}] {message}")]
    Configuration {
        /// Stable error code (e.g. [`error_codes::E_CONFIG_INVALID_VALUE`]).
        code: &'static str,
        /// Human-readable description of the failure.
        message: String,
    },
    /// JSON serialization or deserialization failure.
    #[error("[{code}] {message}")]
    Serialization {
        /// Stable error code (e.g. [`error_codes::E_SER_SERIALIZE_FAILED`]).
        code: &'static str,
        /// Human-readable description of the failure.
        message: String,
        /// Optional underlying [`serde_json::Error`].
        #[source]
        source: Option<serde_json::Error>,
    },
    /// File or resource lock could not be acquired.
    #[error("[{code}] {message}")]
    Lock {
        /// Stable error code (e.g. [`error_codes::E_LOCK_TIMEOUT`]).
        code: &'static str,
        /// Human-readable description of the failure.
        message: String,
    },
    /// Memory store integrity violation (corrupted index, bad checksum).
    #[error("[{code}] {message}")]
    MemoryCorruption {
        /// Stable error code (e.g. [`error_codes::E_MEM_CORRUPTED_INDEX`]).
        code: &'static str,
        /// Human-readable description of the failure.
        message: String,
    },
    /// Caller-provided input failed validation.
    #[error("[{code}] {message}")]
    InvalidInput {
        /// Stable error code (e.g. [`error_codes::E_INPUT_EMPTY_FIELD`]).
        code: &'static str,
        /// Human-readable description of the failure.
        message: String,
    },
    /// Platform adapter system failure.
    #[error("[{code}] {message}")]
    Platform {
        /// Stable error code (e.g. [`error_codes::E_PLATFORM_PATH_TRAVERSAL`]).
        code: &'static str,
        /// Human-readable description of the failure.
        message: String,
    },
}

impl AgentBrainError {
    /// Returns the stable error code string for this error.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::FileSystem { code, .. }
            | Self::Configuration { code, .. }
            | Self::Serialization { code, .. }
            | Self::Lock { code, .. }
            | Self::MemoryCorruption { code, .. }
            | Self::InvalidInput { code, .. }
            | Self::Platform { code, .. } => code,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;

    // T003: Unit tests for AgentBrainError

    #[test]
    fn filesystem_variant_has_correct_code() {
        let err = AgentBrainError::FileSystem {
            code: error_codes::E_FS_NOT_FOUND,
            message: "file not found".to_string(),
            source: None,
        };
        assert_eq!(err.code(), "E_FS_NOT_FOUND");
        assert_eq!(format!("{err}"), "[E_FS_NOT_FOUND] file not found");
    }

    #[test]
    fn configuration_variant_has_correct_code() {
        let err = AgentBrainError::Configuration {
            code: error_codes::E_CONFIG_INVALID_VALUE,
            message: "bad config".to_string(),
        };
        assert_eq!(err.code(), "E_CONFIG_INVALID_VALUE");
        assert_eq!(format!("{err}"), "[E_CONFIG_INVALID_VALUE] bad config");
    }

    #[test]
    fn serialization_variant_has_correct_code() {
        let err = AgentBrainError::Serialization {
            code: error_codes::E_SER_SERIALIZE_FAILED,
            message: "serialize failed".to_string(),
            source: None,
        };
        assert_eq!(err.code(), "E_SER_SERIALIZE_FAILED");
        assert_eq!(
            format!("{err}"),
            "[E_SER_SERIALIZE_FAILED] serialize failed"
        );
    }

    #[test]
    fn lock_variant_has_correct_code() {
        let err = AgentBrainError::Lock {
            code: error_codes::E_LOCK_ACQUISITION_FAILED,
            message: "lock failed".to_string(),
        };
        assert_eq!(err.code(), "E_LOCK_ACQUISITION_FAILED");
        assert_eq!(format!("{err}"), "[E_LOCK_ACQUISITION_FAILED] lock failed");
    }

    #[test]
    fn memory_corruption_variant_has_correct_code() {
        let err = AgentBrainError::MemoryCorruption {
            code: error_codes::E_MEM_CORRUPTED_INDEX,
            message: "index corrupt".to_string(),
        };
        assert_eq!(err.code(), "E_MEM_CORRUPTED_INDEX");
        assert_eq!(format!("{err}"), "[E_MEM_CORRUPTED_INDEX] index corrupt");
    }

    #[test]
    fn invalid_input_variant_has_correct_code() {
        let err = AgentBrainError::InvalidInput {
            code: error_codes::E_INPUT_EMPTY_FIELD,
            message: "field empty".to_string(),
        };
        assert_eq!(err.code(), "E_INPUT_EMPTY_FIELD");
        assert_eq!(format!("{err}"), "[E_INPUT_EMPTY_FIELD] field empty");
    }

    #[test]
    fn platform_variant_has_correct_code() {
        let err = AgentBrainError::Platform {
            code: error_codes::E_PLATFORM_PATH_TRAVERSAL,
            message: "path traversal detected".to_string(),
        };
        assert_eq!(err.code(), "E_PLATFORM_PATH_TRAVERSAL");
        assert_eq!(
            format!("{err}"),
            "[E_PLATFORM_PATH_TRAVERSAL] path traversal detected"
        );
    }

    #[test]
    fn filesystem_source_returns_io_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "no such file");
        let err = AgentBrainError::FileSystem {
            code: error_codes::E_FS_NOT_FOUND,
            message: "file not found".to_string(),
            source: Some(io_err),
        };
        assert!(err.source().is_some());
    }

    #[test]
    fn serialization_source_returns_serde_error() {
        let serde_err = serde_json::from_str::<serde_json::Value>("invalid").unwrap_err();
        let err = AgentBrainError::Serialization {
            code: error_codes::E_SER_DESERIALIZE_FAILED,
            message: "deser failed".to_string(),
            source: Some(serde_err),
        };
        assert!(err.source().is_some());
    }

    #[test]
    fn variants_without_source_return_none() {
        let err = AgentBrainError::Configuration {
            code: error_codes::E_CONFIG_INVALID_VALUE,
            message: "bad".to_string(),
        };
        assert!(err.source().is_none());

        let err = AgentBrainError::Lock {
            code: error_codes::E_LOCK_TIMEOUT,
            message: "timeout".to_string(),
        };
        assert!(err.source().is_none());

        let err = AgentBrainError::MemoryCorruption {
            code: error_codes::E_MEM_INVALID_CHECKSUM,
            message: "bad checksum".to_string(),
        };
        assert!(err.source().is_none());

        let err = AgentBrainError::InvalidInput {
            code: error_codes::E_INPUT_OUT_OF_RANGE,
            message: "out of range".to_string(),
        };
        assert!(err.source().is_none());

        let err = AgentBrainError::Platform {
            code: error_codes::E_PLATFORM_MISSING_SESSION_ID,
            message: "no session id".to_string(),
        };
        assert!(err.source().is_none());
    }

    // T022: Error code constant verification tests

    #[test]
    fn all_error_codes_match_their_constant_names() {
        let codes: &[(&str, &str)] = &[
            (error_codes::E_FS_NOT_FOUND, "E_FS_NOT_FOUND"),
            (
                error_codes::E_FS_PERMISSION_DENIED,
                "E_FS_PERMISSION_DENIED",
            ),
            (error_codes::E_FS_IO_ERROR, "E_FS_IO_ERROR"),
            (
                error_codes::E_CONFIG_INVALID_VALUE,
                "E_CONFIG_INVALID_VALUE",
            ),
            (
                error_codes::E_CONFIG_MISSING_FIELD,
                "E_CONFIG_MISSING_FIELD",
            ),
            (error_codes::E_CONFIG_PARSE_ERROR, "E_CONFIG_PARSE_ERROR"),
            (
                error_codes::E_SER_SERIALIZE_FAILED,
                "E_SER_SERIALIZE_FAILED",
            ),
            (
                error_codes::E_SER_DESERIALIZE_FAILED,
                "E_SER_DESERIALIZE_FAILED",
            ),
            (
                error_codes::E_LOCK_ACQUISITION_FAILED,
                "E_LOCK_ACQUISITION_FAILED",
            ),
            (error_codes::E_LOCK_TIMEOUT, "E_LOCK_TIMEOUT"),
            (error_codes::E_MEM_CORRUPTED_INDEX, "E_MEM_CORRUPTED_INDEX"),
            (
                error_codes::E_MEM_INVALID_CHECKSUM,
                "E_MEM_INVALID_CHECKSUM",
            ),
            (error_codes::E_INPUT_EMPTY_FIELD, "E_INPUT_EMPTY_FIELD"),
            (error_codes::E_INPUT_OUT_OF_RANGE, "E_INPUT_OUT_OF_RANGE"),
            (
                error_codes::E_INPUT_INVALID_FORMAT,
                "E_INPUT_INVALID_FORMAT",
            ),
            (
                error_codes::E_PLATFORM_INCOMPATIBLE_CONTRACT,
                "E_PLATFORM_INCOMPATIBLE_CONTRACT",
            ),
            (
                error_codes::E_PLATFORM_INVALID_CONTRACT_VERSION,
                "E_PLATFORM_INVALID_CONTRACT_VERSION",
            ),
            (
                error_codes::E_PLATFORM_MISSING_SESSION_ID,
                "E_PLATFORM_MISSING_SESSION_ID",
            ),
            (
                error_codes::E_PLATFORM_MISSING_PROJECT_IDENTITY,
                "E_PLATFORM_MISSING_PROJECT_IDENTITY",
            ),
            (
                error_codes::E_PLATFORM_PATH_TRAVERSAL,
                "E_PLATFORM_PATH_TRAVERSAL",
            ),
            (
                error_codes::E_PLATFORM_ADAPTER_NOT_FOUND,
                "E_PLATFORM_ADAPTER_NOT_FOUND",
            ),
        ];
        for (actual, expected) in codes {
            assert_eq!(*actual, *expected, "error code constant mismatch");
        }
    }

    // T023: Error::source() chaining tests

    #[test]
    fn filesystem_wraps_io_error_with_source() {
        let io_err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "access denied");
        let err = AgentBrainError::FileSystem {
            code: error_codes::E_FS_PERMISSION_DENIED,
            message: "permission denied".to_string(),
            source: Some(io_err),
        };
        let src = err
            .source()
            .expect("source must be Some for FileSystem with io::Error");
        assert!(
            src.downcast_ref::<std::io::Error>().is_some(),
            "source must downcast to std::io::Error"
        );
    }

    #[test]
    fn serialization_wraps_serde_error_with_source() {
        let serde_err = serde_json::from_str::<serde_json::Value>("not json").unwrap_err();
        let err = AgentBrainError::Serialization {
            code: error_codes::E_SER_DESERIALIZE_FAILED,
            message: "deserialize failed".to_string(),
            source: Some(serde_err),
        };
        let src = err
            .source()
            .expect("source must be Some for Serialization with serde_json::Error");
        assert!(
            src.downcast_ref::<serde_json::Error>().is_some(),
            "source must downcast to serde_json::Error"
        );
    }

    // T024: Cause chain traversal

    #[test]
    fn cause_chain_traversal() {
        // Verify AgentBrainError -> io::Error link
        let io_err = std::io::Error::new(std::io::ErrorKind::Other, "underlying failure");
        let agent_err = AgentBrainError::FileSystem {
            code: error_codes::E_FS_IO_ERROR,
            message: "io failed".to_string(),
            source: Some(io_err),
        };
        let src = agent_err.source().expect("source must be Some");
        assert!(src.downcast_ref::<std::io::Error>().is_some());

        // Verify serde error chain
        let serde_err = serde_json::from_str::<serde_json::Value>("invalid").unwrap_err();
        let agent_err = AgentBrainError::Serialization {
            code: error_codes::E_SER_DESERIALIZE_FAILED,
            message: "deser failed".to_string(),
            source: Some(serde_err),
        };
        let src = agent_err.source().expect("source must be Some");
        assert!(src.downcast_ref::<serde_json::Error>().is_some());
    }

    #[test]
    fn display_format_consistent_for_all_variants() {
        let cases: &[(&str, AgentBrainError)] = &[
            (
                "[E_FS_NOT_FOUND] file missing",
                AgentBrainError::FileSystem {
                    code: error_codes::E_FS_NOT_FOUND,
                    message: "file missing".to_string(),
                    source: None,
                },
            ),
            (
                "[E_CONFIG_INVALID_VALUE] bad value",
                AgentBrainError::Configuration {
                    code: error_codes::E_CONFIG_INVALID_VALUE,
                    message: "bad value".to_string(),
                },
            ),
            (
                "[E_SER_SERIALIZE_FAILED] encode error",
                AgentBrainError::Serialization {
                    code: error_codes::E_SER_SERIALIZE_FAILED,
                    message: "encode error".to_string(),
                    source: None,
                },
            ),
            (
                "[E_LOCK_TIMEOUT] timed out",
                AgentBrainError::Lock {
                    code: error_codes::E_LOCK_TIMEOUT,
                    message: "timed out".to_string(),
                },
            ),
            (
                "[E_MEM_CORRUPTED_INDEX] index bad",
                AgentBrainError::MemoryCorruption {
                    code: error_codes::E_MEM_CORRUPTED_INDEX,
                    message: "index bad".to_string(),
                },
            ),
            (
                "[E_INPUT_INVALID_FORMAT] bad format",
                AgentBrainError::InvalidInput {
                    code: error_codes::E_INPUT_INVALID_FORMAT,
                    message: "bad format".to_string(),
                },
            ),
            (
                "[E_PLATFORM_ADAPTER_NOT_FOUND] no adapter",
                AgentBrainError::Platform {
                    code: error_codes::E_PLATFORM_ADAPTER_NOT_FOUND,
                    message: "no adapter".to_string(),
                },
            ),
        ];

        for (expected, err) in cases {
            assert_eq!(
                format!("{err}"),
                *expected,
                "Display for variant with code '{}' did not match expected format",
                err.code()
            );
        }
    }
}
