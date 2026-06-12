//! Record/replay of REAL embedding vectors as committed fixtures (W1.0).
//!
//! The deterministic provider guards regressions but cannot show semantic
//! quality; live API calls cannot run in CI. This module bridges the two:
//! a **recording** pass (run manually, with credentials/network) captures every
//! vector a real model produces for the committed corpus, and a **replay**
//! provider serves those committed vectors with zero network and zero keys —
//! the mode CI and later workstreams use for semantic measurement.
//!
//! ## Key shape (W1.4)
//!
//! Each recorded vector is keyed on `(model_id, input_kind, sha256(text))`.
//! `input_kind` is `"document"` or `"query"` ([`rb_embed::EmbedKind::as_str`]):
//! recordings made after W1.4 capture each text under the kind the engine
//! embedded it with (corpus documents as `"document"`, golden/holdout query
//! strings as `"query"`). Fixtures recorded BEFORE W1.4 hold only
//! `"document"` entries; for those, [`ReplayProvider`] serves a query-kind
//! lookup from the document-kind vector with a logged warning — exact for
//! kind-blind models (the committed all-MiniLM-L6-v2 fixture), and the
//! deterministic fallback the plan prescribes until a Voyage fixture with
//! true query-side vectors is recorded.
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

use rb_embed::{EmbedKind, EmbeddingProvider};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

/// Fixture key component for write-side (document) vectors.
pub const INPUT_KIND_DOCUMENT: &str = "document";
/// Fixture key component for recall-side (query) vectors (W1.4).
pub const INPUT_KIND_QUERY: &str = "query";

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

    /// Delegates to the wrapped provider with the SAME kind, and records each
    /// vector under that kind's fixture key (`kind.as_str()`, W1.4) — so a
    /// recording run captures query vectors as `"query"` and document vectors
    /// as `"document"`, exactly as replay will look them up.
    async fn embed(&self, texts: &[String], kind: EmbedKind) -> rb_types::Result<Vec<Vec<f32>>> {
        let vectors = self.inner.embed(texts, kind).await?;
        let mut recorded = self
            .recorded
            .lock()
            .map_err(|_| rb_types::Error::Embedding("recording mutex poisoned".into()))?;
        for (text, vector) in texts.iter().zip(vectors.iter()) {
            let preview: String = text.chars().take(80).collect();
            recorded.insert(
                (kind.as_str().to_string(), text_sha256(text)),
                (preview, vector.clone()),
            );
        }
        Ok(vectors)
    }
}

/// Serves committed real-model vectors with zero network and zero keys.
///
/// `embed` FAILS CLOSED on any text absent from the fixture: corpus drift must
/// force a re-recording, never silently degrade to wrong vectors. The single
/// sanctioned exception (W1.4): a `Query`-kind lookup missing from the fixture
/// is served from the same text's `"document"` entry, with a logged warning
/// and a [`ReplayProvider::query_fallbacks`] count — pre-W1.4 fixtures hold
/// only document vectors, and for kind-blind models the fallback is exact.
pub struct ReplayProvider {
    model_id: String,
    dim: usize,
    vectors: HashMap<FixtureKey, Vec<f32>>,
    /// How many query-kind lookups were served via the document fallback.
    query_fallbacks: AtomicUsize,
    /// Ensures the fallback warning is logged once per provider, not per query.
    fallback_warned: AtomicUsize,
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
            query_fallbacks: AtomicUsize::new(0),
            fallback_warned: AtomicUsize::new(0),
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

    /// How many `Query`-kind lookups were served from `"document"` entries
    /// (W1.4 fallback). Zero once the fixture carries true query-kind vectors.
    pub fn query_fallbacks(&self) -> usize {
        self.query_fallbacks.load(Ordering::Relaxed)
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

    async fn embed(&self, texts: &[String], kind: EmbedKind) -> rb_types::Result<Vec<Vec<f32>>> {
        texts
            .iter()
            .map(|text| {
                let sha = text_sha256(text);
                let key = (kind.as_str().to_string(), sha.clone());
                if let Some(v) = self.vectors.get(&key) {
                    return Ok(v.clone());
                }
                // W1.4 fallback: a query-kind miss may be served from the
                // document-kind entry (pre-W1.4 fixtures recorded queries as
                // documents; for kind-blind models the vectors are identical).
                // Document-kind misses NEVER fall back — fail closed.
                if kind == EmbedKind::Query {
                    let doc_key = (INPUT_KIND_DOCUMENT.to_string(), sha.clone());
                    if let Some(v) = self.vectors.get(&doc_key) {
                        self.query_fallbacks.fetch_add(1, Ordering::Relaxed);
                        if self.fallback_warned.swap(1, Ordering::Relaxed) == 0 {
                            eprintln!(
                                "rb-eval replay WARNING: fixture for model '{}' has no \
                                 '{INPUT_KIND_QUERY}' vectors; serving query lookups from \
                                 '{INPUT_KIND_DOCUMENT}' entries (exact for kind-blind models; \
                                 re-record with a query-differentiating provider to remove \
                                 this fallback)",
                                self.model_id
                            );
                        }
                        return Ok(v.clone());
                    }
                }
                let preview: String = text.chars().take(80).collect();
                Err(rb_types::Error::Embedding(format!(
                    "replay miss: no recorded '{}' vector for sha256 {sha} \
                     (text preview: {preview:?}). The corpus drifted from the committed \
                     fixture — re-record it (see rb-eval/src/replay.rs module docs)",
                    kind.as_str()
                )))
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
        let original = recorder.embed(&texts, EmbedKind::Document).await.unwrap();

        let fixture = recorder.fixture("test fixture").unwrap();
        assert_eq!(fixture.model_id, "deterministic");
        assert_eq!(fixture.dim, 8);
        assert_eq!(fixture.vectors.len(), 2);
        assert!(fixture
            .vectors
            .iter()
            .all(|v| v.input_kind == INPUT_KIND_DOCUMENT));

        let replay = ReplayProvider::from_fixture(&fixture).unwrap();
        let replayed = replay.embed(&texts, EmbedKind::Document).await.unwrap();
        assert_eq!(original, replayed, "replay must be bit-identical");
    }

    #[tokio::test]
    async fn query_kind_vectors_are_recorded_and_replayed_under_query() {
        // W1.4: a recording run embeds queries with EmbedKind::Query; the
        // fixture entry is keyed "query" and replay serves it kind-for-kind
        // with ZERO fallbacks.
        let recorder = RecordingProvider::new(DeterministicProvider::new(8));
        let queries = vec!["how is writing serialized?".to_string()];
        let original = recorder.embed(&queries, EmbedKind::Query).await.unwrap();

        let fixture = recorder.fixture("t").unwrap();
        assert_eq!(fixture.vectors.len(), 1);
        assert_eq!(fixture.vectors[0].input_kind, INPUT_KIND_QUERY);

        let replay = ReplayProvider::from_fixture(&fixture).unwrap();
        let replayed = replay.embed(&queries, EmbedKind::Query).await.unwrap();
        assert_eq!(original, replayed, "query replay must be bit-identical");
        assert_eq!(
            replay.query_fallbacks(),
            0,
            "a true query-kind entry must not count as a fallback"
        );
    }

    #[tokio::test]
    async fn query_lookup_falls_back_to_document_vector_with_a_count() {
        // Pre-W1.4 fixtures recorded query texts under "document". A Query
        // lookup must be served from that entry (identical bytes) and counted,
        // keeping eval deterministic until a query-side fixture is recorded.
        let recorder = RecordingProvider::new(DeterministicProvider::new(8));
        let texts = vec!["legacy recorded as document".to_string()];
        let as_document = recorder.embed(&texts, EmbedKind::Document).await.unwrap();

        let replay = ReplayProvider::from_fixture(&recorder.fixture("t").unwrap()).unwrap();
        let as_query = replay.embed(&texts, EmbedKind::Query).await.unwrap();
        assert_eq!(
            as_document, as_query,
            "the fallback must serve the document vector bytes"
        );
        assert_eq!(replay.query_fallbacks(), 1, "fallback must be counted");
    }

    #[tokio::test]
    async fn replay_fails_closed_on_unrecorded_text() {
        let recorder = RecordingProvider::new(DeterministicProvider::new(8));
        recorder
            .embed(&["known".to_string()], EmbedKind::Document)
            .await
            .unwrap();
        let replay = ReplayProvider::from_fixture(&recorder.fixture("t").unwrap()).unwrap();

        // Unknown text fails closed for BOTH kinds (the query fallback only
        // applies when a document vector for the same text exists).
        for kind in [EmbedKind::Document, EmbedKind::Query] {
            let err = replay
                .embed(&["never recorded".to_string()], kind)
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
        assert_eq!(replay.query_fallbacks(), 0);
    }

    #[tokio::test]
    async fn document_lookup_never_falls_back_to_a_query_entry() {
        // The fixture key includes input_kind: a vector recorded as "query"
        // must NOT satisfy a "document" lookup — persisted document vectors
        // failing closed is what forces a re-record on corpus drift.
        let recorder = RecordingProvider::new(DeterministicProvider::new(8));
        recorder
            .embed(&["text".to_string()], EmbedKind::Query)
            .await
            .unwrap();
        let fixture = recorder.fixture("t").unwrap();
        assert_eq!(fixture.vectors[0].input_kind, INPUT_KIND_QUERY);
        let replay = ReplayProvider::from_fixture(&fixture).unwrap();
        let err = replay
            .embed(&["text".to_string()], EmbedKind::Document)
            .await
            .unwrap_err();
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
