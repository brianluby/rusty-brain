use rb_proto::Response;
use rb_types::Error;

/// Map a domain error to a stable wire error response.
pub(crate) fn error_to_response(err: Error) -> Response {
    let kind = match &err {
        Error::Storage(_) => "storage",
        Error::Migration(_) => "migration",
        Error::NotFound(_) => "not_found",
        Error::InvalidNamespace(_) => "invalid_namespace",
        Error::InvalidMemoryType(_) => "invalid_memory_type",
        Error::InvalidLinkType(_) => "invalid_link_type",
        Error::Serialization(_) => "serialization",
        Error::DimensionMismatch { .. } => "dimension_mismatch",
        Error::Io(_) => "io",
        Error::Embedding(_) => "embedding",
    };
    Response::Error {
        kind: kind.to_string(),
        message: err.to_string(),
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
            (Error::Embedding("x".into()), "embedding"),
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
    fn message_does_not_leak_struct_internals() {
        let r = error_to_response(Error::Storage("disk full".into()));
        if let Response::Error { message, .. } = r {
            assert_eq!(message, "storage error: disk full");
        } else {
            panic!("expected error response");
        }
    }
}
