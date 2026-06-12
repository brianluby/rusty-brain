//! Record/replay of REAL embedding vectors as committed fixtures (W1.0).
//!
//! The deterministic provider guards regressions but cannot show semantic
//! quality; live API calls cannot run in CI. This module bridges the two:
//! a **recording** pass (run manually, with credentials/network) captures every
//! vector a real model produces for the committed corpus, and a **replay**
//! provider serves those committed vectors with zero network and zero keys —
//! the mode CI and later workstreams use for semantic measurement.
//!
//! ## Key shape (W1.4-proof)
//!
//! Each recorded vector is keyed on `(model_id, input_kind, sha256(text))`.
//! `input_kind` is `"document"` or `"query"`; every value is `"document"`
//! today because the `EmbeddingProvider` trait has no kind parameter yet —
//! when W1.4 lands query-kind embeddings, query vectors are recorded under
//! `"query"` without invalidating the document fixtures.
//!
//! ## Commands
//!
//! Record with the local ONNX model (downloads weights on first use):
//!
//! ```text
//! cargo test -p rb-eval --features record-local --test record_embeddings \
//!   -- --ignored record_local_model_fixture --nocapture
//! ```
//!
//! Record with Voyage (preferred when a key is available):
//!
//! ```text
//! VOYAGE_API_KEY=... cargo test -p rb-eval --test record_embeddings \
//!   -- --ignored record_voyage_fixture --nocapture
//! ```
//!
//! Replay (offline, what CI runs): see `tests/replay_model.rs`.

use rb_embed::EmbeddingProvider;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

/// The only input kind recorded today (see module docs).
pub const INPUT_KIND_DOCUMENT: &str = "document";

/// Crate-relative path of the committed default replay fixture. Re-point this
/// (and the `include_str!` in [`ReplayProvider::committed`]) if a Voyage
/// fixture supersedes the local-model one.
pub const COMMITTED_FIXTURE_PATH: &str = "fixtures/embeddings/all-MiniLM-L6-v2.json";

/// Hex of the SHA-256 digest of `text` — the text component of the fixture key.
pub fn text_sha256(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    encode_hex(&digest)
}

fn encode_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// Encode an f32 vector as lowercase hex of its little-endian bytes: exact
/// round-trip (bit-for-bit), no external base64/float-text dependency.
fn encode_vector(v: &[f32]) -> String {
    let mut bytes = Vec::with_capacity(v.len() * 4);
    for f in v {
        bytes.extend_from_slice(&f.to_le_bytes());
    }
    encode_hex(&bytes)
}

/// Decode [`encode_vector`] output. Fails closed on odd length, non-hex
/// characters, or a byte count that is not a multiple of 4.
fn decode_vector(s: &str) -> Result<Vec<f32>, String> {
    if !s.len().is_multiple_of(8) {
        return Err(format!(
            "embedding hex length {} is not a multiple of 8 (4 LE bytes per f32)",
            s.len()
        ));
    }
    let mut bytes = Vec::with_capacity(s.len() / 2);
    let chars = s.as_bytes();
    for pair in chars.chunks_exact(2) {
        let hi = hex_digit(pair[0])?;
        let lo = hex_digit(pair[1])?;
        bytes.push((hi << 4) | lo);
    }
    Ok(bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect())
}

fn hex_digit(c: u8) -> Result<u8, String> {
    match c {
        b'0'..=b'9' => Ok(c - b'0'),
        b'a'..=b'f' => Ok(c - b'a' + 10),
        b'A'..=b'F' => Ok(c - b'A' + 10),
        other => Err(format!("invalid hex character {:?}", other as char)),
    }
}

/// One recorded `(input_kind, text) -> vector` entry. `text_preview` is for
/// human debugging only; the lookup key is `(input_kind, text_sha256)`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordedVector {
    pub input_kind: String,
    pub text_sha256: String,
    pub text_preview: String,
    /// Lowercase hex of the vector's little-endian f32 bytes (exact).
    pub embedding_hex: String,
}

/// A committed embedding fixture: every vector one real model produced for the
/// corpus, keyed per the module docs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingFixture {
    /// Provenance/how-to-regenerate note for human readers.
    #[serde(default, rename = "_comment")]
    pub comment: String,
    pub model_id: String,
    pub dim: usize,
    pub vectors: Vec<RecordedVector>,
}

/// Fixture key: `(input_kind, text_sha256)`. The third key component,
/// `model_id`, scopes the whole fixture file.
type FixtureKey = (String, String);
/// Recorded payload: `(text_preview, vector)`.
type RecordedEntry = (String, Vec<f32>);

/// Wraps a real provider and records every `(text, vector)` pair it serves.
/// Used only by the manual `#[ignore]` recording tests.
pub struct RecordingProvider<P> {
    inner: P,
    /// BTreeMap so the emitted fixture file is deterministically ordered.
    recorded: Mutex<BTreeMap<FixtureKey, RecordedEntry>>,
}

impl<P: EmbeddingProvider> RecordingProvider<P> {
    pub fn new(inner: P) -> Self {
        Self {
            inner,
            recorded: Mutex::new(BTreeMap::new()),
        }
    }

    /// Snapshot everything recorded so far into a fixture.
    pub fn fixture(&self, comment: &str) -> rb_types::Result<EmbeddingFixture> {
        let recorded = self
            .recorded
            .lock()
            .map_err(|_| rb_types::Error::Embedding("recording mutex poisoned".into()))?;
        let vectors = recorded
            .iter()
            .map(
                |((input_kind, text_sha256), (preview, vector))| RecordedVector {
                    input_kind: input_kind.clone(),
                    text_sha256: text_sha256.clone(),
                    text_preview: preview.clone(),
                    embedding_hex: encode_vector(vector),
                },
            )
            .collect();
        Ok(EmbeddingFixture {
            comment: comment.to_string(),
            model_id: self.inner.model_id().to_string(),
            dim: self.inner.dim(),
            vectors,
        })
    }
}

#[async_trait::async_trait]
impl<P: EmbeddingProvider> EmbeddingProvider for RecordingProvider<P> {
    fn model_id(&self) -> &str {
        self.inner.model_id()
    }

    fn dim(&self) -> usize {
        self.inner.dim()
    }

    async fn embed(&self, texts: &[String]) -> rb_types::Result<Vec<Vec<f32>>> {
        let vectors = self.inner.embed(texts).await?;
        let mut recorded = self
            .recorded
            .lock()
            .map_err(|_| rb_types::Error::Embedding("recording mutex poisoned".into()))?;
        for (text, vector) in texts.iter().zip(vectors.iter()) {
            let preview: String = text.chars().take(80).collect();
            recorded.insert(
                (INPUT_KIND_DOCUMENT.to_string(), text_sha256(text)),
                (preview, vector.clone()),
            );
        }
        Ok(vectors)
    }
}

/// Serves committed real-model vectors with zero network and zero keys.
///
/// `embed` FAILS CLOSED on any text absent from the fixture: corpus drift must
/// force a re-recording, never silently degrade to wrong vectors.
pub struct ReplayProvider {
    model_id: String,
    dim: usize,
    vectors: HashMap<FixtureKey, Vec<f32>>,
}

impl ReplayProvider {
    /// Build from a parsed fixture, validating every vector against `dim`.
    pub fn from_fixture(fixture: &EmbeddingFixture) -> Result<Self, String> {
        if fixture.dim == 0 {
            return Err("fixture dim must be non-zero".to_string());
        }
        let mut vectors: HashMap<FixtureKey, Vec<f32>> = HashMap::new();
        for rv in &fixture.vectors {
            let v = decode_vector(&rv.embedding_hex)
                .map_err(|e| format!("vector for hash {}: {e}", rv.text_sha256))?;
            if v.len() != fixture.dim {
                return Err(format!(
                    "vector for hash {} has dim {}, fixture says {}",
                    rv.text_sha256,
                    v.len(),
                    fixture.dim
                ));
            }
            if vectors
                .insert((rv.input_kind.clone(), rv.text_sha256.clone()), v)
                .is_some()
            {
                return Err(format!(
                    "duplicate fixture key ({}, {})",
                    rv.input_kind, rv.text_sha256
                ));
            }
        }
        Ok(Self {
            model_id: fixture.model_id.clone(),
            dim: fixture.dim,
            vectors,
        })
    }

    /// Parse + validate a fixture from raw JSON.
    pub fn from_json(raw: &str) -> Result<Self, String> {
        let fixture: EmbeddingFixture =
            serde_json::from_str(raw).map_err(|e| format!("fixture parse error: {e}"))?;
        Self::from_fixture(&fixture)
    }

    /// Load the committed default fixture bundled at compile time (currently
    /// the local `all-MiniLM-L6-v2` recording; see [`COMMITTED_FIXTURE_PATH`]).
    pub fn committed() -> Result<Self, String> {
        const RAW: &str = include_str!("../fixtures/embeddings/all-MiniLM-L6-v2.json");
        Self::from_json(RAW)
    }

    /// Number of recorded vectors (observability for tests/reports).
    pub fn len(&self) -> usize {
        self.vectors.len()
    }

    pub fn is_empty(&self) -> bool {
        self.vectors.is_empty()
    }
}

#[async_trait::async_trait]
impl EmbeddingProvider for ReplayProvider {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn dim(&self) -> usize {
        self.dim
    }

    async fn embed(&self, texts: &[String]) -> rb_types::Result<Vec<Vec<f32>>> {
        texts
            .iter()
            .map(|text| {
                let key = (INPUT_KIND_DOCUMENT.to_string(), text_sha256(text));
                self.vectors.get(&key).cloned().ok_or_else(|| {
                    let preview: String = text.chars().take(80).collect();
                    rb_types::Error::Embedding(format!(
                        "replay miss: no recorded '{INPUT_KIND_DOCUMENT}' vector for sha256 {} \
                         (text preview: {preview:?}). The corpus drifted from the committed \
                         fixture — re-record it (see rb-eval/src/replay.rs module docs)",
                        key.1
                    ))
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rb_embed::DeterministicProvider;

    #[test]
    fn vector_hex_round_trips_exactly() {
        let v = vec![0.0f32, -1.5, 3.25e-7, f32::MAX, f32::MIN_POSITIVE];
        let encoded = encode_vector(&v);
        let back = decode_vector(&encoded).unwrap();
        assert_eq!(v.len(), back.len());
        for (a, b) in v.iter().zip(back.iter()) {
            assert_eq!(a.to_bits(), b.to_bits(), "bit-exact round trip");
        }
    }

    #[test]
    fn decode_rejects_bad_hex() {
        assert!(decode_vector("zz000000").is_err(), "non-hex chars rejected");
        assert!(decode_vector("0000").is_err(), "non-multiple-of-8 rejected");
    }

    #[test]
    fn text_hash_is_stable_and_text_sensitive() {
        let a = text_sha256("single writer");
        assert_eq!(a, text_sha256("single writer"));
        assert_ne!(a, text_sha256("single writer "));
        assert_eq!(a.len(), 64);
    }

    #[tokio::test]
    async fn record_then_replay_serves_identical_vectors() {
        let recorder = RecordingProvider::new(DeterministicProvider::new(8));
        let texts = vec!["alpha doc".to_string(), "beta doc".to_string()];
        let original = recorder.embed(&texts).await.unwrap();

        let fixture = recorder.fixture("test fixture").unwrap();
        assert_eq!(fixture.model_id, "deterministic");
        assert_eq!(fixture.dim, 8);
        assert_eq!(fixture.vectors.len(), 2);
        assert!(fixture
            .vectors
            .iter()
            .all(|v| v.input_kind == INPUT_KIND_DOCUMENT));

        let replay = ReplayProvider::from_fixture(&fixture).unwrap();
        let replayed = replay.embed(&texts).await.unwrap();
        assert_eq!(original, replayed, "replay must be bit-identical");
    }

    #[tokio::test]
    async fn replay_fails_closed_on_unrecorded_text() {
        let recorder = RecordingProvider::new(DeterministicProvider::new(8));
        recorder.embed(&["known".to_string()]).await.unwrap();
        let replay = ReplayProvider::from_fixture(&recorder.fixture("t").unwrap()).unwrap();

        let err = replay
            .embed(&["never recorded".to_string()])
            .await
            .unwrap_err();
        assert!(
            matches!(err, rb_types::Error::Embedding(_)),
            "replay miss must be Error::Embedding, got {err:?}"
        );
        assert!(
            err.to_string().contains("replay miss"),
            "error names the failure mode: {err}"
        );
    }

    #[tokio::test]
    async fn replay_key_is_kind_scoped_so_w14_query_vectors_can_coexist() {
        // The fixture key includes input_kind: a vector recorded as "document"
        // must NOT satisfy a "query" lookup. (All lookups are "document" until
        // W1.4; this pins the key shape it relies on.)
        let recorder = RecordingProvider::new(DeterministicProvider::new(8));
        recorder.embed(&["text".to_string()]).await.unwrap();
        let mut fixture = recorder.fixture("t").unwrap();
        // Re-tag the recorded vector as a future query-kind entry.
        fixture.vectors[0].input_kind = "query".to_string();
        let replay = ReplayProvider::from_fixture(&fixture).unwrap();
        let err = replay.embed(&["text".to_string()]).await.unwrap_err();
        assert!(
            matches!(err, rb_types::Error::Embedding(_)),
            "a query-kind recording must not serve a document-kind lookup"
        );
    }

    #[test]
    fn fixture_rejects_dim_mismatch_and_duplicates() {
        let mk = |hash: &str| RecordedVector {
            input_kind: INPUT_KIND_DOCUMENT.to_string(),
            text_sha256: hash.to_string(),
            text_preview: "p".to_string(),
            embedding_hex: encode_vector(&[1.0, 2.0]),
        };
        let wrong_dim = EmbeddingFixture {
            comment: String::new(),
            model_id: "m".to_string(),
            dim: 3,
            vectors: vec![mk("a")],
        };
        assert!(ReplayProvider::from_fixture(&wrong_dim).is_err());

        let dup = EmbeddingFixture {
            comment: String::new(),
            model_id: "m".to_string(),
            dim: 2,
            vectors: vec![mk("a"), mk("a")],
        };
        assert!(ReplayProvider::from_fixture(&dup).is_err());
    }
}
