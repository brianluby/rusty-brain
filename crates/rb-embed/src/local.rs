#![cfg(feature = "local")]
//! Local ONNX embedding provider backed by `fastembed`.
//!
//! Compiled only under the `local` cargo feature so the default build closure
//! never links fastembed/ort/onnxruntime. The default model is
//! `all-MiniLM-L6-v2` (384-dim). Model weights are downloaded at runtime on
//! first use into fastembed's cache directory; there is no network access in
//! unit tests (the real-embedding test is `#[ignore]`).

use crate::provider::EmbeddingProvider;
use async_trait::async_trait;
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use rb_types::Error;
use std::sync::{Arc, Mutex};

/// Default model when none is specified. 384-dimensional.
pub const DEFAULT_MODEL: &str = "all-MiniLM-L6-v2";
/// Embedding dimension of `all-MiniLM-L6-v2`.
pub const DEFAULT_DIM: usize = 384;

/// Map an (optionally empty) model name to the canonical name we support.
/// An empty string selects the default model.
pub fn resolve_model_name(name: &str) -> &str {
    if name.trim().is_empty() {
        DEFAULT_MODEL
    } else {
        name
    }
}

/// Map a model name to its fastembed enum variant, failing closed on unknown
/// names rather than guessing a dimension.
fn model_for_name(name: &str) -> rb_types::Result<EmbeddingModel> {
    match resolve_model_name(name) {
        "all-MiniLM-L6-v2" => Ok(EmbeddingModel::AllMiniLML6V2),
        other => Err(Error::Embedding(format!(
            "unsupported local embedding model: {other}"
        ))),
    }
}

/// Map a model name to its fixed embedding dimension. Unknown names are an
/// `Error::Embedding` (the dim contract is sacred; never guess).
pub fn dim_for_model(name: &str) -> rb_types::Result<usize> {
    match resolve_model_name(name) {
        "all-MiniLM-L6-v2" => Ok(DEFAULT_DIM),
        other => Err(Error::Embedding(format!(
            "unsupported local embedding model: {other}"
        ))),
    }
}

/// Offline-capable local embedding provider. Holds a loaded fastembed
/// `TextEmbedding` behind a `Mutex` (its `embed` takes `&mut self`) and runs
/// inference on a blocking thread pool. `model` is `None` only in tests built
/// via [`LocalProvider::without_model`], which never load weights so metadata
/// can be asserted offline.
pub struct LocalProvider {
    model: Option<Arc<Mutex<TextEmbedding>>>,
    model_id: String,
    dim: usize,
}

impl LocalProvider {
    /// Load `model_name` (empty selects the default), downloading weights at
    /// runtime on first use. Maps any fastembed init failure to
    /// `Error::Embedding`.
    pub fn load(model_name: &str) -> rb_types::Result<Self> {
        let canonical = resolve_model_name(model_name).to_string();
        let model_enum = model_for_name(&canonical)?;
        let dim = dim_for_model(&canonical)?;
        let options = InitOptions::new(model_enum).with_show_download_progress(false);
        let model = TextEmbedding::try_new(options)
            .map_err(|e| Error::Embedding(format!("failed to load local model: {e}")))?;
        Ok(Self {
            model: Some(Arc::new(Mutex::new(model))),
            model_id: canonical,
            dim,
        })
    }

    /// Test/diagnostic constructor that records metadata WITHOUT loading any
    /// weights. `embed` on such a provider fails closed with `Error::Embedding`.
    pub fn without_model(model_id: &str, dim: usize) -> Self {
        Self {
            model: None,
            model_id: model_id.to_string(),
            dim,
        }
    }
}

#[async_trait]
impl EmbeddingProvider for LocalProvider {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn dim(&self) -> usize {
        self.dim
    }

    async fn embed(&self, texts: &[String]) -> rb_types::Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let model = self
            .model
            .as_ref()
            .ok_or_else(|| Error::Embedding("local model is not loaded".to_string()))?;
        let model = Arc::clone(model);
        let owned: Vec<String> = texts.to_vec();
        let expected_dim = self.dim;

        // ONNX inference is CPU-bound and synchronous; run it off the async
        // runtime. SharedEmbedder's semaphore bounds how many of these run.
        let vectors = tokio::task::spawn_blocking(move || -> rb_types::Result<Vec<Vec<f32>>> {
            let mut guard = model
                .lock()
                .map_err(|_| Error::Embedding("local model mutex poisoned".to_string()))?;
            let out = guard
                .embed(&owned, None)
                .map_err(|e| Error::Embedding(format!("local embedding failed: {e}")))?;
            if out.len() != owned.len() {
                return Err(Error::Embedding(format!(
                    "local model returned {} embeddings for {} inputs",
                    out.len(),
                    owned.len()
                )));
            }
            for v in &out {
                if v.len() != expected_dim {
                    return Err(Error::Embedding(format!(
                        "local embedding dimension mismatch: expected {}, got {}",
                        expected_dim,
                        v.len()
                    )));
                }
            }
            Ok(out)
        })
        .await
        .map_err(|e| Error::Embedding(format!("local embedding task failed to join: {e}")))??;

        Ok(vectors)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::provider::EmbeddingProvider;

    #[test]
    fn default_model_name_resolves_to_all_minilm() {
        assert_eq!(resolve_model_name(""), "all-MiniLM-L6-v2");
        assert_eq!(resolve_model_name("all-MiniLM-L6-v2"), "all-MiniLM-L6-v2");
    }

    #[test]
    fn known_model_reports_384_dim() {
        assert_eq!(dim_for_model("all-MiniLM-L6-v2").unwrap(), 384);
    }

    #[test]
    fn unknown_model_is_an_embedding_error() {
        let err = dim_for_model("not-a-real-model").unwrap_err();
        assert!(
            matches!(err, rb_types::Error::Embedding(_)),
            "expected Error::Embedding for unknown model, got {err:?}"
        );
    }

    #[test]
    fn fixture_provider_reports_model_id_and_dim_without_loading_a_model() {
        // Build a provider WITHOUT a loaded model so the test is fully offline:
        // metadata must be available before (or without ever) downloading weights.
        let p = LocalProvider::without_model("all-MiniLM-L6-v2", 384);
        assert_eq!(p.model_id(), "all-MiniLM-L6-v2");
        assert_eq!(p.dim(), 384);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn embed_without_loaded_model_is_an_embedding_error() {
        // The fixture provider has no model; calling embed must fail closed
        // with Error::Embedding rather than panicking.
        let p = LocalProvider::without_model("all-MiniLM-L6-v2", 384);
        let err = p.embed(&["hello".to_string()]).await.unwrap_err();
        assert!(
            matches!(err, rb_types::Error::Embedding(_)),
            "expected Error::Embedding when no model is loaded, got {err:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn empty_input_yields_empty_output_without_loading_a_model() {
        // Empty input short-circuits: no model access, no error.
        let p = LocalProvider::without_model("all-MiniLM-L6-v2", 384);
        let out = p.embed(&[]).await.unwrap();
        assert!(out.is_empty());
    }

    // Real-model smoke test. Ignored by default; downloads ~90MB of weights on
    // first run. Run with:
    //   cargo test -p rb-embed --features local -- --ignored local_real_model
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "downloads the all-MiniLM-L6-v2 model and runs ONNX inference"]
    async fn local_real_model_smoke() {
        let p = LocalProvider::load("all-MiniLM-L6-v2").unwrap();
        assert_eq!(p.dim(), 384);
        let out = p
            .embed(&["hello world".to_string(), "second".to_string()])
            .await
            .unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].len(), 384);
        assert_eq!(out[1].len(), 384);
    }
}
