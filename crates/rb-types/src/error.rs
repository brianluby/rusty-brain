use crate::memory_id::MemoryId;

/// Domain error type for rusty-brain. All library crates return `Result<T, Error>`.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("storage error: {0}")]
    Storage(String),
    #[error("migration error: {0}")]
    Migration(String),
    #[error("memory not found: {0}")]
    NotFound(MemoryId),
    #[error("invalid namespace: {0}")]
    InvalidNamespace(String),
    #[error("invalid memory type: {0}")]
    InvalidMemoryType(String),
    #[error("invalid link type: {0}")]
    InvalidLinkType(String),
    #[error("serialization error: {0}")]
    Serialization(String),
    #[error("embedding dimension mismatch: expected {expected}, got {got}")]
    DimensionMismatch { expected: usize, got: usize },
    #[error("io error: {0}")]
    Io(String),
    #[error("embedding error: {0}")]
    Embedding(String),
    #[error("enrichment error: {0}")]
    Enrichment(String),
}

/// Convenience alias used throughout rusty-brain.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::unnecessary_literal_unwrap
    )]
    use super::*;
    use crate::memory_id::MemoryId;

    #[test]
    fn display_messages_match_spine() {
        assert_eq!(
            Error::Storage("disk".into()).to_string(),
            "storage error: disk"
        );
        assert_eq!(
            Error::Migration("bad".into()).to_string(),
            "migration error: bad"
        );
        assert_eq!(
            Error::InvalidNamespace("x".into()).to_string(),
            "invalid namespace: x"
        );
        assert_eq!(
            Error::InvalidMemoryType("zz".into()).to_string(),
            "invalid memory type: zz"
        );
        assert_eq!(
            Error::InvalidLinkType("qq".into()).to_string(),
            "invalid link type: qq"
        );
        assert_eq!(
            Error::Serialization("json".into()).to_string(),
            "serialization error: json"
        );
        assert_eq!(Error::Io("eof".into()).to_string(), "io error: eof");
        assert_eq!(
            Error::Embedding("provider down".into()).to_string(),
            "embedding error: provider down"
        );
    }

    #[test]
    fn dimension_mismatch_message() {
        let e = Error::DimensionMismatch {
            expected: 1024,
            got: 768,
        };
        assert_eq!(
            e.to_string(),
            "embedding dimension mismatch: expected 1024, got 768"
        );
    }

    #[test]
    fn not_found_message_uses_memory_id_display() {
        let id = MemoryId::new();
        let e = Error::NotFound(id.clone());
        assert_eq!(e.to_string(), format!("memory not found: {id}"));
    }

    #[test]
    fn result_alias_resolves() {
        let ok: Result<u8> = Ok(7);
        assert_eq!(ok.unwrap(), 7);
    }

    #[test]
    fn enrichment_message_matches_spine() {
        assert_eq!(
            Error::Enrichment("model unavailable".into()).to_string(),
            "enrichment error: model unavailable"
        );
    }
}
