//! Recall abstention with machine-readable reason codes (Vikunja #62).
//!
//! Recall used to always return the nearest match: the 5th-best candidate was
//! injected with the same framing as the 1st. The abstention gate declines to
//! answer instead — recall returns NO results plus one of these reason codes,
//! so every surface (CLI, MCP, HTTP, hooks, scorecard) can distinguish "looked
//! and responsibly declined" from an ordinary empty result set.
//!
//! The codes are separate because they demand different consumer reactions:
//! `no_candidates`/`filter_excluded` are corpus/query facts, `below_threshold`
//! is the calibrated quality gate, and `degraded_backend` asks for a warning
//! and a retry (the W1.6d posture).

use serde::{Deserialize, Serialize};

/// Why recall abstained instead of returning the nearest match (Vikunja #62).
///
/// Serialized snake_case on every surface that carries it: the proto
/// `Response::Recalled` frame (additive, no contract-version bump), the CLI
/// `--json` recall output, and the MCP `recall` tool's structured content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AbstainReason {
    /// No retrieval channel surfaced ANY candidate: the corpus (in scope) has
    /// nothing that even lexically, semantically, or graph-adjacently touches
    /// the query.
    NoCandidates,
    /// Candidates existed, but the best final blend score fell below the
    /// calibrated abstention threshold ([`rb_search::ABSTAIN_THRESHOLD`]) —
    /// a weak nearest match is not an answer.
    BelowThreshold,
    /// Candidates existed and were excluded by the caller's
    /// [`crate::RecallFilter`] — the query may be answerable without the
    /// filter.
    FilterExcluded,
    /// Retrieval ran DEGRADED (embedder outage, W1.6d) and nothing survived:
    /// the emptiness is blamed on the backend before any corpus conclusion.
    DegradedBackend,
}

impl AbstainReason {
    /// Stable machine-readable wire string (the serde name, exposed for
    /// surfaces that render the code without a serde serializer).
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            AbstainReason::NoCandidates => "no_candidates",
            AbstainReason::BelowThreshold => "below_threshold",
            AbstainReason::FilterExcluded => "filter_excluded",
            AbstainReason::DegradedBackend => "degraded_backend",
        }
    }
}

impl std::fmt::Display for AbstainReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Read-time corpus snapshot fingerprint (Vikunja #62): what corpus produced
/// this recall outcome, derived cheaply and read-only from the store — one
/// aggregate query, no schema change, no migration.
///
/// A preregistered eval or scorecard run records the fingerprint so its
/// results are pinned to an exact corpus state: any insert, update, or delete
/// in the namespace changes at least one component.
///
/// Components (all namespace-scoped):
/// - `memories` — row count (all states: active + archived),
/// - `generation` — `MAX(rowid)`: monotonically increasing with inserts,
/// - `last_write_epoch_s` — `MAX(updated_at)`: unix seconds of the newest
///   write, catching same-count rewrites (supersede, feedback, archive).
///
/// Not a content hash by design: hashing every row would cost a full corpus
/// scan on every recall. The triple pins the corpus state for the realistic
/// adversary of a pinned eval run — concurrent writes — not bit-level
/// tampering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CorpusSnapshot {
    /// Rows in the namespace, active and archived.
    pub memories: u64,
    /// `MAX(rowid)` in the namespace: increases with every insert.
    pub generation: u64,
    /// `MAX(updated_at)` in unix seconds; `0` for an empty namespace.
    pub last_write_epoch_s: i64,
}

impl CorpusSnapshot {
    /// The wire/render form: `"memories:generation:last_write_epoch_s"`.
    /// Two runs over the same corpus state produce the same string; any write
    /// visible to the snapshot query changes it.
    #[must_use]
    pub fn fingerprint(&self) -> String {
        format!("{}:{}:{}", self.memories, self.generation, self.last_write_epoch_s)
    }
}

impl std::fmt::Display for CorpusSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.fingerprint())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn reason_codes_serialize_snake_case_and_round_trip() {
        for (reason, wire) in [
            (AbstainReason::NoCandidates, "no_candidates"),
            (AbstainReason::BelowThreshold, "below_threshold"),
            (AbstainReason::FilterExcluded, "filter_excluded"),
            (AbstainReason::DegradedBackend, "degraded_backend"),
        ] {
            assert_eq!(serde_json::to_string(&reason).unwrap(), format!("\"{wire}\""));
            let back: AbstainReason = serde_json::from_str(&format!("\"{wire}\"")).unwrap();
            assert_eq!(back, reason);
            assert_eq!(reason.as_str(), wire);
            assert_eq!(reason.to_string(), wire);
        }
    }

    #[test]
    fn unknown_reason_code_fails_to_decode() {
        // Machine-readable means fail-closed on typos, not silent defaults.
        assert!(serde_json::from_str::<AbstainReason>("\"no_answer\"").is_err());
    }

    #[test]
    fn snapshot_fingerprint_is_stable_and_write_sensitive() {
        let a = CorpusSnapshot {
            memories: 12,
            generation: 34,
            last_write_epoch_s: 1_767_225_600,
        };
        assert_eq!(a.fingerprint(), "12:34:1767225600");
        assert_eq!(a.to_string(), "12:34:1767225600");
        // Every component participates.
        assert_ne!(
            a.fingerprint(),
            CorpusSnapshot {
                memories: 13,
                ..a
            }
            .fingerprint()
        );
        assert_ne!(
            a.fingerprint(),
            CorpusSnapshot {
                generation: 35,
                ..a
            }
            .fingerprint()
        );
        assert_ne!(
            a.fingerprint(),
            CorpusSnapshot {
                last_write_epoch_s: 1,
                ..a
            }
            .fingerprint()
        );
        // Round-trips through serde for the wire frame.
        let back: CorpusSnapshot = serde_json::from_str(&serde_json::to_string(&a).unwrap())
            .expect("snapshot is serializable");
        assert_eq!(back, a);
    }

    #[test]
    fn empty_corpus_snapshot_has_a_canonical_fingerprint() {
        let empty = CorpusSnapshot {
            memories: 0,
            generation: 0,
            last_write_epoch_s: 0,
        };
        assert_eq!(empty.fingerprint(), "0:0:0");
    }
}
