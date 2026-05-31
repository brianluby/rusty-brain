//! Stable mapping between `rb_types::Error` and `Response::Error`.
//!
//! `kind` strings are part of the wire contract and must stay stable across
//! versions. The daemon maps domain errors out; the client maps them back.

use crate::Response;
use rb_types::Error;

/// Map a domain error into a wire `Response::Error`. The `kind` is a stable
/// identifier; `message` is the human-readable `Display` form (no internal
/// detail beyond what `rb_types::Error` already exposes).
pub fn error_to_response(err: &Error) -> Response {
    let kind = error_kind(err);
    Response::Error {
        kind: kind.to_string(),
        message: err.to_string(),
    }
}

/// The stable wire `kind` string for a domain error.
fn error_kind(err: &Error) -> &'static str {
    match err {
        Error::Storage(_) => "storage",
        Error::Migration(_) => "migration",
        Error::NotFound(_) => "not_found",
        Error::InvalidNamespace(_) => "invalid_namespace",
        Error::InvalidMemoryType(_) => "invalid_memory_type",
        Error::InvalidLinkType(_) => "invalid_link_type",
        Error::Serialization(_) => "serialization",
        Error::DimensionMismatch { .. } => "dimension_mismatch",
        Error::Io(_) => "io",
    }
}

/// Reconstruct a domain error from a wire `kind`/`message`.
///
/// Variants that carry structured data which cannot be parsed back from a
/// string (`NotFound`, `DimensionMismatch`) degrade to `Error::Storage`
/// carrying the original message -- faithful text, lossy structure. Unknown
/// kinds also map to `Error::Storage` (fail closed: never silently succeed).
pub fn response_error_to_error(kind: &str, message: &str) -> Error {
    match kind {
        "storage" => Error::Storage(message.to_string()),
        "migration" => Error::Migration(message.to_string()),
        "invalid_namespace" => Error::InvalidNamespace(message.to_string()),
        "invalid_memory_type" => Error::InvalidMemoryType(message.to_string()),
        "invalid_link_type" => Error::InvalidLinkType(message.to_string()),
        "serialization" => Error::Serialization(message.to_string()),
        "io" => Error::Io(message.to_string()),
        // not_found / dimension_mismatch / anything unrecognized: preserve the
        // message under Storage rather than fabricate structured fields.
        _ => Error::Storage(message.to_string()),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use crate::Response;
    use rb_types::{Error, MemoryId};

    fn round_trip(err: Error) -> Error {
        let resp = error_to_response(&err);
        match resp {
            Response::Error { kind, message } => response_error_to_error(&kind, &message),
            other => panic!("expected Response::Error, got {other:?}"),
        }
    }

    #[test]
    fn not_found_maps_to_stable_kind() {
        let id = MemoryId::new();
        let resp = error_to_response(&Error::NotFound(id.clone()));
        match resp {
            Response::Error { kind, message } => {
                assert_eq!(kind, "not_found");
                assert!(message.contains(&id.to_string()));
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn storage_round_trips_to_storage() {
        assert!(matches!(
            round_trip(Error::Storage("disk".into())),
            Error::Storage(_)
        ));
    }

    #[test]
    fn migration_round_trips_to_migration() {
        assert!(matches!(
            round_trip(Error::Migration("bad".into())),
            Error::Migration(_)
        ));
    }

    #[test]
    fn invalid_namespace_round_trips() {
        assert!(matches!(
            round_trip(Error::InvalidNamespace("x".into())),
            Error::InvalidNamespace(_)
        ));
    }

    #[test]
    fn invalid_memory_type_round_trips() {
        assert!(matches!(
            round_trip(Error::InvalidMemoryType("zz".into())),
            Error::InvalidMemoryType(_)
        ));
    }

    #[test]
    fn invalid_link_type_round_trips() {
        assert!(matches!(
            round_trip(Error::InvalidLinkType("qq".into())),
            Error::InvalidLinkType(_)
        ));
    }

    #[test]
    fn serialization_round_trips() {
        assert!(matches!(
            round_trip(Error::Serialization("json".into())),
            Error::Serialization(_)
        ));
    }

    #[test]
    fn io_round_trips() {
        assert!(matches!(round_trip(Error::Io("eof".into())), Error::Io(_)));
    }

    #[test]
    fn dimension_mismatch_round_trips_to_storage_with_detail() {
        // DimensionMismatch carries structured fields that cannot be reconstructed
        // from a string; it degrades to Storage carrying the human message, which
        // is the documented, lossy-but-faithful behavior.
        let err = Error::DimensionMismatch {
            expected: 1024,
            got: 768,
        };
        let resp = error_to_response(&err);
        match resp {
            Response::Error { kind, message } => {
                assert_eq!(kind, "dimension_mismatch");
                assert!(message.contains("1024") && message.contains("768"));
                let back = response_error_to_error(&kind, &message);
                assert!(matches!(back, Error::Storage(_)));
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn unknown_kind_maps_to_storage() {
        let back = response_error_to_error("totally_unknown_kind", "weird");
        assert!(matches!(back, Error::Storage(_)));
    }
}
