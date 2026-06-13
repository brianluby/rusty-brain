use rb_proto::Response;
use rb_types::Error;
use tracing::warn;

/// Map a domain error to a stable wire error response.
///
/// Client-safe errors (validation / not-found variants) pass their message
/// through. Internal errors (storage, I/O, migration, serialization,
/// embedding) are replaced with a generic "internal error" message; the real
/// detail is logged server-side so it never leaks to the caller.
pub(crate) fn error_to_response(err: Error) -> Response {
    let (kind, message) = match &err {
        // Client-safe: the message is meaningful to the caller and contains
        // no internal path or infrastructure detail.
        Error::NotFound(_) => ("not_found", err.to_string()),
        Error::InvalidNamespace(_) => ("invalid_namespace", err.to_string()),
        Error::InvalidMemoryType(_) => ("invalid_memory_type", err.to_string()),
        Error::InvalidLinkType(_) => ("invalid_link_type", err.to_string()),
        Error::DimensionMismatch { .. } => ("dimension_mismatch", err.to_string()),
        Error::InvalidArgument(_) => ("invalid_argument", err.to_string()),
        Error::PermissionDenied(_) => ("permission_denied", err.to_string()),

        // Internal: log the real detail, send a generic message to the client.
        Error::Storage(_) => {
            warn!(error = %err, "internal storage error");
            ("storage", "internal error".to_string())
        }
        Error::Io(_) => {
            warn!(error = %err, "internal io error");
            ("io", "internal error".to_string())
        }
        Error::IoKind { .. } => {
            warn!(error = %err, "internal io error");
            ("io", "internal error".to_string())
        }
        Error::Migration(_) => {
            warn!(error = %err, "internal migration error");
            ("migration", "internal error".to_string())
        }
        Error::Serialization(_) => {
            warn!(error = %err, "internal serialization error");
            ("serialization", "internal error".to_string())
        }
        Error::Embedding(_) => {
            warn!(error = %err, "internal embedding error");
            ("embedding", "internal error".to_string())
        }
        Error::Enrichment(_) => {
            // F26: like every other internal arm, the real detail goes to the
            // server log (the wire still gets only the opaque sentinel).
            warn!(error = %err, "internal enrichment error");
            ("enrichment", "internal error".to_string())
        }
    };
    Response::Error {
        kind: kind.to_string(),
        message,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use rb_proto::Response;
    use rb_types::{Error, MemoryId};

    #[test]
    fn maps_each_error_to_stable_kind() {
        let cases: Vec<(Error, &str)> = vec![
            (Error::Storage("x".into()), "storage"),
            (Error::Migration("x".into()), "migration"),
            (Error::NotFound(MemoryId::new()), "not_found"),
            (Error::InvalidNamespace("x".into()), "invalid_namespace"),
            (Error::InvalidMemoryType("x".into()), "invalid_memory_type"),
            (Error::InvalidLinkType("x".into()), "invalid_link_type"),
            (Error::Serialization("x".into()), "serialization"),
            (
                Error::DimensionMismatch {
                    expected: 1,
                    got: 2,
                },
                "dimension_mismatch",
            ),
            (Error::Io("x".into()), "io"),
            (
                Error::IoKind {
                    kind: std::io::ErrorKind::NotFound,
                    message: "x".into(),
                },
                "io",
            ),
            (Error::Embedding("x".into()), "embedding"),
            (Error::Enrichment("x".into()), "enrichment"),
            (Error::InvalidArgument("x".into()), "invalid_argument"),
            (Error::PermissionDenied("x".into()), "permission_denied"),
        ];
        for (err, expected_kind) in cases {
            match error_to_response(err) {
                Response::Error { kind, message } => {
                    assert_eq!(kind, expected_kind);
                    assert!(!message.is_empty(), "message is populated");
                }
                other => panic!("expected Response::Error, got {other:?}"),
            }
        }
    }

    #[test]
    fn enrichment_is_internal_opaque_over_wire() {
        // Enrichment is an internal fault: the stable kind is forwarded but the
        // detail is replaced with the generic sentinel (no leak to the caller).
        let r = error_to_response(Error::Enrichment("llm 401: bad key abc".into()));
        if let Response::Error { kind, message } = r {
            assert_eq!(kind, "enrichment");
            assert_eq!(message, "internal error");
            assert!(!message.contains("abc"), "internal detail must not leak");
        } else {
            panic!("expected error response");
        }
    }

    #[test]
    fn io_kind_is_internal_opaque_over_wire() {
        // IoKind carries a std::io::Error-derived message that may embed a path;
        // it is mapped as an internal fault, so the wire message is the opaque
        // sentinel and the sensitive path must not leak to the caller.
        let r = error_to_response(Error::IoKind {
            kind: std::io::ErrorKind::ConnectionRefused,
            message: "/secret/path/to/sock: connection refused".into(),
        });
        if let Response::Error { kind, message } = r {
            assert_eq!(kind, "io");
            assert_eq!(message, "internal error");
            assert!(
                !message.contains("/secret/path"),
                "internal io path must not leak over the wire"
            );
        } else {
            panic!("expected error response");
        }
    }

    #[test]
    fn internal_errors_do_not_leak_detail_over_wire() {
        // Storage errors with internal detail must not expose that detail
        // in the wire message — only the generic "internal error" sentinel.
        let r = error_to_response(Error::Storage("/secret/path: permission denied".into()));
        if let Response::Error { message, kind } = r {
            assert_eq!(kind, "storage");
            assert_eq!(
                message, "internal error",
                "wire message must not contain internal Storage detail"
            );
            assert!(
                !message.contains("/secret/path"),
                "internal path must not appear in wire message"
            );
        } else {
            panic!("expected error response");
        }

        // InvalidArgument is validation-class: its message IS the guidance the
        // client needs (e.g. the content-update rejection) and must pass verbatim.
        let guidance = "content updates are not supported; create a new memory so \
                        embeddings stay consistent";
        let r = error_to_response(Error::InvalidArgument(guidance.into()));
        if let Response::Error { message, kind } = r {
            assert_eq!(kind, "invalid_argument");
            assert_eq!(
                message,
                format!("invalid argument: {guidance}"),
                "validation message must be forwarded verbatim"
            );
        } else {
            panic!("expected error response");
        }
    }

    #[test]
    fn permission_denied_is_client_safe_guidance() {
        // W2.6: an authorization denial is guidance for the caller (what to do
        // to be allowed), not an internal fault — it travels verbatim.
        let msg = "this is an admin operation: it requires a kernel-verified \
                   peer uid matching the daemon's user";
        let r = error_to_response(Error::PermissionDenied(msg.into()));
        if let Response::Error { kind, message } = r {
            assert_eq!(kind, "permission_denied");
            assert_eq!(message, format!("permission denied: {msg}"));
        } else {
            panic!("expected error response");
        }
    }

    #[test]
    fn client_safe_errors_pass_through_message() {
        // NotFound, InvalidNamespace, InvalidMemoryType, InvalidLinkType,
        // DimensionMismatch, and InvalidArgument are safe to forward verbatim.
        let id = MemoryId::new();
        let cases: Vec<Error> = vec![
            Error::NotFound(id),
            Error::InvalidNamespace("bad-ns".into()),
            Error::InvalidMemoryType("bad-type".into()),
            Error::InvalidLinkType("bad-link".into()),
            Error::DimensionMismatch {
                expected: 512,
                got: 768,
            },
            Error::InvalidArgument("importance 0 is out of range 1..=10".into()),
        ];
        for err in cases {
            let display = err.to_string();
            if let Response::Error { message, .. } = error_to_response(err) {
                assert_eq!(
                    message, display,
                    "client-safe error message must be forwarded verbatim"
                );
            } else {
                panic!("expected Response::Error");
            }
        }
    }
}
