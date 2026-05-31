use std::sync::Arc;

use async_trait::async_trait;
use rb_embed::EmbeddingProvider;
use rb_types::Result;

/// Reference-counted, cloneable embedding provider shared by all connections.
#[derive(Clone)]
pub struct SharedEmbedder {
    inner: Arc<dyn EmbeddingProvider>,
}

impl SharedEmbedder {
    /// Wrap any concrete provider.
    pub fn new<P: EmbeddingProvider + 'static>(provider: P) -> Self {
        Self {
            inner: Arc::new(provider),
        }
    }
}

#[async_trait]
impl EmbeddingProvider for SharedEmbedder {
    fn model_id(&self) -> &str {
        self.inner.model_id()
    }

    fn dim(&self) -> usize {
        self.inner.dim()
    }

    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        self.inner.embed(texts).await
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use rb_embed::{DeterministicProvider, EmbeddingProvider};

    #[tokio::test]
    async fn shared_embedder_delegates_dim_and_embed() {
        let inner = DeterministicProvider::new(8);
        let model = inner.model_id().to_string();
        let shared = SharedEmbedder::new(inner);
        assert_eq!(shared.dim(), 8);
        assert_eq!(shared.model_id(), model);
        let out = shared.embed(&["hello".to_string()]).await.unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].len(), 8);
    }

    #[tokio::test]
    async fn cloning_shares_one_instance() {
        let shared = SharedEmbedder::new(DeterministicProvider::new(8));
        let clone = shared.clone();
        let a = shared.embed(&["same".to_string()]).await.unwrap();
        let b = clone.embed(&["same".to_string()]).await.unwrap();
        assert_eq!(a, b);
    }
}
