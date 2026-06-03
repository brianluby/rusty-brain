//! Fixture loader + validation.
//!
//! Fixtures are committed JSON under `fixtures/`. They use *stable string keys*
//! (e.g. `"single_writer"`) rather than UUIDs, because `engine.remember` mints a
//! fresh random `MemoryId` per note; the runner maps each fixture key to the
//! generated id after ingestion. Golden queries and dedup clusters reference
//! those keys, never UUIDs, so the corpus is human-authored and stable.
//!
//! Validation fails fast (returns `Err`) on any malformed fixture: unknown
//! memory type, out-of-range importance/confidence, duplicate keys, or golden
//! queries / clusters referencing unknown keys.

use rb_types::MemoryType;
use serde::Deserialize;
use std::collections::HashSet;

/// A single corpus memory authored by hand.
///
/// `memory_type` deserializes from the serde variant name (e.g. `"Insight"`).
#[derive(Debug, Clone, Deserialize)]
pub struct FixtureMemory {
    /// Stable handle referenced by golden queries and clusters.
    pub key: String,
    pub content: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub context: String,
    pub memory_type: MemoryType,
    pub importance: u8,
    /// Optional confidence override (0.0..=1.0); defaults to full trust.
    #[serde(default = "default_confidence")]
    pub confidence: f32,
}

fn default_confidence() -> f32 {
    1.0
}

/// A golden query: text plus the set of fixture keys that *should* rank highly.
#[derive(Debug, Clone, Deserialize)]
pub struct GoldenQuery {
    pub query: String,
    /// Fixture keys expected among the top results, best-relevance unordered.
    pub expected: Vec<String>,
    /// Optional `k` for this query's recall (defaults applied by the runner).
    #[serde(default)]
    pub k: Option<usize>,
}

/// A near-duplicate cluster: fixture keys that are mutually redundant.
#[derive(Debug, Clone, Deserialize)]
pub struct DedupCluster {
    pub members: Vec<String>,
}

/// The whole committed corpus: memories, golden queries, and dedup clusters.
#[derive(Debug, Clone, Deserialize)]
pub struct Corpus {
    pub memories: Vec<FixtureMemory>,
    pub golden_queries: Vec<GoldenQuery>,
    #[serde(default)]
    pub dedup_clusters: Vec<DedupCluster>,
}

/// Error surfaced when a fixture file is malformed or internally inconsistent.
#[derive(Debug)]
pub enum CorpusError {
    /// JSON failed to parse into the corpus shape.
    Parse(String),
    /// The corpus parsed but violates an invariant (the message explains which).
    Invalid(String),
}

impl std::fmt::Display for CorpusError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CorpusError::Parse(m) => write!(f, "fixture parse error: {m}"),
            CorpusError::Invalid(m) => write!(f, "fixture validation error: {m}"),
        }
    }
}

impl std::error::Error for CorpusError {}

impl Corpus {
    /// Parse a corpus from a JSON string and validate it. Fails fast.
    pub fn from_json(raw: &str) -> Result<Self, CorpusError> {
        let corpus: Corpus =
            serde_json::from_str(raw).map_err(|e| CorpusError::Parse(e.to_string()))?;
        corpus.validate()?;
        Ok(corpus)
    }

    /// Reject any internally inconsistent corpus.
    ///
    /// Invariants:
    /// - at least one memory and one golden query,
    /// - unique, non-empty memory keys with non-empty content,
    /// - importance in `1..=10`, confidence in `0.0..=1.0`,
    /// - every golden query references only known keys and has a non-empty
    ///   expected set with a positive `k` (if present),
    /// - every dedup cluster has >= 2 known members.
    fn validate(&self) -> Result<(), CorpusError> {
        if self.memories.is_empty() {
            return Err(CorpusError::Invalid("corpus has no memories".into()));
        }
        if self.golden_queries.is_empty() {
            return Err(CorpusError::Invalid("corpus has no golden queries".into()));
        }

        let mut keys: HashSet<&str> = HashSet::new();
        for m in &self.memories {
            if m.key.trim().is_empty() {
                return Err(CorpusError::Invalid("a memory has an empty key".into()));
            }
            if !keys.insert(m.key.as_str()) {
                return Err(CorpusError::Invalid(format!(
                    "duplicate memory key '{}'",
                    m.key
                )));
            }
            if m.content.trim().is_empty() {
                return Err(CorpusError::Invalid(format!(
                    "memory '{}' has empty content",
                    m.key
                )));
            }
            if !(1..=10).contains(&m.importance) {
                return Err(CorpusError::Invalid(format!(
                    "memory '{}' importance {} out of range 1..=10",
                    m.key, m.importance
                )));
            }
            if !m.confidence.is_finite() || !(0.0..=1.0).contains(&m.confidence) {
                return Err(CorpusError::Invalid(format!(
                    "memory '{}' confidence {} out of range 0.0..=1.0",
                    m.key, m.confidence
                )));
            }
        }

        for (qi, q) in self.golden_queries.iter().enumerate() {
            if q.query.trim().is_empty() {
                return Err(CorpusError::Invalid(format!(
                    "golden query {qi} has empty text"
                )));
            }
            if q.expected.is_empty() {
                return Err(CorpusError::Invalid(format!(
                    "golden query '{}' has no expected keys",
                    q.query
                )));
            }
            if let Some(k) = q.k {
                if k == 0 {
                    return Err(CorpusError::Invalid(format!(
                        "golden query '{}' has k = 0",
                        q.query
                    )));
                }
            }
            let mut expected_seen: HashSet<&str> = HashSet::new();
            for key in &q.expected {
                if !expected_seen.insert(key.as_str()) {
                    return Err(CorpusError::Invalid(format!(
                        "golden query '{}' has duplicate expected key '{key}'",
                        q.query
                    )));
                }
                if !keys.contains(key.as_str()) {
                    return Err(CorpusError::Invalid(format!(
                        "golden query '{}' references unknown key '{key}'",
                        q.query
                    )));
                }
            }
        }

        for (ci, cluster) in self.dedup_clusters.iter().enumerate() {
            if cluster.members.len() < 2 {
                return Err(CorpusError::Invalid(format!(
                    "dedup cluster {ci} needs >= 2 members"
                )));
            }
            let mut member_seen: HashSet<&str> = HashSet::new();
            for key in &cluster.members {
                if !member_seen.insert(key.as_str()) {
                    return Err(CorpusError::Invalid(format!(
                        "dedup cluster {ci} has duplicate member '{key}'"
                    )));
                }
                if !keys.contains(key.as_str()) {
                    return Err(CorpusError::Invalid(format!(
                        "dedup cluster {ci} references unknown key '{key}'"
                    )));
                }
            }
        }

        Ok(())
    }
}

/// Load and validate the committed corpus bundled at compile time.
///
/// The fixtures are embedded with `include_str!` so the harness has no runtime
/// filesystem dependency and runs identically in CI and locally.
pub fn load_committed_corpus() -> Result<Corpus, CorpusError> {
    const RAW: &str = include_str!("../fixtures/corpus.json");
    Corpus::from_json(RAW)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = r#"{
        "memories": [
            {"key": "a", "content": "alpha content", "memory_type": "Insight", "importance": 5},
            {"key": "b", "content": "beta content", "memory_type": "Reference", "importance": 3}
        ],
        "golden_queries": [
            {"query": "alpha", "expected": ["a"]}
        ],
        "dedup_clusters": [
            {"members": ["a", "b"]}
        ]
    }"#;

    #[test]
    fn parses_and_validates_minimal_corpus() {
        let c = Corpus::from_json(MINIMAL).unwrap();
        assert_eq!(c.memories.len(), 2);
        assert_eq!(c.golden_queries.len(), 1);
        assert_eq!(c.dedup_clusters.len(), 1);
        // confidence defaults to 1.0 when omitted
        assert!((c.memories[0].confidence - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn rejects_unknown_memory_type() {
        let bad = MINIMAL.replace("\"Insight\"", "\"NotAType\"");
        assert!(matches!(
            Corpus::from_json(&bad),
            Err(CorpusError::Parse(_))
        ));
    }

    #[test]
    fn rejects_duplicate_keys() {
        let bad = r#"{
            "memories": [
                {"key": "a", "content": "x", "memory_type": "Insight", "importance": 5},
                {"key": "a", "content": "y", "memory_type": "Insight", "importance": 5}
            ],
            "golden_queries": [{"query": "x", "expected": ["a"]}]
        }"#;
        let err = Corpus::from_json(bad).unwrap_err();
        assert!(matches!(err, CorpusError::Invalid(_)));
    }

    #[test]
    fn rejects_out_of_range_importance() {
        let bad = MINIMAL.replace("\"importance\": 5", "\"importance\": 0");
        assert!(matches!(
            Corpus::from_json(&bad),
            Err(CorpusError::Invalid(_))
        ));
    }

    #[test]
    fn rejects_out_of_range_confidence() {
        let bad = r#"{
            "memories": [
                {"key": "a", "content": "x", "memory_type": "Insight", "importance": 5, "confidence": 1.5}
            ],
            "golden_queries": [{"query": "x", "expected": ["a"]}]
        }"#;
        assert!(matches!(
            Corpus::from_json(bad),
            Err(CorpusError::Invalid(_))
        ));
    }

    #[test]
    fn rejects_golden_query_referencing_unknown_key() {
        let bad = MINIMAL.replace("\"expected\": [\"a\"]", "\"expected\": [\"zzz\"]");
        let err = Corpus::from_json(&bad).unwrap_err();
        assert!(matches!(err, CorpusError::Invalid(_)));
    }

    #[test]
    fn rejects_cluster_referencing_unknown_key() {
        let bad = MINIMAL.replace(
            "\"members\": [\"a\", \"b\"]",
            "\"members\": [\"a\", \"zzz\"]",
        );
        let err = Corpus::from_json(&bad).unwrap_err();
        assert!(matches!(err, CorpusError::Invalid(_)));
    }

    #[test]
    fn rejects_duplicate_expected_key_in_golden_query() {
        // A duplicate within one query's expected list would double-count in
        // recall@k / mrr, so it is rejected even though the key is known.
        let bad = MINIMAL.replace("\"expected\": [\"a\"]", "\"expected\": [\"a\", \"a\"]");
        let err = Corpus::from_json(&bad).unwrap_err();
        assert!(matches!(err, CorpusError::Invalid(_)));
    }

    #[test]
    fn rejects_duplicate_member_in_dedup_cluster() {
        // A duplicate cluster member would skew dedup_precision; reject it.
        let bad = MINIMAL.replace("\"members\": [\"a\", \"b\"]", "\"members\": [\"a\", \"a\"]");
        let err = Corpus::from_json(&bad).unwrap_err();
        assert!(matches!(err, CorpusError::Invalid(_)));
    }

    #[test]
    fn rejects_cluster_with_one_member() {
        let bad = MINIMAL.replace("\"members\": [\"a\", \"b\"]", "\"members\": [\"a\"]");
        let err = Corpus::from_json(&bad).unwrap_err();
        assert!(matches!(err, CorpusError::Invalid(_)));
    }

    #[test]
    fn committed_corpus_loads_and_validates() {
        // The shipped fixtures must always be well-formed.
        let c = load_committed_corpus().unwrap();
        assert!(c.memories.len() >= 8, "corpus should be non-trivial");
        assert!(!c.golden_queries.is_empty());
        assert!(!c.dedup_clusters.is_empty());
    }
}
