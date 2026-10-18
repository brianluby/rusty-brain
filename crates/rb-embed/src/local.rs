#![cfg(feature = "local")]
//! Local ONNX embedding provider backed by `fastembed`.
//!
//! Compiled only under the `local` cargo feature so the default build closure
//! never links fastembed/ort/onnxruntime. The default model is
//! `all-MiniLM-L6-v2` (384-dim). Model files are downloaded at runtime on
//! first use into fastembed's cache directory, at a pinned Hugging Face
//! revision with SHA-256-checked files; there is no network access in unit
//! tests (the real-embedding tests are `#[ignore]`).

use crate::provider::{EmbedKind, EmbeddingProvider};
use async_trait::async_trait;
use fastembed::{
    InitOptionsUserDefined, Pooling, TextEmbedding, TokenizerFiles, UserDefinedEmbeddingModel,
};
use hf_hub::api::sync::ApiBuilder;
use hf_hub::{Repo, RepoType};
use rb_types::Error;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
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

/// One file of a pinned model revision and its SHA-256 (lowercase hex).
#[derive(Debug, Clone, Copy)]
pub struct PinnedFile {
    pub name: &'static str,
    pub sha256: &'static str,
}

/// An exact Hugging Face revision of a local model. Loading fetches these
/// files at this revision only and fails closed if any hash differs, so an
/// upstream push to `main` cannot silently change embeddings (Vikunja #100).
#[derive(Debug, Clone)]
pub struct PinnedModel {
    pub repo: &'static str,
    pub revision: &'static str,
    pub onnx: PinnedFile,
    pub tokenizer: PinnedFile,
    pub config: PinnedFile,
    pub special_tokens_map: PinnedFile,
    pub tokenizer_config: PinnedFile,
    pub pooling: Pooling,
    /// Truncation used when the caller does not request one.
    pub default_max_tokens: usize,
}

/// `all-MiniLM-L6-v2` as fastembed served it until 2026-09-27, when upstream
/// commit `944c7f6` lowered `model_max_length` from 512 to 256 without
/// changing the weights. Every committed baseline, the real-vector replay
/// fixture and the September external bench were produced with this revision
/// at 512 tokens, so pinning it keeps the documented default.
pub const ALL_MINILM_L6_V2: PinnedModel = PinnedModel {
    repo: "Qdrant/all-MiniLM-L6-v2-onnx",
    revision: "5f1b8cd78bc4fb444dd171e59b18f3a3af89a079",
    onnx: PinnedFile {
        name: "model.onnx",
        sha256: "bbd7b466f6d58e646fdc2bd5fd67b2f5e93c0b687011bd4548c420f7bd46f0c5",
    },
    tokenizer: PinnedFile {
        name: "tokenizer.json",
        sha256: "da0e79933b9ed51798a3ae27893d3c5fa4a201126cef75586296df9b4d2c62a0",
    },
    config: PinnedFile {
        name: "config.json",
        sha256: "1b4d8e2a3988377ed8b519a31d8d31025a25f1c5f8606998e8014111438efcd7",
    },
    special_tokens_map: PinnedFile {
        name: "special_tokens_map.json",
        sha256: "5d5b662e421ea9fac075174bb0688ee0d9431699900b90662acd44b2a350503a",
    },
    tokenizer_config: PinnedFile {
        name: "tokenizer_config.json",
        sha256: "bd2e06a5b20fd1b13ca988bedc8763d332d242381b4fbc98f8fead4524158f79",
    },
    pooling: Pooling::Mean,
    default_max_tokens: 512,
};

/// Map a model name to its pinned revision, failing closed on unknown names
/// rather than guessing a dimension.
pub fn pinned_model(name: &str) -> rb_types::Result<&'static PinnedModel> {
    match resolve_model_name(name) {
        "all-MiniLM-L6-v2" => Ok(&ALL_MINILM_L6_V2),
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

fn verify_sha256(file: &PinnedFile, bytes: &[u8]) -> rb_types::Result<()> {
    let actual = format!("{:x}", Sha256::digest(bytes));
    if actual == file.sha256 {
        Ok(())
    } else {
        Err(Error::Embedding(format!(
            "pinned model file {} has SHA-256 {actual}, expected {}; refusing to load",
            file.name, file.sha256
        )))
    }
}

/// The tokenizer's own cap. fastembed truncates at
/// `min(requested, model_max_length)`, so this bounds every request.
fn model_max_length(tokenizer_config: &[u8]) -> rb_types::Result<usize> {
    let config: serde_json::Value = serde_json::from_slice(tokenizer_config)
        .map_err(|e| Error::Embedding(format!("invalid tokenizer_config.json: {e}")))?;
    config["model_max_length"]
        .as_f64()
        .filter(|n| *n >= 1.0)
        .map(|n| n as usize)
        .ok_or_else(|| {
            Error::Embedding("tokenizer_config.json has no usable model_max_length".to_string())
        })
}

/// Resolve the truncation to load with. A request above the tokenizer's cap is
/// an error: fastembed would silently clamp it, which is how a 512 request
/// came to mean 256 (Vikunja #100).
fn resolve_max_tokens(
    requested: Option<usize>,
    default: usize,
    cap: usize,
) -> rb_types::Result<usize> {
    let n = requested.unwrap_or(default);
    if n == 0 || n > cap {
        return Err(Error::Embedding(format!(
            "requested {n} embedding tokens but the pinned tokenizer accepts 1..={cap}"
        )));
    }
    Ok(n)
}

/// Fetch and hash-check every pinned file, using the same cache and endpoint
/// resolution as fastembed's own download path. Cached files are reused, so
/// this is offline after the first load.
fn fetch_pinned(pin: &PinnedModel) -> rb_types::Result<PinnedBytes> {
    let cache_dir = std::env::var("HF_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(fastembed::get_cache_dir()));
    let endpoint =
        std::env::var("HF_ENDPOINT").unwrap_or_else(|_| "https://huggingface.co".to_string());
    let api = ApiBuilder::new()
        .with_cache_dir(cache_dir)
        .with_endpoint(endpoint)
        .with_progress(false)
        .build()
        .map_err(|e| Error::Embedding(format!("failed to set up model download: {e}")))?;
    let repo = api.repo(Repo::with_revision(
        pin.repo.to_string(),
        RepoType::Model,
        pin.revision.to_string(),
    ));
    let read = |file: &PinnedFile| -> rb_types::Result<Vec<u8>> {
        let path = repo.get(file.name).map_err(|e| {
            Error::Embedding(format!(
                "failed to fetch {}@{} {}: {e}",
                pin.repo, pin.revision, file.name
            ))
        })?;
        let bytes = std::fs::read(&path)
            .map_err(|e| Error::Embedding(format!("failed to read {}: {e}", path.display())))?;
        verify_sha256(file, &bytes)?;
        Ok(bytes)
    };
    Ok(PinnedBytes {
        onnx: read(&pin.onnx)?,
        tokenizer: TokenizerFiles {
            tokenizer_file: read(&pin.tokenizer)?,
            config_file: read(&pin.config)?,
            special_tokens_map_file: read(&pin.special_tokens_map)?,
            tokenizer_config_file: read(&pin.tokenizer_config)?,
        },
    })
}

struct PinnedBytes {
    onnx: Vec<u8>,
    tokenizer: TokenizerFiles,
}

/// Truncation actually applied by a loaded model, read back from its tokenizer.
fn effective_max_tokens(model: &TextEmbedding) -> rb_types::Result<usize> {
    model
        .tokenizer
        .get_truncation()
        .map(|t| t.max_length)
        .ok_or_else(|| Error::Embedding("local tokenizer has no truncation set".to_string()))
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
    pin: Option<&'static PinnedModel>,
    max_tokens: Option<usize>,
}

impl LocalProvider {
    /// Load `model_name` (empty selects the default) at its pinned revision,
    /// downloading weights at runtime on first use. Maps any fetch, hash or
    /// fastembed init failure to `Error::Embedding`.
    pub fn load(model_name: &str) -> rb_types::Result<Self> {
        Self::load_with_max_tokens(model_name, None)
    }

    /// Like [`LocalProvider::load`], but truncating inputs at `max_tokens`
    /// instead of the pin's default (512). Benchmarks use 256 to match
    /// sentence-transformers' `max_seq_length` for all-MiniLM-L6-v2. A value
    /// above the tokenizer's cap is an error, never a silent clamp.
    pub fn load_with_max_tokens(
        model_name: &str,
        max_tokens: Option<usize>,
    ) -> rb_types::Result<Self> {
        let canonical = resolve_model_name(model_name).to_string();
        let pin = pinned_model(&canonical)?;
        let dim = dim_for_model(&canonical)?;
        let files = fetch_pinned(pin)?;
        let cap = model_max_length(&files.tokenizer.tokenizer_config_file)?;
        let requested = resolve_max_tokens(max_tokens, pin.default_max_tokens, cap)?;
        let user_model = UserDefinedEmbeddingModel::new(files.onnx, files.tokenizer)
            .with_pooling(pin.pooling.clone());
        let model = TextEmbedding::try_new_from_user_defined(
            user_model,
            InitOptionsUserDefined::new().with_max_length(requested),
        )
        .map_err(|e| Error::Embedding(format!("failed to load local model: {e}")))?;
        let effective = effective_max_tokens(&model)?;
        if effective != requested {
            return Err(Error::Embedding(format!(
                "local tokenizer truncates at {effective} tokens, not the requested {requested}"
            )));
        }
        Ok(Self {
            model: Some(Arc::new(Mutex::new(model))),
            model_id: canonical,
            dim,
            pin: Some(pin),
            max_tokens: Some(effective),
        })
    }

    /// Test/diagnostic constructor that records metadata WITHOUT loading any
    /// weights. `embed` on such a provider fails closed with `Error::Embedding`.
    pub fn without_model(model_id: &str, dim: usize) -> Self {
        Self {
            model: None,
            model_id: model_id.to_string(),
            dim,
            pin: None,
            max_tokens: None,
        }
    }

    /// The pinned revision the loaded files came from (`None` without a model).
    pub fn pin(&self) -> Option<&'static PinnedModel> {
        self.pin
    }

    /// Effective input truncation in tokens, read back from the loaded
    /// tokenizer rather than assumed (`None` without a model).
    pub fn max_tokens(&self) -> Option<usize> {
        self.max_tokens
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

    /// The kind is IGNORED by design (W1.4): all-MiniLM-L6-v2 is a symmetric
    /// model with no query/document conditioning (no instruction prefixes), so
    /// query vectors must stay in the same space as the persisted document
    /// vectors — differentiating here would silently split the vector space.
    async fn embed(&self, texts: &[String], _kind: EmbedKind) -> rb_types::Result<Vec<Vec<f32>>> {
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
        let err = p
            .embed(&["hello".to_string()], EmbedKind::Document)
            .await
            .unwrap_err();
        assert!(
            matches!(err, rb_types::Error::Embedding(_)),
            "expected Error::Embedding when no model is loaded, got {err:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn empty_input_yields_empty_output_without_loading_a_model() {
        // Empty input short-circuits: no model access, no error.
        let p = LocalProvider::without_model("all-MiniLM-L6-v2", 384);
        let out = p.embed(&[], EmbedKind::Document).await.unwrap();
        assert!(out.is_empty());
    }

    fn is_lower_hex(s: &str, len: usize) -> bool {
        s.len() == len && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    }

    #[test]
    fn default_model_is_pinned_to_an_exact_revision_with_file_hashes() {
        let pin = pinned_model("").unwrap();
        assert_eq!(pin.repo, "Qdrant/all-MiniLM-L6-v2-onnx");
        assert!(
            is_lower_hex(pin.revision, 40),
            "revision is a full commit id"
        );
        for file in [
            pin.onnx,
            pin.tokenizer,
            pin.config,
            pin.special_tokens_map,
            pin.tokenizer_config,
        ] {
            assert!(is_lower_hex(file.sha256, 64), "{} hash", file.name);
        }
        assert_eq!(pin.default_max_tokens, 512);
        assert!(pinned_model("not-a-real-model").is_err());
    }

    #[test]
    fn file_hash_mismatch_fails_closed() {
        let file = PinnedFile {
            name: "tokenizer_config.json",
            // SHA-256 of the empty string.
            sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        };
        verify_sha256(&file, b"").unwrap();
        let err = verify_sha256(&file, b"{}").unwrap_err();
        assert!(matches!(err, rb_types::Error::Embedding(_)), "{err:?}");
    }

    #[test]
    fn model_max_length_reads_the_tokenizer_cap() {
        assert_eq!(
            model_max_length(br#"{"model_max_length": 512}"#).unwrap(),
            512
        );
        assert_eq!(
            model_max_length(br#"{"model_max_length": 256.0}"#).unwrap(),
            256
        );
        assert!(model_max_length(br#"{"max_length": 128}"#).is_err());
        assert!(model_max_length(b"not json").is_err());
    }

    #[test]
    fn requests_above_the_tokenizer_cap_are_errors_not_silent_clamps() {
        assert_eq!(resolve_max_tokens(None, 512, 512).unwrap(), 512);
        assert_eq!(resolve_max_tokens(Some(256), 512, 512).unwrap(), 256);
        // The #100 failure: 512 requested against a 256-token tokenizer.
        assert!(resolve_max_tokens(None, 512, 256).is_err());
        assert!(resolve_max_tokens(Some(512), 512, 256).is_err());
        assert!(resolve_max_tokens(Some(0), 512, 512).is_err());
    }

    #[test]
    fn metadata_only_provider_has_no_pin_or_truncation() {
        let p = LocalProvider::without_model("all-MiniLM-L6-v2", 384);
        assert!(p.pin().is_none());
        assert_eq!(p.max_tokens(), None);
    }

    // Real-model pin test. Ignored by default; downloads the pinned files on
    // first run. Run with:
    //   cargo test -p rb-embed --features local -- --ignored local_real_model
    #[test]
    #[ignore = "downloads the pinned all-MiniLM-L6-v2 files and loads the ONNX model"]
    fn local_real_model_loads_the_pin_and_reports_effective_truncation() {
        let default = LocalProvider::load("").unwrap();
        assert_eq!(default.pin().unwrap().revision, ALL_MINILM_L6_V2.revision);
        assert_eq!(default.max_tokens(), Some(512));
        let short = LocalProvider::load_with_max_tokens("", Some(256)).unwrap();
        assert_eq!(short.max_tokens(), Some(256));
        assert!(LocalProvider::load_with_max_tokens("", Some(513)).is_err());
    }

    // Confirms the #100 mechanism in fastembed itself: with a tokenizer config
    // declaring `model_max_length: 256` (upstream since 2026-09-27), a 512
    // request is clamped to 256 without any error.
    #[test]
    #[ignore = "downloads the pinned all-MiniLM-L6-v2 files and loads the ONNX model"]
    fn local_real_model_fastembed_clamps_to_tokenizer_model_max_length() {
        let mut files = fetch_pinned(&ALL_MINILM_L6_V2).unwrap();
        let mut config: serde_json::Value =
            serde_json::from_slice(&files.tokenizer.tokenizer_config_file).unwrap();
        assert_eq!(config["model_max_length"], 512);
        config["model_max_length"] = 256.into();
        files.tokenizer.tokenizer_config_file = serde_json::to_vec(&config).unwrap();
        let model = TextEmbedding::try_new_from_user_defined(
            UserDefinedEmbeddingModel::new(files.onnx, files.tokenizer).with_pooling(Pooling::Mean),
            InitOptionsUserDefined::new().with_max_length(512),
        )
        .unwrap();
        assert_eq!(effective_max_tokens(&model).unwrap(), 256);
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
            .embed(
                &["hello world".to_string(), "second".to_string()],
                EmbedKind::Document,
            )
            .await
            .unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].len(), 384);
        assert_eq!(out[1].len(), 384);
    }
}
