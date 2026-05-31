use crate::backend::MemoryBackend;
use crate::enrich::{default_summary, derive_keywords};
use rb_embed::EmbeddingProvider;
use rb_search::Weights;
use rb_types::{MemoryId, MemoryNote, MemoryType, Namespace};

/// Input to `remember`. Mirrors the proto `Request::Remember` payload.
pub struct RememberInput {
    pub content: String,
    pub context: Option<String>,
    pub memory_type: MemoryType,
    pub importance: u8,
    pub keywords: Vec<String>,
    pub tags: Vec<String>,
    pub related_files: Vec<String>,
}

/// Policy layer: orchestrates heuristic enrichment + embedding + ranking over a
/// `MemoryBackend`. Generic so it is unit-tested without a DB or network.
pub struct MemoryEngine<B: MemoryBackend, P: EmbeddingProvider> {
    backend: B,
    embedder: P,
    weights: Weights,
    namespace: Namespace,
}

impl<B: MemoryBackend, P: EmbeddingProvider> MemoryEngine<B, P> {
    /// Construct an engine bound to a single namespace (set server-side from the
    /// client handshake; clients cannot widen it).
    pub fn new(backend: B, embedder: P, namespace: Namespace) -> Self {
        Self {
            backend,
            embedder,
            weights: Weights::default(),
            namespace,
        }
    }

    /// Borrow the backend (used by daemon/tests for introspection).
    pub fn backend(&self) -> &B {
        &self.backend
    }

    /// Borrow the embedding provider.
    pub fn embedder(&self) -> &P {
        &self.embedder
    }

    /// Borrow the ranking weights (used by `recall`).
    pub fn weights(&self) -> Weights {
        self.weights
    }

    /// Store a new memory: heuristic-enrich, embed the content, then write.
    pub async fn remember(&self, input: RememberInput) -> rb_types::Result<MemoryId> {
        let mut note = MemoryNote::new(
            self.namespace.clone(),
            input.content,
            input.memory_type,
            input.importance,
        );
        // Heuristic enrichment (no LLM in P1).
        note.summary = default_summary(&note.content);
        note.keywords = if input.keywords.is_empty() {
            derive_keywords(&note.content)
        } else {
            input.keywords
        };
        note.tags = input.tags;
        note.related_files = input.related_files;
        if let Some(ctx) = input.context {
            note.context = ctx;
        }
        note.embedding_model = self.embedder.model_id().to_string();

        // Embed the content (single text in, single vector out).
        let mut embeddings = self.embedder.embed(&[note.content.clone()]).await?;
        let embedding = embeddings.pop();

        let id = note.id.clone();
        self.backend.write(note, embedding).await?;
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::test_support::MockBackend;
    use rb_embed::DeterministicProvider;
    use rb_types::{MemoryType, Namespace};

    fn engine() -> MemoryEngine<MockBackend, DeterministicProvider> {
        MemoryEngine::new(
            MockBackend::default(),
            DeterministicProvider::new(16),
            Namespace::Project("rb".into()),
        )
    }

    fn input(content: &str, importance: u8) -> RememberInput {
        RememberInput {
            content: content.to_string(),
            context: None,
            memory_type: MemoryType::Insight,
            importance,
            keywords: Vec::new(),
            tags: Vec::new(),
            related_files: Vec::new(),
        }
    }

    #[tokio::test]
    async fn remember_stores_note_and_embedding() {
        let eng = engine();
        let id = eng
            .remember(input("single writer over sqlite wal", 7))
            .await
            .unwrap();
        // exactly one note written, with an embedding of provider dim.
        assert_eq!(eng.backend().count(), 1);
        let emb = eng.backend().embedding_of(&id).unwrap();
        assert_eq!(emb.len(), 16);
    }

    #[tokio::test]
    async fn remember_applies_heuristic_summary_and_keywords() {
        let eng = engine();
        let content = "concurrent readers never block the single dedicated writer thread";
        let id = eng.remember(input(content, 6)).await.unwrap();
        let note = eng.backend().note_of(&id).unwrap();
        // summary defaults to the (trimmed) content since it's < 150 chars.
        assert_eq!(note.summary, content);
        // keywords derived (empty input) — non-empty, lowercased, capped at 5.
        assert!(!note.keywords.is_empty());
        assert!(note.keywords.len() <= 5);
        assert!(note.keywords.iter().all(|k| k == &k.to_lowercase()));
    }

    #[tokio::test]
    async fn remember_preserves_explicit_keywords_and_namespace() {
        let eng = engine();
        let mut inp = input("body text here", 5);
        inp.keywords = vec!["explicit".to_string()];
        inp.tags = vec!["t1".to_string()];
        inp.context = Some("ctx".to_string());
        let id = eng.remember(inp).await.unwrap();
        let note = eng.backend().note_of(&id).unwrap();
        assert_eq!(note.keywords, vec!["explicit".to_string()]);
        assert_eq!(note.tags, vec!["t1".to_string()]);
        assert_eq!(note.context, "ctx");
        // engine enforces its own namespace.
        assert_eq!(note.namespace, Namespace::Project("rb".into()));
    }

    #[tokio::test]
    async fn remember_sets_embedding_model_from_provider() {
        let eng = engine();
        let id = eng.remember(input("model id check", 5)).await.unwrap();
        let note = eng.backend().note_of(&id).unwrap();
        assert_eq!(note.embedding_model, eng.embedder().model_id());
    }

    #[tokio::test]
    async fn remember_is_deterministic_same_content_same_embedding() {
        let eng = engine();
        let id1 = eng.remember(input("identical content", 5)).await.unwrap();
        let id2 = eng.remember(input("identical content", 5)).await.unwrap();
        assert_ne!(id1, id2); // distinct notes
        assert_eq!(
            eng.backend().embedding_of(&id1),
            eng.backend().embedding_of(&id2)
        ); // deterministic provider => same vector
    }
}
