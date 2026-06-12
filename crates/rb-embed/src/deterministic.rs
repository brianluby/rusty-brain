use crate::provider::{EmbedKind, EmbeddingProvider};
use async_trait::async_trait;
use sha2::{Digest, Sha256};

/// Offline, deterministic embedding provider. Hashes each input text into a
/// reproducible unit-length vector of length `dim`. Same text always yields the
/// same vector; different texts yield different vectors. Never performs IO and
/// never errors. Public so it can be used as a no-API-key fallback and in tests.
///
/// Determinism relies on SHA-256 so persisted vectors do not depend on standard
/// library hash implementation details.
pub struct DeterministicProvider {
    dim: usize,
}

impl DeterministicProvider {
    /// Create a provider that emits `dim`-length vectors.
    pub fn new(dim: usize) -> Self {
        Self { dim }
    }

    /// Produce one reproducible unit-length vector for `text`.
    fn embed_one(&self, text: &str) -> Vec<f32> {
        let mut raw = Vec::with_capacity(self.dim);
        for i in 0..self.dim {
            let mut hasher = Sha256::new();
            hasher.update(text.as_bytes());
            hasher.update([0]);
            hasher.update((i as u64).to_le_bytes());
            let digest = hasher.finalize();
            let mut bytes = [0_u8; 8];
            bytes.copy_from_slice(&digest[..8]);
            let h = u64::from_le_bytes(bytes);
            let unit = (h as f64) / (u64::MAX as f64);
            raw.push((unit * 2.0 - 1.0) as f32);
        }

        let norm: f32 = raw.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for x in &mut raw {
                *x /= norm;
            }
        } else if !raw.is_empty() {
            raw[0] = 1.0;
        }
        raw
    }
}

#[async_trait]
impl EmbeddingProvider for DeterministicProvider {
    fn model_id(&self) -> &str {
        "deterministic"
    }

    fn dim(&self) -> usize {
        self.dim
    }

    /// The kind is IGNORED by design (W1.4): deterministic vectors are a pure
    /// function of the text, so persisted corpora and the committed eval
    /// baselines stay valid — `embed(t, Query) == embed(t, Document)` always.
    async fn embed(&self, texts: &[String], _kind: EmbedKind) -> rb_types::Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|t| self.embed_one(t)).collect())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::provider::EmbeddingProvider;

    #[test]
    fn model_id_and_dim_are_reported() {
        let p = DeterministicProvider::new(512);
        assert_eq!(p.dim(), 512);
        assert_eq!(p.model_id(), "deterministic");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn embed_returns_one_vector_per_input_of_correct_length() {
        let p = DeterministicProvider::new(8);
        let inputs = vec!["alpha".to_string(), "beta".to_string(), "gamma".to_string()];
        let out = p.embed(&inputs, EmbedKind::Document).await.unwrap();
        assert_eq!(out.len(), 3);
        for v in &out {
            assert_eq!(v.len(), 8);
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn same_text_yields_same_vector() {
        let p = DeterministicProvider::new(16);
        let a = p
            .embed(&["repeatable".to_string()], EmbedKind::Document)
            .await
            .unwrap();
        let b = p
            .embed(&["repeatable".to_string()], EmbedKind::Document)
            .await
            .unwrap();
        assert_eq!(a, b);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn kind_blindness_query_equals_document_for_identical_text() {
        // W1.4 invariant: DeterministicProvider MUST ignore the EmbedKind so
        // persisted corpora and the committed eval baselines stay valid.
        let p = DeterministicProvider::new(16);
        let texts = vec!["identical text".to_string(), "another one".to_string()];
        let as_query = p.embed(&texts, EmbedKind::Query).await.unwrap();
        let as_document = p.embed(&texts, EmbedKind::Document).await.unwrap();
        assert_eq!(
            as_query, as_document,
            "deterministic embed must be kind-blind: embed_query == embed_document"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn vector_values_are_stable_for_persisted_embeddings() {
        let p = DeterministicProvider::new(4);
        let out = p
            .embed(
                &["stable persisted vector".to_string()],
                EmbedKind::Document,
            )
            .await
            .unwrap();
        let expected = [
            -0.19579384_f32,
            -0.48179987_f32,
            -0.622556_f32,
            -0.58477145_f32,
        ];

        for (actual, expected) in out[0].iter().zip(expected) {
            assert!(
                (actual - expected).abs() < 1e-6,
                "expected {expected}, got {actual}"
            );
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn different_text_yields_different_vector() {
        let p = DeterministicProvider::new(16);
        let out = p
            .embed(&["one".to_string(), "two".to_string()], EmbedKind::Document)
            .await
            .unwrap();
        assert_ne!(out[0], out[1]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn embed_preserves_input_order() {
        let p = DeterministicProvider::new(16);
        let inputs = vec!["x".to_string(), "y".to_string(), "z".to_string()];
        // Embedding the list at once must equal embedding each item alone, in order.
        let batched = p.embed(&inputs, EmbedKind::Document).await.unwrap();
        let mut individually = Vec::new();
        for t in &inputs {
            individually.push(
                p.embed(std::slice::from_ref(t), EmbedKind::Document)
                    .await
                    .unwrap()[0]
                    .clone(),
            );
        }
        assert_eq!(batched, individually);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn empty_input_yields_empty_output() {
        let p = DeterministicProvider::new(4);
        let out = p.embed(&[], EmbedKind::Document).await.unwrap();
        assert!(out.is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn vectors_are_unit_length() {
        let p = DeterministicProvider::new(32);
        let out = p
            .embed(&["normalize me".to_string()], EmbedKind::Document)
            .await
            .unwrap();
        let norm: f32 = out[0].iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(
            (norm - 1.0).abs() < 1e-5,
            "expected unit vector, got norm {norm}"
        );
    }
}
