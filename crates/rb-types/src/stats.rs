//! Read-only observability aggregates (doctor/stats PRD). Lives in `rb-types`
//! (the leaf crate) so `rb-store` (the aggregation queries), `rb-proto` (the
//! wire `Response::Stats`) and the CLI (rendering) can share one definition
//! without a dependency cycle — the `MemoryChanged` precedent.

use crate::MemoryId;
use serde::{Deserialize, Serialize};

/// Namespace-scoped value/health aggregate computed on the daemon's READ pool
/// (W1.8: the stats path issues zero writer ops). Counts and ids only — never
/// memory content, so stats output cannot leak a stored secret.
///
/// Every field is `#[serde(default)]` so the payload stays additive: a peer
/// that predates (or postdates) a field decodes the rest — the
/// `RecallChannelTotals` precedent, no CONTRACT_VERSION bump.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
pub struct MemoryStats {
    /// Namespace the aggregate is scoped to (db string form).
    #[serde(default)]
    pub namespace: String,
    /// Bounded window, in days, for the windowed fields below.
    #[serde(default)]
    pub window_days: u32,
    /// The daemon's database file path (the authoritative one — the daemon may
    /// have been started with a different env than this client).
    #[serde(default)]
    pub db_path: String,
    /// Active (non-archived) memories in the namespace.
    #[serde(default)]
    pub live: u64,
    /// Archived memories in the namespace.
    #[serde(default)]
    pub archived: u64,
    /// Rows in the vec0 vector index (whole DB — the index is one table).
    #[serde(default)]
    pub vectors: u64,
    /// Size of the SQLite `-wal` sidecar in bytes (0 when absent).
    #[serde(default)]
    pub wal_bytes: u64,
    /// `meta.embedding_model` — the DB's recorded embedding-model identity
    /// (the W0.2 fail-closed contract). `None` on a legacy DB with no stamp.
    #[serde(default)]
    pub db_embedding_model: Option<String>,
    /// All-time usefulness-feedback totals for the namespace (W3.7).
    #[serde(default)]
    pub feedback: FeedbackTotals,
    /// Sum of `access_count` over the namespace's memories (recall volume,
    /// cumulative — access bumps are the only recall-derived counter, W1.8).
    #[serde(default)]
    pub total_accesses: u64,
    /// Live memories accessed within the window (`last_accessed_at` based).
    #[serde(default)]
    pub accessed_in_window: u64,
    /// Most-recalled live memories (ids + counts only), bounded by the server.
    #[serde(default)]
    pub top_recalled: Vec<TopRecalled>,
    /// Live memories recall has NEVER returned (`access_count = 0`).
    #[serde(default)]
    pub never_recalled_live: u64,
    /// Live memories with at least one active `contradicts` link whose both
    /// endpoints are active and in this namespace (the `contested` read flag).
    #[serde(default)]
    pub contested: u64,
    /// Live memories with confidence below the review bound
    /// (`REVIEW_LOW_CONFIDENCE_BOUND`, exclusive) — with `contested` and
    /// `never_recalled_live`, the review-queue trend (PRD 2026-07-02 REV-4;
    /// the near-dup tier is deliberately not counted here: it would cost one
    /// KNN probe per live memory on every stats call). Additive
    /// `#[serde(default)]` field, the `retention_eligible` precedent.
    #[serde(default)]
    pub low_confidence_live: u64,
    /// Corpus growth: memories created per UTC day inside the window, oldest
    /// first. Days with no writes are omitted.
    #[serde(default)]
    pub created_per_day: Vec<GrowthBucket>,
    /// Active memories whose embedding stamp is stale relative to the current
    /// `(model, input_version)` — the re-embed backlog (P5 Feature A scan).
    #[serde(default)]
    pub reembed_pending: u64,
    /// Apply-mode forget candidates under the daemon's `[retention]` policy
    /// (retention PRD RET-4). `None` = no policy configured (distinguishes
    /// "no policy" from "0 eligible").
    #[serde(default)]
    pub retention_eligible: Option<u64>,
    /// Unix seconds of the namespace's last retention sweep (from the bulk
    /// `retention_sweep` oplog row). `None` = never swept.
    #[serde(default)]
    pub last_forget_at: Option<i64>,
}

/// All-time helpful/wrong/stale counts from the `memory_feedback` event log.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FeedbackTotals {
    #[serde(default)]
    pub helpful: u64,
    #[serde(default)]
    pub wrong: u64,
    #[serde(default)]
    pub stale: u64,
}

impl FeedbackTotals {
    /// Net trust trend: helpful minus the two negative signals. Saturating on
    /// the (absurd) overflow edge rather than panicking in a render path.
    pub fn net(&self) -> i64 {
        let pos = i64::try_from(self.helpful).unwrap_or(i64::MAX);
        let neg = i64::try_from(self.wrong.saturating_add(self.stale)).unwrap_or(i64::MAX);
        pos.saturating_sub(neg)
    }

    /// Total feedback events (drives the low-N caveat in rendering).
    pub fn total(&self) -> u64 {
        self.helpful
            .saturating_add(self.wrong)
            .saturating_add(self.stale)
    }
}

/// One top-recalled entry: id and its cumulative access count — never content.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct TopRecalled {
    pub id: MemoryId,
    pub access_count: u64,
}

/// One corpus-growth bucket: memories created on `day` (UTC, `YYYY-MM-DD`).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct GrowthBucket {
    pub day: String,
    pub count: u64,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::MemoryId;

    #[test]
    fn memory_stats_round_trips_with_every_field() {
        let stats = MemoryStats {
            namespace: "project:rusty-brain".to_string(),
            window_days: 30,
            db_path: "/tmp/rb.db".to_string(),
            live: 12,
            archived: 3,
            vectors: 12,
            wal_bytes: 4096,
            db_embedding_model: Some("deterministic".to_string()),
            feedback: FeedbackTotals {
                helpful: 5,
                wrong: 1,
                stale: 2,
            },
            total_accesses: 40,
            accessed_in_window: 7,
            top_recalled: vec![TopRecalled {
                id: MemoryId::new(),
                access_count: 9,
            }],
            never_recalled_live: 4,
            contested: 1,
            low_confidence_live: 2,
            created_per_day: vec![GrowthBucket {
                day: "2026-07-10".to_string(),
                count: 2,
            }],
            reembed_pending: 3,
            retention_eligible: Some(4),
            last_forget_at: Some(1_700_000_000),
        };
        let json = serde_json::to_string(&stats).unwrap();
        let back: MemoryStats = serde_json::from_str(&json).unwrap();
        assert_eq!(back, stats);
    }

    #[test]
    fn memory_stats_decodes_from_an_empty_object() {
        // Every field is additive + serde(default): a payload from an
        // older/newer peer that omits fields must still decode (the
        // RecallChannelTotals precedent), yielding the zero-valued default.
        let back: MemoryStats = serde_json::from_str("{}").unwrap();
        assert_eq!(back, MemoryStats::default());
        assert_eq!(back.feedback.helpful, 0);
        assert!(back.top_recalled.is_empty());
        assert!(back.db_embedding_model.is_none());
    }

    #[test]
    fn retention_fields_are_additive_and_default_to_none() {
        // A stats payload from a daemon that predates the retention PRD (or
        // has no [retention] policy) must decode with both fields None —
        // distinguishing "no policy" from "0 eligible".
        let back: MemoryStats = serde_json::from_str("{}").unwrap();
        assert_eq!(back.retention_eligible, None);
        assert_eq!(back.last_forget_at, None);

        let with: MemoryStats =
            serde_json::from_str(r#"{"retention_eligible": 4, "last_forget_at": 1700000000}"#)
                .unwrap();
        assert_eq!(with.retention_eligible, Some(4));
        assert_eq!(with.last_forget_at, Some(1_700_000_000));
    }

    #[test]
    fn feedback_totals_net_and_total() {
        let fb = FeedbackTotals {
            helpful: 5,
            wrong: 1,
            stale: 2,
        };
        // Net trust trend: helpful minus the two negative signals.
        assert_eq!(fb.net(), 2);
        assert_eq!(fb.total(), 8);
        // Net can go negative and must not underflow.
        let bad = FeedbackTotals {
            helpful: 0,
            wrong: 3,
            stale: 1,
        };
        assert_eq!(bad.net(), -4);
        assert_eq!(FeedbackTotals::default().total(), 0);
    }
}
