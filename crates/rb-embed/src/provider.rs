use async_trait::async_trait;

/// What the text being embedded *is*, from the retrieval model's point of view
/// (W1.4). Asymmetric providers (Voyage's `input_type`) condition the vector on
/// this; symmetric providers MUST ignore it so query and document vectors stay
/// in one space.
///
/// Recall paths (user queries) pass [`EmbedKind::Query`]; remember / reembed /
/// consolidation paths (stored text) pass [`EmbedKind::Document`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EmbedKind {
    /// A retrieval query (recall side).
    Query,
    /// A stored document (write side). This is the kind every persisted vector
    /// was produced with.
    Document,
}

impl EmbedKind {
    /// Canonical lowercase string: `"query"` / `"document"`. Doubles as the
    /// Voyage `input_type` request value and the record/replay fixture key
    /// component (`(model_id, input_kind, sha256(text))`).
    pub fn as_str(self) -> &'static str {
        match self {
            EmbedKind::Query => "query",
            EmbedKind::Document => "document",
        }
    }
}

/// A source of embedding vectors. Implementations may be remote (Voyage) or
/// local/offline (deterministic). All implementations are `Send + Sync` so the
/// daemon can share a single provider across connection tasks.
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Stable identifier of the model, stored on each memory as `embedding_model`.
    fn model_id(&self) -> &str;

    /// The fixed embedding dimension. Enforced against `meta.embedding_dim` at init.
    fn dim(&self) -> usize;

    /// Embed each input text, returning one vector per input **in input order**.
    /// Every returned vector has length `self.dim()`.
    ///
    /// `kind` declares whether the texts are retrieval queries or stored
    /// documents (W1.4). Providers without query/document asymmetry MUST
    /// ignore it (and say why in a comment) so persisted corpora stay valid.
    async fn embed(&self, texts: &[String], kind: EmbedKind) -> rb_types::Result<Vec<Vec<f32>>>;
}

#[cfg(test)]
mod tests {
    use super::EmbedKind;

    #[test]
    fn kind_strings_are_the_canonical_wire_and_fixture_values() {
        // These exact strings are Voyage's `input_type` values AND the
        // record/replay fixture `input_kind` key component — never rename.
        assert_eq!(EmbedKind::Query.as_str(), "query");
        assert_eq!(EmbedKind::Document.as_str(), "document");
    }
}
