//! Optimistic guard for asynchronous embedding work. Not a wire or DB field.
use crate::MemoryNote;
use sha2::{Digest, Sha256};

/// Fingerprint of the raw fields consumed by document embedding: content,
/// keywords, tags, and context. Compare a candidate's fingerprint with the live
/// row inside the vector-write transaction, never with the stale version stamp.
///
/// Length prefixes preserve field/list boundaries and order. Hashing raw values
/// is conservative: even edits normalized away by the composer cause a retry.
/// Metadata, model/version stamps, and timestamps intentionally do not participate.
/// Keep this field set in sync with `rb_engine::embedding_input`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EmbeddingInputFingerprint([u8; 32]);

impl From<&MemoryNote> for EmbeddingInputFingerprint {
    fn from(note: &MemoryNote) -> Self {
        let mut hash = Sha256::new();
        // Treat scalar fields as one-element sequences for unambiguous framing.
        for fields in [
            std::slice::from_ref(&note.content),
            note.keywords.as_slice(),
            note.tags.as_slice(),
            std::slice::from_ref(&note.context),
        ] {
            hash.update((fields.len() as u64).to_le_bytes());
            for field in fields {
                hash.update((field.len() as u64).to_le_bytes());
                hash.update(field.as_bytes());
            }
        }
        Self(hash.finalize().into())
    }
}
