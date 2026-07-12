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
        Error::IoKind { .. } => "io",
        Error::Embedding(_) => "embedding",
        Error::Enrichment(_) => "enrichment",
        Error::InvalidArgument(_) => "invalid_argument",
        Error::PermissionDenied(_) => "permission_denied",
        Error::StalePlan(_) => "stale_plan",
    }
}

/// Reconstruct a domain error from a wire `kind`/`message`.
///
/// The wire `message` is the sender's rendered `Display` (prefix + inner, e.g.
/// `"invalid argument: …"`). Re-wrapping that whole string in the same variant
/// would re-add the prefix, so reconstruction would `Display` as
/// `"invalid argument: invalid argument: …"`. To keep the round-tripped error's
/// `Display` identical to the original, each same-variant arm strips its own
/// `Display` prefix via [`unprefixed`] before re-wrapping — a no-op when the
/// prefix is absent (e.g. an internal error whose wire message is the opaque
/// `"internal error"` sentinel), so it is always safe.
///
/// Variants that carry structured data which cannot be parsed back from a
/// string (`NotFound`, `DimensionMismatch`) degrade to `Error::Storage`
/// carrying the original message -- faithful text, lossy structure. Unknown
/// kinds also map to `Error::Storage` (fail closed: never silently succeed).
pub fn response_error_to_error(kind: &str, message: &str) -> Error {
    match kind {
        "storage" => Error::Storage(unprefixed(message, "storage error: ")),
        "migration" => Error::Migration(unprefixed(message, "migration error: ")),
        "invalid_namespace" => Error::InvalidNamespace(unprefixed(message, "invalid namespace: ")),
        "invalid_memory_type" => {
            Error::InvalidMemoryType(unprefixed(message, "invalid memory type: "))
        }
        "invalid_link_type" => Error::InvalidLinkType(unprefixed(message, "invalid link type: ")),
        "serialization" => Error::Serialization(unprefixed(message, "serialization error: ")),
        "io" => Error::Io(unprefixed(message, "io error: ")),
        "embedding" => Error::Embedding(unprefixed(message, "embedding error: ")),
        "enrichment" => Error::Enrichment(unprefixed(message, "enrichment error: ")),
        "invalid_argument" => Error::InvalidArgument(unprefixed(message, "invalid argument: ")),
        "permission_denied" => Error::PermissionDenied(unprefixed(message, "permission denied: ")),
        "stale_plan" => Error::StalePlan(unprefixed(message, "stale review plan: ")),
        // not_found / dimension_mismatch / anything unrecognized: preserve the
        // message under Storage rather than fabricate structured fields.
        _ => Error::Storage(message.to_string()),
    }
}

/// Strip a variant's own `Display` prefix from a wire message if present,
/// returning an owned `String`. A no-op (returns the message unchanged) when
/// the prefix is absent, so reconstruction is robust whether the sender
/// rendered the full `Display` or substituted an opaque sentinel.
fn unprefixed(message: &str, prefix: &str) -> String {
    message.strip_prefix(prefix).unwrap_or(message).to_string()
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
    fn stale_plan_round_trips_verbatim() {
        // The stale-plan error is caller GUIDANCE (re-run review), so it must
        // travel verbatim and reconstruct to the same variant — policy sweeps
        // and interactive loops classify on it.
        let err = Error::StalePlan("members were already resolved".to_string());
        let resp = error_to_response(&err);
        match &resp {
            Response::Error { kind, message } => {
                assert_eq!(kind, "stale_plan");
                assert!(message.contains("already resolved"), "{message}");
            }
            other => panic!("unexpected: {other:?}"),
        }
        match round_trip(err) {
            Error::StalePlan(msg) => assert_eq!(msg, "members were already resolved"),
            other => panic!("expected StalePlan back, got {other:?}"),
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
    fn embedding_round_trips() {
        assert!(matches!(
            round_trip(Error::Embedding("provider failed".into())),
            Error::Embedding(_)
        ));
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

    #[test]
    fn enrichment_round_trips_as_enrichment_kind() {
        // error_kind -> "enrichment", and reconstruct must stay Enrichment
        // (fail-closed: no silent downgrade to a different variant).
        let resp = error_to_response(&Error::Enrichment("llm down".into()));
        let crate::Response::Error { kind, message } = resp else {
            panic!("expected Response::Error");
        };
        assert_eq!(kind, "enrichment");
        assert!(matches!(
            response_error_to_error(&kind, &message),
            Error::Enrichment(_)
        ));
    }

    #[test]
    fn invalid_argument_round_trips() {
        assert!(matches!(
            round_trip(Error::InvalidArgument("importance 0".into())),
            Error::InvalidArgument(_)
        ));
    }

    #[test]
    fn permission_denied_round_trips_without_double_prefix() {
        let back = round_trip(Error::PermissionDenied("admin op required".into()));
        assert!(matches!(back, Error::PermissionDenied(_)));
        // The reconstructed error must Display exactly once, not
        // "permission denied: permission denied: …".
        assert_eq!(back.to_string(), "permission denied: admin op required");
    }

    #[test]
    fn same_variant_round_trips_preserve_display_without_doubling_the_prefix() {
        // Every variant that reconstructs to the SAME variant must round-trip
        // with an unchanged Display — no "<prefix>: <prefix>: …" doubling.
        let cases = [
            Error::Storage("disk full".into()),
            Error::Migration("bad step".into()),
            Error::InvalidNamespace("project:".into()),
            Error::InvalidMemoryType("bogus".into()),
            Error::InvalidLinkType("depends_on".into()),
            Error::Serialization("bad json".into()),
            Error::Io("broken pipe".into()),
            Error::Embedding("model down".into()),
            Error::Enrichment("llm 500".into()),
            Error::InvalidArgument("importance 0".into()),
            Error::PermissionDenied("admin op required".into()),
        ];
        for err in cases {
            let want = err.to_string();
            let back = round_trip(err);
            assert_eq!(
                back.to_string(),
                want,
                "round-trip must preserve Display exactly (no doubled prefix)"
            );
        }
    }
}
