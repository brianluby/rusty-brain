//! Tests for [`types::RustyBrainError`] — variant codes, display format, and
//! `Error::source()` chaining.
//!
//! Moved from `crates/types/src/error.rs` inline tests (RB-ARCH-009).

use std::error::Error;

use types::error::error_codes;
use types::{AgentBrainError, RustyBrainError, StorageSource};

// -------------------------------------------------------------------------
// T003: Unit tests for AgentBrainError
// -------------------------------------------------------------------------

#[test]
fn filesystem_variant_has_correct_code() {
    let err = RustyBrainError::FileSystem {
        code: error_codes::E_FS_NOT_FOUND,
        message: "file not found".to_string(),
        source: None,
    };
    assert_eq!(err.code(), "E_FS_NOT_FOUND");
    assert_eq!(format!("{err}"), "[E_FS_NOT_FOUND] file not found");
}

#[test]
fn configuration_variant_has_correct_code() {
    let err = RustyBrainError::Configuration {
        code: error_codes::E_CONFIG_INVALID_VALUE,
        message: "bad config".to_string(),
    };
    assert_eq!(err.code(), "E_CONFIG_INVALID_VALUE");
    assert_eq!(format!("{err}"), "[E_CONFIG_INVALID_VALUE] bad config");
}

#[test]
fn serialization_variant_has_correct_code() {
    let err = RustyBrainError::Serialization {
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
    let err = RustyBrainError::Lock {
        code: error_codes::E_LOCK_ACQUISITION_FAILED,
        message: "lock failed".to_string(),
    };
    assert_eq!(err.code(), "E_LOCK_ACQUISITION_FAILED");
    assert_eq!(format!("{err}"), "[E_LOCK_ACQUISITION_FAILED] lock failed");
}

#[test]
fn memory_corruption_variant_has_correct_code() {
    let err = RustyBrainError::MemoryCorruption {
        code: error_codes::E_MEM_CORRUPTED_INDEX,
        message: "index corrupt".to_string(),
    };
    assert_eq!(err.code(), "E_MEM_CORRUPTED_INDEX");
    assert_eq!(format!("{err}"), "[E_MEM_CORRUPTED_INDEX] index corrupt");
}

#[test]
fn invalid_input_variant_has_correct_code() {
    let err = RustyBrainError::InvalidInput {
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
    let err = RustyBrainError::FileSystem {
        code: error_codes::E_FS_NOT_FOUND,
        message: "file not found".to_string(),
        source: Some(io_err),
    };
    assert!(err.source().is_some());
}

#[test]
fn serialization_source_returns_serde_error() {
    let serde_err = serde_json::from_str::<serde_json::Value>("invalid").unwrap_err();
    let err = RustyBrainError::Serialization {
        code: error_codes::E_SER_DESERIALIZE_FAILED,
        message: "deser failed".to_string(),
        source: Some(serde_err),
    };
    assert!(err.source().is_some());
}

#[test]
fn storage_source_returns_storage_source_error() {
    let err = RustyBrainError::Storage {
        code: error_codes::E_STORAGE_BACKEND,
        message: "backend failed".to_string(),
        source: Some(StorageSource("memvid error".to_string())),
    };
    let src = err
        .source()
        .expect("source must be Some for Storage with StorageSource");
    assert!(
        src.downcast_ref::<StorageSource>().is_some(),
        "source must downcast to StorageSource"
    );
}

#[test]
fn storage_without_source_returns_none() {
    let err = RustyBrainError::Storage {
        code: error_codes::E_STORAGE_BACKEND,
        message: "backend failed".to_string(),
        source: None,
    };
    assert!(err.source().is_none());
}

#[test]
fn variants_without_source_return_none() {
    let err = RustyBrainError::Configuration {
        code: error_codes::E_CONFIG_INVALID_VALUE,
        message: "bad".to_string(),
    };
    assert!(err.source().is_none());

    let err = RustyBrainError::Lock {
        code: error_codes::E_LOCK_TIMEOUT,
        message: "timeout".to_string(),
    };
    assert!(err.source().is_none());

    let err = RustyBrainError::MemoryCorruption {
        code: error_codes::E_MEM_INVALID_CHECKSUM,
        message: "bad checksum".to_string(),
    };
    assert!(err.source().is_none());

    let err = RustyBrainError::InvalidInput {
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

// -------------------------------------------------------------------------
// T022: Error code constant verification tests
// -------------------------------------------------------------------------

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
        (error_codes::E_STORAGE_BACKEND, "E_STORAGE_BACKEND"),
        (
            error_codes::E_STORAGE_CORRUPTED_FILE,
            "E_STORAGE_CORRUPTED_FILE",
        ),
        (
            error_codes::E_STORAGE_FILE_TOO_LARGE,
            "E_STORAGE_FILE_TOO_LARGE",
        ),
        (error_codes::E_UNKNOWN, "E_UNKNOWN"),
    ];
    for (actual, expected) in codes {
        assert_eq!(*actual, *expected, "error code constant mismatch");
    }
}

// -------------------------------------------------------------------------
// T023: Error::source() chaining tests
// -------------------------------------------------------------------------

#[test]
fn filesystem_wraps_io_error_with_source() {
    let io_err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "access denied");
    let err = RustyBrainError::FileSystem {
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
    let err = RustyBrainError::Serialization {
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

// -------------------------------------------------------------------------
// T024: Cause chain traversal
// -------------------------------------------------------------------------

#[test]
fn cause_chain_traversal() {
    // Verify AgentBrainError -> io::Error link
    let io_err = std::io::Error::new(std::io::ErrorKind::Other, "underlying failure");
    let agent_err = RustyBrainError::FileSystem {
        code: error_codes::E_FS_IO_ERROR,
        message: "io failed".to_string(),
        source: Some(io_err),
    };
    let src = agent_err.source().expect("source must be Some");
    assert!(src.downcast_ref::<std::io::Error>().is_some());

    // Verify serde error chain
    let serde_err = serde_json::from_str::<serde_json::Value>("invalid").unwrap_err();
    let agent_err = RustyBrainError::Serialization {
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
            RustyBrainError::FileSystem {
                code: error_codes::E_FS_NOT_FOUND,
                message: "file missing".to_string(),
                source: None,
            },
        ),
        (
            "[E_CONFIG_INVALID_VALUE] bad value",
            RustyBrainError::Configuration {
                code: error_codes::E_CONFIG_INVALID_VALUE,
                message: "bad value".to_string(),
            },
        ),
        (
            "[E_SER_SERIALIZE_FAILED] encode error",
            RustyBrainError::Serialization {
                code: error_codes::E_SER_SERIALIZE_FAILED,
                message: "encode error".to_string(),
                source: None,
            },
        ),
        (
            "[E_LOCK_TIMEOUT] timed out",
            RustyBrainError::Lock {
                code: error_codes::E_LOCK_TIMEOUT,
                message: "timed out".to_string(),
            },
        ),
        (
            "[E_MEM_CORRUPTED_INDEX] index bad",
            RustyBrainError::MemoryCorruption {
                code: error_codes::E_MEM_CORRUPTED_INDEX,
                message: "index bad".to_string(),
            },
        ),
        (
            "[E_INPUT_INVALID_FORMAT] bad format",
            RustyBrainError::InvalidInput {
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
        (
            "[E_STORAGE_BACKEND] backend failed",
            RustyBrainError::Storage {
                code: error_codes::E_STORAGE_BACKEND,
                message: "backend failed".to_string(),
                source: None,
            },
        ),
        (
            "[E_STORAGE_CORRUPTED_FILE] file corrupt",
            RustyBrainError::CorruptedFile {
                code: error_codes::E_STORAGE_CORRUPTED_FILE,
                message: "file corrupt".to_string(),
            },
        ),
        (
            "[E_STORAGE_FILE_TOO_LARGE] too big",
            RustyBrainError::FileTooLarge {
                code: error_codes::E_STORAGE_FILE_TOO_LARGE,
                message: "too big".to_string(),
            },
        ),
        (
            "[E_LOCK_TIMEOUT] lock timed out",
            RustyBrainError::LockTimeout {
                code: error_codes::E_LOCK_TIMEOUT,
                message: "lock timed out".to_string(),
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
