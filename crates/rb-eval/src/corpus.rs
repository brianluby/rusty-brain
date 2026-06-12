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
    /// Relevance grades aligned index-for-index with `expected`:
    /// `3` = primary answer, `2` = substantially relevant, `1` = marginally
    /// relevant. Empty means ungraded (legacy / hand-built test corpora); when
    /// present there must be exactly one grade per expected key, each in
    /// `1..=3`. Grades are authored *content* — the current runner's binary
    /// recall@k / MRR ignore them; graded metrics consume them later (W1.0b).
    #[serde(default)]
    pub grades: Vec<u8>,
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
            if !q.grades.is_empty() {
                if q.grades.len() != q.expected.len() {
                    return Err(CorpusError::Invalid(format!(
                        "golden query '{}' has {} grades for {} expected keys",
                        q.query,
                        q.grades.len(),
                        q.expected.len()
                    )));
                }
                if let Some(bad) = q.grades.iter().find(|g| !(1..=3).contains(*g)) {
                    return Err(CorpusError::Invalid(format!(
                        "golden query '{}' has grade {bad} out of range 1..=3",
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

/// Load and validate the committed **held-out** query set.
///
/// The held-out queries live in `fixtures/holdout_queries.json`, are graded
/// exactly like golden queries, and reference the committed corpus's memory
/// keys. **They must never be used for weight tuning or threshold selection**
/// — they exist so Phase 4 (W4.1) can gate CI on queries no tuning loop has
/// ever seen. Validation substitutes them into the committed corpus so every
/// key reference and grade obeys the same invariants as the tuning goldens.
pub fn load_committed_holdout_queries() -> Result<Vec<GoldenQuery>, CorpusError> {
    const RAW: &str = include_str!("../fixtures/holdout_queries.json");

    #[derive(Deserialize)]
    struct HoldoutFile {
        golden_queries: Vec<GoldenQuery>,
    }

    let parsed: HoldoutFile =
        serde_json::from_str(RAW).map_err(|e| CorpusError::Parse(e.to_string()))?;
    let mut shadow = load_committed_corpus()?;
    shadow.golden_queries = parsed.golden_queries;
    shadow.validate()?;
    Ok(shadow.golden_queries)
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
    fn rejects_grade_count_mismatch() {
        let bad = MINIMAL.replace(
            "\"expected\": [\"a\"]",
            "\"expected\": [\"a\"], \"grades\": [3, 2]",
        );
        let err = Corpus::from_json(&bad).unwrap_err();
        assert!(matches!(err, CorpusError::Invalid(_)));
    }

    #[test]
    fn rejects_out_of_range_grade() {
        for bad_grade in ["0", "4"] {
            let bad = MINIMAL.replace(
                "\"expected\": [\"a\"]",
                &format!("\"expected\": [\"a\"], \"grades\": [{bad_grade}]"),
            );
            let err = Corpus::from_json(&bad).unwrap_err();
            assert!(matches!(err, CorpusError::Invalid(_)));
        }
    }

    #[test]
    fn accepts_graded_golden_query() {
        let graded = MINIMAL.replace(
            "\"expected\": [\"a\"]",
            "\"expected\": [\"a\"], \"grades\": [3]",
        );
        let c = Corpus::from_json(&graded).unwrap();
        assert_eq!(c.golden_queries[0].grades, vec![3]);
        // Ungraded legacy corpora still parse (grades default to empty).
        let legacy = Corpus::from_json(MINIMAL).unwrap();
        assert!(legacy.golden_queries[0].grades.is_empty());
    }

    #[test]
    fn committed_corpus_loads_and_validates() {
        // The shipped fixtures must always be well-formed, and the Phase 1
        // corpus must meet the W1.0 size floor: >= 200 memories and >= 50
        // graded natural-language golden queries.
        let c = load_committed_corpus().unwrap();
        assert!(
            c.memories.len() >= 200,
            "phase-1 corpus needs >= 200 memories, got {}",
            c.memories.len()
        );
        assert!(
            c.golden_queries.len() >= 50,
            "phase-1 corpus needs >= 50 golden queries, got {}",
            c.golden_queries.len()
        );
        let graded = c
            .golden_queries
            .iter()
            .filter(|q| !q.grades.is_empty())
            .count();
        assert!(
            graded >= 50,
            "phase-1 corpus needs >= 50 GRADED golden queries, got {graded}"
        );
        assert!(!c.dedup_clusters.is_empty());
    }

    #[test]
    fn committed_corpus_contains_readme_quickstart_golden() {
        // Phase 1 gate (plan section 4): "the README quickstart query returns
        // its target memory". The corpus must therefore carry the quickstart
        // query VERBATIM as a golden, and its target memory verbatim from the
        // README's `remember` example. Retrieval quality on it is gated later
        // (after W1.2/W1.4); this test pins the *content* contract.
        let c = load_committed_corpus().unwrap();
        let golden = c
            .golden_queries
            .iter()
            .find(|q| q.query == "how is writing serialized?")
            .expect("README quickstart query must be a committed golden");
        assert!(
            golden.expected.iter().any(|k| k == "readme_quickstart"),
            "quickstart golden must expect its README target memory"
        );
        let target = c
            .memories
            .iter()
            .find(|m| m.key == "readme_quickstart")
            .expect("README quickstart target memory must exist");
        assert_eq!(
            target.content,
            "We use a single-writer daemon over SQLite WAL"
        );
    }

    #[test]
    fn committed_holdout_set_loads_and_stays_disjoint() {
        // The held-out set (>= 15 graded queries, separate file) must always
        // validate against the committed corpus and must never overlap the
        // tuning goldens — it is reserved for the W4.1 CI gate and MUST NOT
        // be used for weight tuning.
        let holdout = load_committed_holdout_queries().unwrap();
        assert!(
            holdout.len() >= 15,
            "held-out set needs >= 15 queries, got {}",
            holdout.len()
        );
        assert!(
            holdout.iter().all(|q| !q.grades.is_empty()),
            "every held-out query must be graded"
        );

        let tuning = load_committed_corpus().unwrap();
        let tuning_texts: HashSet<&str> = tuning
            .golden_queries
            .iter()
            .map(|q| q.query.as_str())
            .collect();
        for q in &holdout {
            assert!(
                !tuning_texts.contains(q.query.as_str()),
                "held-out query '{}' duplicates a tuning golden",
                q.query
            );
        }
    }
}
