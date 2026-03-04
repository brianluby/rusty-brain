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

    /// Memvid storage backend error (wraps memvid-core errors).
    pub const E_STORAGE_BACKEND: &str = "E_STORAGE_BACKEND";
    /// Memory file is corrupted and cannot be opened.
    pub const E_STORAGE_CORRUPTED_FILE: &str = "E_STORAGE_CORRUPTED_FILE";
    /// Memory file exceeds the maximum allowed size.
    pub const E_STORAGE_FILE_TOO_LARGE: &str = "E_STORAGE_FILE_TOO_LARGE";

    /// Unknown or unclassified internal error.
    pub const E_UNKNOWN: &str = "E_UNKNOWN";
}

/// Opaque wrapper for storage backend error messages.
///
/// Used as the `#[source]` in [`RustyBrainError::Storage`] to preserve the
/// `Error::source()` chain when the original error type is not `Send + Sync`.
#[derive(Debug)]
pub struct StorageSource(pub String);

impl std::fmt::Display for StorageSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for StorageSource {}

/// Unified error type for all rusty-brain operations.
///
/// Each variant carries a stable `code` from [`error_codes`] and a
/// human-readable `message`. Some variants also wrap an underlying `source`
/// error for `Error::source()` chaining. The enum is `#[non_exhaustive]` so
/// new variants can be added without a breaking change.
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum RustyBrainError {
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
    /// Memvid storage backend error (wraps memvid-core errors).
    #[error("[{code}] {message}")]
    Storage {
        /// Stable error code ([`error_codes::E_STORAGE_BACKEND`]).
        code: &'static str,
        /// Human-readable description of the failure.
        message: String,
        /// Underlying error source (memvid errors are converted to strings
        /// because the original types may not be `Send + Sync`).
        #[source]
        source: Option<StorageSource>,
    },
    /// Memory file is corrupted and cannot be opened.
    #[error("[{code}] {message}")]
    CorruptedFile {
        /// Stable error code ([`error_codes::E_STORAGE_CORRUPTED_FILE`]).
        code: &'static str,
        /// Human-readable description of the failure.
        message: String,
    },
    /// Memory file exceeds the maximum allowed size.
    #[error("[{code}] {message}")]
    FileTooLarge {
        /// Stable error code ([`error_codes::E_STORAGE_FILE_TOO_LARGE`]).
        code: &'static str,
        /// Human-readable description of the failure.
        message: String,
    },
    /// Lock acquisition timed out after retry attempts.
    #[error("[{code}] {message}")]
    LockTimeout {
        /// Stable error code ([`error_codes::E_LOCK_TIMEOUT`]).
        code: &'static str,
        /// Human-readable description of the failure.
        message: String,
    },
}

/// Backwards-compatible alias for [`RustyBrainError`].
pub type AgentBrainError = RustyBrainError;

impl RustyBrainError {
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
            | Self::Platform { code, .. }
            | Self::Storage { code, .. }
            | Self::CorruptedFile { code, .. }
            | Self::FileTooLarge { code, .. }
            | Self::LockTimeout { code, .. } => code,
        }
    }
}
