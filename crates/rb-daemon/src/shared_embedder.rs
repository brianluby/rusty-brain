use std::sync::Arc;

use async_trait::async_trait;
use rb_embed::{EmbedKind, EmbeddingProvider};
use rb_types::Result;
use tokio::sync::Semaphore;

const DEFAULT_MAX_CONCURRENT_EMBEDDINGS: usize = 4;

/// Reference-counted, cloneable embedding provider shared by all connections.
#[derive(Clone)]
pub struct SharedEmbedder {
    inner: Arc<dyn EmbeddingProvider>,
    permits: Arc<Semaphore>,
}

impl SharedEmbedder {
    /// Wrap any concrete provider.
    pub fn new<P: EmbeddingProvider + 'static>(provider: P) -> Self {
        Self::with_concurrency_limit(provider, DEFAULT_MAX_CONCURRENT_EMBEDDINGS)
    }

    /// Wrap any concrete provider and cap concurrent `embed` calls.
    pub fn with_concurrency_limit<P: EmbeddingProvider + 'static>(
        provider: P,
        max_concurrent: usize,
    ) -> Self {
        Self {
            inner: Arc::new(provider),
            permits: Arc::new(Semaphore::new(max_concurrent.max(1))),
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

    /// Pure delegation: the kind is forwarded untouched to the wrapped
    /// provider (only the provider knows whether it differentiates).
    async fn embed(&self, texts: &[String], kind: EmbedKind) -> Result<Vec<Vec<f32>>> {
        let _permit = self
            .permits
            .acquire()
            .await
            .map_err(|_| rb_types::Error::Embedding("embedder semaphore closed".to_string()))?;
        self.inner.embed(texts, kind).await
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use rb_embed::{DeterministicProvider, EmbeddingProvider};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::time::{sleep, Duration};

    struct SlowProvider {
        active: Arc<AtomicUsize>,
        max_seen: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl EmbeddingProvider for SlowProvider {
        fn model_id(&self) -> &str {
            "slow-test"
        }

        fn dim(&self) -> usize {
            2
        }

        // Test-only stub: the kind is irrelevant to concurrency accounting.
        async fn embed(&self, texts: &[String], _kind: EmbedKind) -> Result<Vec<Vec<f32>>> {
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_seen.fetch_max(active, Ordering::SeqCst);
            sleep(Duration::from_millis(50)).await;
            self.active.fetch_sub(1, Ordering::SeqCst);
            Ok(vec![vec![1.0, 0.0]; texts.len()])
        }
    }

    /// Captures the kinds passed to `embed`, in call order, through a handle
    /// that survives the provider moving into the SharedEmbedder's Arc (W1.4).
    struct KindCapturingProvider {
        kinds: Arc<std::sync::Mutex<Vec<EmbedKind>>>,
    }

    #[async_trait]
    impl EmbeddingProvider for KindCapturingProvider {
        fn model_id(&self) -> &str {
            "kind-capturing"
        }

        fn dim(&self) -> usize {
            2
        }

        async fn embed(&self, texts: &[String], kind: EmbedKind) -> Result<Vec<Vec<f32>>> {
            self.kinds
                .lock()
                .map_err(|_| rb_types::Error::Embedding("kind log poisoned".to_string()))?
                .push(kind);
            Ok(vec![vec![0.0, 0.0]; texts.len()])
        }
    }

    #[tokio::test]
    async fn shared_embedder_forwards_the_embed_kind_untouched() {
        let kinds = Arc::new(std::sync::Mutex::new(Vec::new()));
        let shared = SharedEmbedder::new(KindCapturingProvider {
            kinds: Arc::clone(&kinds),
        });
        shared
            .embed(&["q".to_string()], EmbedKind::Query)
            .await
            .unwrap();
        shared
            .embed(&["d".to_string()], EmbedKind::Document)
            .await
            .unwrap();
        let seen = kinds.lock().unwrap().clone();
        assert_eq!(seen, vec![EmbedKind::Query, EmbedKind::Document]);
    }

    #[tokio::test]
    async fn shared_embedder_delegates_dim_and_embed() {
        let inner = DeterministicProvider::new(8);
        let model = inner.model_id().to_string();
        let shared = SharedEmbedder::new(inner);
        assert_eq!(shared.dim(), 8);
        assert_eq!(shared.model_id(), model);
        let out = shared
            .embed(&["hello".to_string()], EmbedKind::Document)
            .await
            .unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].len(), 8);
    }

    #[tokio::test]
    async fn cloning_shares_one_instance() {
        let shared = SharedEmbedder::new(DeterministicProvider::new(8));
        let clone = shared.clone();
        let a = shared
            .embed(&["same".to_string()], EmbedKind::Document)
            .await
            .unwrap();
        let b = clone
            .embed(&["same".to_string()], EmbedKind::Document)
            .await
            .unwrap();
        assert_eq!(a, b);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shared_embedder_limits_concurrent_embed_calls() {
        let active = Arc::new(AtomicUsize::new(0));
        let max_seen = Arc::new(AtomicUsize::new(0));
        let shared = SharedEmbedder::with_concurrency_limit(
            SlowProvider {
                active: Arc::clone(&active),
                max_seen: Arc::clone(&max_seen),
            },
            1,
        );

        let a = shared.clone();
        let b = shared.clone();
        let first =
            tokio::spawn(async move { a.embed(&["a".to_string()], EmbedKind::Document).await });
        let second =
            tokio::spawn(async move { b.embed(&["b".to_string()], EmbedKind::Document).await });

        first.await.unwrap().unwrap();
        second.await.unwrap().unwrap();

        assert_eq!(max_seen.load(Ordering::SeqCst), 1);
    }
}
