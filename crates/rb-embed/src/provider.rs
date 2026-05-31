use async_trait::async_trait;

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
    async fn embed(&self, texts: &[String]) -> rb_types::Result<Vec<Vec<f32>>>;
}
