//! Guided-review vocabulary (PRD 2026-07-02 contradiction/dedup review).
//!
//! Lives in `rb-types` (the leaf crate) so `rb-store` (the queue generator
//! and snooze persistence), `rb-proto` (the wire `Request::Review` /
//! `Request::Resolve`), `rb-daemon` (the resolution orchestrator) and the CLI
//! (rendering + the interactive loop) share one definition — the
//! `RetentionPolicy`/`ForgetPlan` precedent. Every wire field is
//! `#[serde(default)]` so the shapes stay additive (no CONTRACT_VERSION bump).

use crate::error::{Error, Result};
use crate::memory_id::MemoryId;
use serde::{Deserialize, Serialize};

/// PRD REV-1 tier 3: live memories with confidence strictly below this bound
/// enter the review queue.
pub const REVIEW_LOW_CONFIDENCE_BOUND: f32 = 0.4;
/// Default near-duplicate similarity threshold — the consolidation job's
/// documented conservative 0.95 (raw cosine similarity, the
/// `near_duplicates()` unit). One knob, one definition.
pub const REVIEW_DEFAULT_THRESHOLD: f32 = 0.95;
/// Floor for a caller-supplied threshold: the server clamps below this so a
/// wild client cannot turn the whole corpus into "duplicates" (PRD risk:
/// destructive auto-merge on false positives).
pub const REVIEW_MIN_THRESHOLD: f32 = 0.80;
/// Tier 3 staleness horizon: a live memory never recalled and older than
/// this many days is a stale candidate (mirrors the stats
/// `never_recalled_live` predicate plus an age bound).
pub const REVIEW_STALE_DAYS: u32 = 30;
/// `keep --bump` confidence delta — deliberately the `FeedbackKind::Helpful`
/// magnitude, clamped to `0.0..=1.0` on apply.
pub const REVIEW_KEEP_BUMP: f32 = 0.05;
/// `demote` confidence delta — deliberately the `FeedbackKind::Stale`
/// magnitude (a review demotion means "outdated", not "wrong"), clamped to
/// `0.0..=1.0` on apply.
pub const REVIEW_DEMOTE_STEP: f32 = 0.15;
/// Default snooze window (REV-2): a snoozed item stays hidden this long.
pub const REVIEW_DEFAULT_SNOOZE_DAYS: u32 = 7;
/// Hard ceiling for a snooze window so a typo cannot hide an item forever.
pub const REVIEW_MAX_SNOOZE_DAYS: u32 = 365;
/// Default bound on queue items per sweep (the `batch_limit` precedent).
pub const REVIEW_DEFAULT_LIMIT: u32 = 50;
/// Hard ceiling for the per-sweep item bound.
pub const REVIEW_MAX_LIMIT: u32 = 500;

/// Why an item is in the review queue (REV-1, in priority order).
#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReviewReason {
    /// An active `contradicts` pair (`contested` memories) — priority 1.
    #[default]
    Contradiction,
    /// A near-duplicate pair from `near_duplicates()` — priority 2.
    NearDuplicate,
    /// A live memory with confidence below
    /// [`REVIEW_LOW_CONFIDENCE_BOUND`] — priority 3.
    LowConfidence,
    /// A live memory never recalled and older than
    /// [`REVIEW_STALE_DAYS`] — priority 3 (after low-confidence).
    StaleNeverRecalled,
}

impl ReviewReason {
    /// Stable string form — the key prefix and the oplog/JSON vocabulary.
    pub fn as_str(&self) -> &'static str {
        match self {
            ReviewReason::Contradiction => "contradiction",
            ReviewReason::NearDuplicate => "near_duplicate",
            ReviewReason::LowConfidence => "low_confidence",
            ReviewReason::StaleNeverRecalled => "stale_never_recalled",
        }
    }
}

/// Canonical, order-independent key for a review item: the reason prefix plus
/// the member ids sorted lexicographically. This is the `review_state`
/// primary key (snooze persistence), so the same pair discovered from either
/// anchor maps to one row, and a snoozed dup pair can never hide a later
/// contradiction over the same ids.
pub fn review_item_key(reason: ReviewReason, ids: &[MemoryId]) -> String {
    let mut sorted: Vec<String> = ids.iter().map(|id| id.to_string()).collect();
    sorted.sort();
    format!("{}:{}", reason.as_str(), sorted.join(":"))
}

/// One side of a review item — everything the human decision needs (REV-1:
/// summary, importance, confidence, age, provenance). Counts, ids, and the
/// stored summary only; renderers redact the summary before display.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
pub struct ReviewMember {
    #[serde(default)]
    pub id: MemoryId,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub importance: u8,
    #[serde(default)]
    pub confidence: f32,
    /// Unix seconds.
    #[serde(default)]
    pub created_at: i64,
    /// Whole days since creation at plan time (display convenience).
    #[serde(default)]
    pub age_days: u32,
    /// Unix seconds of the last recall access, if any.
    #[serde(default)]
    pub last_accessed_at: Option<i64>,
    #[serde(default)]
    pub origin_source: Option<String>,
    #[serde(default)]
    pub origin_user: Option<String>,
}

/// One queue entry: a contradiction pair, a near-dup pair, or a single
/// low-confidence/stale memory.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
pub struct ReviewItem {
    /// [`review_item_key`] of this item — the snooze identity.
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub reason: ReviewReason,
    /// Two members for pairs, one for tier-3 singles.
    #[serde(default)]
    pub members: Vec<ReviewMember>,
    /// Raw cosine similarity (near-duplicate items only).
    #[serde(default)]
    pub similarity: Option<f32>,
}

/// A documented non-interactive policy (REV-3 `--policy`).
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReviewPolicy {
    /// Merge every near-duplicate pair into one combined superseding memory.
    AutoMergeDups,
    /// Demote (lower confidence) every low-confidence item.
    DemoteLowConfidence,
}

impl ReviewPolicy {
    /// Documented (kebab-case) name, as the CLI help spells it.
    pub fn as_str(&self) -> &'static str {
        match self {
            ReviewPolicy::AutoMergeDups => "auto-merge-dups",
            ReviewPolicy::DemoteLowConfidence => "demote-low-confidence",
        }
    }

    /// Parse a policy name (kebab or snake case). Fails closed listing the
    /// documented policies — a typo must never select a different sweep.
    pub fn parse(s: &str) -> Result<Self> {
        match s.trim().replace('_', "-").as_str() {
            "auto-merge-dups" => Ok(ReviewPolicy::AutoMergeDups),
            "demote-low-confidence" => Ok(ReviewPolicy::DemoteLowConfidence),
            other => Err(Error::InvalidArgument(format!(
                "unknown review policy '{other}': expected auto-merge-dups or \
                 demote-low-confidence"
            ))),
        }
    }

    /// The action this policy takes on `item`, or `None` when the item is out
    /// of the policy's scope. Pure — dry-run and apply both derive their plan
    /// from this one function, so they cannot diverge (REV-3 determinism).
    pub fn plan_action(&self, item: &ReviewItem) -> Option<ReviewAction> {
        match (self, item.reason) {
            (ReviewPolicy::AutoMergeDups, ReviewReason::NearDuplicate)
                if item.members.len() == 2 =>
            {
                Some(ReviewAction::Merge)
            }
            (ReviewPolicy::DemoteLowConfidence, ReviewReason::LowConfidence) => item
                .members
                .first()
                .map(|m| ReviewAction::Demote { id: m.id.clone() }),
            _ => None,
        }
    }
}

fn default_snooze_days() -> u32 {
    REVIEW_DEFAULT_SNOOZE_DAYS
}

/// A per-item resolution (REV-2). Serde-tagged on `action`; the safety
/// defaults keep an under-specified frame harmless (`keep` without `bump` is
/// a no-op; `snooze` without `days` uses the documented default).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ReviewAction {
    /// No-op; with `bump`, nudge every member's confidence up by
    /// [`REVIEW_KEEP_BUMP`].
    Keep {
        #[serde(default)]
        bump: bool,
    },
    /// Store a combined memory and supersede both originals (the W3.1
    /// update-as-supersede path). Pair items only.
    Merge,
    /// Soft-delete (archive) the loser — must be a member of the item.
    Archive { id: MemoryId },
    /// Lower the member's confidence by [`REVIEW_DEMOTE_STEP`] (the W2.2 knob).
    Demote { id: MemoryId },
    /// Record `reviewed_at`/`snooze_until` so the item does not reappear
    /// until the window elapses.
    Snooze {
        #[serde(default = "default_snooze_days")]
        days: u32,
    },
}

impl ReviewAction {
    /// Stable action name for oplog details and outcome rendering.
    pub fn kind_str(&self) -> &'static str {
        match self {
            ReviewAction::Keep { .. } => "keep",
            ReviewAction::Merge => "merge",
            ReviewAction::Archive { .. } => "archive",
            ReviewAction::Demote { .. } => "demote",
            ReviewAction::Snooze { .. } => "snooze",
        }
    }

    /// Fail-closed shape validation against the item's reason and member ids
    /// — shared by the daemon boundary (never trust a wire-supplied
    /// resolution) and the CLI (reject before the round trip). Merge is
    /// restricted to near-duplicate items: merging a contradiction would
    /// collapse both sides of a dispute into one memory with no marker of
    /// the conflict.
    pub fn validate(&self, reason: ReviewReason, member_ids: &[MemoryId]) -> Result<()> {
        if member_ids.is_empty() || member_ids.len() > 2 {
            return Err(Error::InvalidArgument(format!(
                "a review item has 1 or 2 members, got {}",
                member_ids.len()
            )));
        }
        match self {
            ReviewAction::Keep { .. } => Ok(()),
            ReviewAction::Merge => {
                if reason != ReviewReason::NearDuplicate {
                    return Err(Error::InvalidArgument(
                        "merge applies to near-duplicate items only; resolve a \
                         contradiction with keep, archive, demote, or snooze"
                            .to_string(),
                    ));
                }
                if member_ids.len() != 2 || member_ids[0] == member_ids[1] {
                    return Err(Error::InvalidArgument(
                        "merge needs exactly two distinct members".to_string(),
                    ));
                }
                Ok(())
            }
            ReviewAction::Archive { id } | ReviewAction::Demote { id } => {
                if !member_ids.contains(id) {
                    return Err(Error::InvalidArgument(format!(
                        "{} target {id} is not a member of this review item",
                        self.kind_str()
                    )));
                }
                Ok(())
            }
            ReviewAction::Snooze { days } => {
                if *days == 0 || *days > REVIEW_MAX_SNOOZE_DAYS {
                    return Err(Error::InvalidArgument(format!(
                        "snooze days {days} is out of range 1..={REVIEW_MAX_SNOOZE_DAYS}"
                    )));
                }
                Ok(())
            }
        }
    }
}

/// Full-namespace category counts for the queue (the review-queue trend).
/// `near_duplicates` counts pairs FOUND within the bounded scan (a full
/// pairwise count would be O(n) KNN probes); the other tiers are exact.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReviewTotals {
    #[serde(default)]
    pub contradictions: u64,
    #[serde(default)]
    pub near_duplicates: u64,
    #[serde(default)]
    pub low_confidence: u64,
    #[serde(default)]
    pub stale_never_recalled: u64,
    /// Items currently hidden by an unexpired snooze.
    #[serde(default)]
    pub snoozed: u64,
}

/// What a policy would do to one queue item (dry-run) — and exactly what the
/// apply pass executes (shared-plan determinism, REV-3).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct PlannedResolution {
    #[serde(default)]
    pub key: String,
    pub action: ReviewAction,
}

/// The review queue / dry-run plan (REV-1): priority-ordered items plus, when
/// a policy was named, the per-item actions one apply pass would take.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
pub struct ReviewPlan {
    #[serde(default)]
    pub items: Vec<ReviewItem>,
    #[serde(default)]
    pub totals: ReviewTotals,
    #[serde(default)]
    pub policy: Option<ReviewPolicy>,
    /// Empty without a policy. Always derived via
    /// [`ReviewPolicy::plan_action`] over `items`.
    #[serde(default)]
    pub planned: Vec<PlannedResolution>,
    /// True when the item bound truncated the queue (re-run hint).
    #[serde(default)]
    pub truncated: bool,
}

/// What one policy apply pass did (the `ForgetOutcome` shape: completed
/// per-item work is never disowned by a mid-batch failure).
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
pub struct ReviewOutcome {
    #[serde(default)]
    pub policy: Option<ReviewPolicy>,
    #[serde(default)]
    pub merged: u64,
    #[serde(default)]
    pub archived: u64,
    #[serde(default)]
    pub demoted: u64,
    #[serde(default)]
    pub kept: u64,
    #[serde(default)]
    pub snoozed: u64,
    /// Queue items the policy did not cover.
    #[serde(default)]
    pub skipped: u64,
    /// Queue size when the pass started.
    #[serde(default)]
    pub total_items: u64,
    /// Present when the pass stopped early: the error that aborted it AFTER
    /// the counts above had already durably committed (each item's mutations
    /// are their own transactions — a later failure never rolls back
    /// completed work). The pass is re-runnable once the cause is fixed.
    #[serde(default)]
    pub failure: Option<String>,
}

/// Confidence of one member after a keep-bump/demote resolution.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
pub struct MemberConfidence {
    #[serde(default)]
    pub id: MemoryId,
    #[serde(default)]
    pub confidence: f32,
}

/// The daemon's reply to one per-item resolution (REV-2 interactive mode).
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
pub struct ReviewResolution {
    #[serde(default)]
    pub key: String,
    /// [`ReviewAction::kind_str`] of the applied action.
    #[serde(default)]
    pub action: String,
    /// The combined memory's id (merge only).
    #[serde(default)]
    pub merged_into: Option<MemoryId>,
    /// Post-nudge confidences (keep-bump / demote only).
    #[serde(default)]
    pub confidence: Vec<MemberConfidence>,
    /// Unix seconds the snooze expires (snooze only).
    #[serde(default)]
    pub snoozed_until: Option<i64>,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use crate::MemoryId;

    fn member(id: &MemoryId) -> ReviewMember {
        ReviewMember {
            id: id.clone(),
            summary: "a summary".to_string(),
            importance: 5,
            confidence: 0.9,
            created_at: 1_700_000_000,
            age_days: 10,
            last_accessed_at: None,
            origin_source: Some("cli".to_string()),
            origin_user: None,
        }
    }

    fn pair_item(reason: ReviewReason, a: &MemoryId, b: &MemoryId) -> ReviewItem {
        ReviewItem {
            key: review_item_key(reason, &[a.clone(), b.clone()]),
            reason,
            members: vec![member(a), member(b)],
            similarity: Some(0.97),
        }
    }

    // --- canonical item keys -------------------------------------------------

    #[test]
    fn item_key_is_order_independent_and_reason_prefixed() {
        let a = MemoryId::new();
        let b = MemoryId::new();
        let forward = review_item_key(ReviewReason::Contradiction, &[a.clone(), b.clone()]);
        let reverse = review_item_key(ReviewReason::Contradiction, &[b.clone(), a.clone()]);
        assert_eq!(
            forward, reverse,
            "the snooze key must not depend on which side was the scan anchor"
        );
        assert!(
            forward.starts_with("contradiction:"),
            "key carries the reason so a snoozed dup pair does not hide a \
             later contradiction over the same ids: {forward}"
        );
        assert!(forward.contains(&a.to_string()) && forward.contains(&b.to_string()));
    }

    #[test]
    fn item_keys_differ_across_reasons_for_the_same_ids() {
        let a = MemoryId::new();
        let b = MemoryId::new();
        let contra = review_item_key(ReviewReason::Contradiction, &[a.clone(), b.clone()]);
        let dup = review_item_key(ReviewReason::NearDuplicate, &[a, b]);
        assert_ne!(contra, dup);
    }

    #[test]
    fn single_member_key_is_reason_plus_id() {
        let a = MemoryId::new();
        let key = review_item_key(ReviewReason::LowConfidence, std::slice::from_ref(&a));
        assert_eq!(key, format!("low_confidence:{a}"));
    }

    // --- reason serde --------------------------------------------------------

    #[test]
    fn reason_serde_is_snake_case_and_round_trips() {
        for (reason, s) in [
            (ReviewReason::Contradiction, "\"contradiction\""),
            (ReviewReason::NearDuplicate, "\"near_duplicate\""),
            (ReviewReason::LowConfidence, "\"low_confidence\""),
            (ReviewReason::StaleNeverRecalled, "\"stale_never_recalled\""),
        ] {
            assert_eq!(serde_json::to_string(&reason).unwrap(), s);
            let back: ReviewReason = serde_json::from_str(s).unwrap();
            assert_eq!(back, reason);
        }
    }

    // --- policy parsing / planning -------------------------------------------

    #[test]
    fn policy_parses_documented_names_in_kebab_and_snake_case() {
        for s in ["auto-merge-dups", "auto_merge_dups"] {
            assert_eq!(
                ReviewPolicy::parse(s).unwrap(),
                ReviewPolicy::AutoMergeDups,
                "{s}"
            );
        }
        for s in ["demote-low-confidence", "demote_low_confidence"] {
            assert_eq!(
                ReviewPolicy::parse(s).unwrap(),
                ReviewPolicy::DemoteLowConfidence,
                "{s}"
            );
        }
    }

    #[test]
    fn policy_parse_rejects_garbage_with_the_documented_names() {
        let err = ReviewPolicy::parse("delete-everything").unwrap_err();
        assert!(matches!(err, crate::Error::InvalidArgument(_)));
        let msg = err.to_string();
        assert!(
            msg.contains("auto-merge-dups") && msg.contains("demote-low-confidence"),
            "the error must list the documented policies: {msg}"
        );
    }

    #[test]
    fn policy_as_str_round_trips_through_parse() {
        for p in [
            ReviewPolicy::AutoMergeDups,
            ReviewPolicy::DemoteLowConfidence,
        ] {
            assert_eq!(ReviewPolicy::parse(p.as_str()).unwrap(), p);
        }
    }

    #[test]
    fn auto_merge_dups_plans_merge_only_for_near_dup_pairs() {
        let a = MemoryId::new();
        let b = MemoryId::new();
        let dup = pair_item(ReviewReason::NearDuplicate, &a, &b);
        assert_eq!(
            ReviewPolicy::AutoMergeDups.plan_action(&dup),
            Some(ReviewAction::Merge)
        );
        // A contradiction is a human decision, never auto-merged.
        let contra = pair_item(ReviewReason::Contradiction, &a, &b);
        assert_eq!(ReviewPolicy::AutoMergeDups.plan_action(&contra), None);
        // Single-member items cannot merge.
        let low = ReviewItem {
            key: review_item_key(ReviewReason::LowConfidence, std::slice::from_ref(&a)),
            reason: ReviewReason::LowConfidence,
            members: vec![member(&a)],
            similarity: None,
        };
        assert_eq!(ReviewPolicy::AutoMergeDups.plan_action(&low), None);
    }

    #[test]
    fn demote_low_confidence_plans_demote_only_for_low_confidence_items() {
        let a = MemoryId::new();
        let b = MemoryId::new();
        let low = ReviewItem {
            key: review_item_key(ReviewReason::LowConfidence, std::slice::from_ref(&a)),
            reason: ReviewReason::LowConfidence,
            members: vec![member(&a)],
            similarity: None,
        };
        assert_eq!(
            ReviewPolicy::DemoteLowConfidence.plan_action(&low),
            Some(ReviewAction::Demote { id: a.clone() })
        );
        // Stale-never-recalled has UNKNOWN confidence (possibly high): the
        // low-confidence policy must not touch it.
        let stale = ReviewItem {
            key: review_item_key(ReviewReason::StaleNeverRecalled, std::slice::from_ref(&a)),
            reason: ReviewReason::StaleNeverRecalled,
            members: vec![member(&a)],
            similarity: None,
        };
        assert_eq!(ReviewPolicy::DemoteLowConfidence.plan_action(&stale), None);
        let dup = pair_item(ReviewReason::NearDuplicate, &a, &b);
        assert_eq!(ReviewPolicy::DemoteLowConfidence.plan_action(&dup), None);
    }

    // --- action validation ----------------------------------------------------

    #[test]
    fn action_validate_accepts_well_formed_actions() {
        let a = MemoryId::new();
        let b = MemoryId::new();
        let pair = [a.clone(), b.clone()];
        for reason in [ReviewReason::Contradiction, ReviewReason::NearDuplicate] {
            ReviewAction::Keep { bump: true }
                .validate(reason, &pair)
                .unwrap();
            ReviewAction::Archive { id: b.clone() }
                .validate(reason, &pair)
                .unwrap();
            ReviewAction::Demote { id: a.clone() }
                .validate(reason, &pair)
                .unwrap();
            ReviewAction::Snooze {
                days: REVIEW_DEFAULT_SNOOZE_DAYS,
            }
            .validate(reason, &pair)
            .unwrap();
        }
        ReviewAction::Merge
            .validate(ReviewReason::NearDuplicate, &pair)
            .unwrap();
        ReviewAction::Keep { bump: false }
            .validate(ReviewReason::LowConfidence, std::slice::from_ref(&a))
            .unwrap();
    }

    #[test]
    fn merge_requires_exactly_two_distinct_members() {
        let a = MemoryId::new();
        let err = ReviewAction::Merge
            .validate(ReviewReason::NearDuplicate, std::slice::from_ref(&a))
            .unwrap_err();
        assert!(matches!(err, crate::Error::InvalidArgument(_)), "{err}");
        let err = ReviewAction::Merge
            .validate(ReviewReason::NearDuplicate, &[a.clone(), a.clone()])
            .unwrap_err();
        assert!(matches!(err, crate::Error::InvalidArgument(_)), "{err}");
    }

    #[test]
    fn merge_is_restricted_to_near_duplicate_items() {
        // Review finding: merging a CONTRADICTION collapses two sides of a
        // dispute into one memory with no marker of the conflict — the
        // resolution for contradictions is keep/archive/demote/snooze. The
        // restriction lives in validate (the daemon boundary) so no wire
        // caller can bypass it.
        let a = MemoryId::new();
        let b = MemoryId::new();
        let pair = [a, b];
        for reason in [
            ReviewReason::Contradiction,
            ReviewReason::LowConfidence,
            ReviewReason::StaleNeverRecalled,
        ] {
            let err = ReviewAction::Merge.validate(reason, &pair).unwrap_err();
            assert!(matches!(err, crate::Error::InvalidArgument(_)), "{err}");
            let msg = err.to_string();
            assert!(
                msg.contains("near-duplicate"),
                "the error names the restriction: {msg}"
            );
            if reason == ReviewReason::Contradiction {
                assert!(
                    msg.contains("keep") && msg.contains("archive"),
                    "contradictions get redirected to the right actions: {msg}"
                );
            }
        }
    }

    #[test]
    fn archive_and_demote_targets_must_be_members() {
        let a = MemoryId::new();
        let b = MemoryId::new();
        let outsider = MemoryId::new();
        for action in [
            ReviewAction::Archive {
                id: outsider.clone(),
            },
            ReviewAction::Demote {
                id: outsider.clone(),
            },
        ] {
            let err = action
                .validate(ReviewReason::Contradiction, &[a.clone(), b.clone()])
                .unwrap_err();
            assert!(matches!(err, crate::Error::InvalidArgument(_)), "{err}");
        }
    }

    #[test]
    fn snooze_days_are_bounded() {
        let a = MemoryId::new();
        for bad in [0u32, REVIEW_MAX_SNOOZE_DAYS + 1] {
            let err = ReviewAction::Snooze { days: bad }
                .validate(ReviewReason::LowConfidence, std::slice::from_ref(&a))
                .unwrap_err();
            assert!(
                matches!(err, crate::Error::InvalidArgument(_)),
                "{bad}: {err}"
            );
        }
    }

    #[test]
    fn empty_or_oversized_member_lists_fail_closed() {
        let ids: Vec<MemoryId> = (0..3).map(|_| MemoryId::new()).collect();
        let err = ReviewAction::Keep { bump: false }
            .validate(ReviewReason::Contradiction, &[])
            .unwrap_err();
        assert!(matches!(err, crate::Error::InvalidArgument(_)), "{err}");
        let err = ReviewAction::Keep { bump: false }
            .validate(ReviewReason::Contradiction, &ids)
            .unwrap_err();
        assert!(matches!(err, crate::Error::InvalidArgument(_)), "{err}");
    }

    // --- wire shapes ----------------------------------------------------------

    #[test]
    fn action_serde_is_tagged_snake_case() {
        let id = MemoryId::new();
        let json = serde_json::to_string(&ReviewAction::Archive { id: id.clone() }).unwrap();
        assert!(json.contains("\"action\":\"archive\""), "{json}");
        let back: ReviewAction = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ReviewAction::Archive { id });
        // An absent `bump` decodes to false (never a surprise write).
        let keep: ReviewAction = serde_json::from_str(r#"{"action":"keep"}"#).unwrap();
        assert_eq!(keep, ReviewAction::Keep { bump: false });
        // An absent snooze window decodes to the documented default.
        let snooze: ReviewAction = serde_json::from_str(r#"{"action":"snooze"}"#).unwrap();
        assert_eq!(
            snooze,
            ReviewAction::Snooze {
                days: REVIEW_DEFAULT_SNOOZE_DAYS
            }
        );
    }

    #[test]
    fn plan_outcome_resolution_round_trip_and_decode_from_empty_objects() {
        let a = MemoryId::new();
        let b = MemoryId::new();
        let plan = ReviewPlan {
            items: vec![pair_item(ReviewReason::NearDuplicate, &a, &b)],
            totals: ReviewTotals {
                contradictions: 1,
                near_duplicates: 2,
                low_confidence: 3,
                stale_never_recalled: 4,
                snoozed: 5,
            },
            policy: Some(ReviewPolicy::AutoMergeDups),
            planned: vec![PlannedResolution {
                key: review_item_key(ReviewReason::NearDuplicate, &[a.clone(), b.clone()]),
                action: ReviewAction::Merge,
            }],
            truncated: true,
        };
        let back: ReviewPlan =
            serde_json::from_str(&serde_json::to_string(&plan).unwrap()).unwrap();
        assert_eq!(back, plan);

        let outcome = ReviewOutcome {
            policy: Some(ReviewPolicy::DemoteLowConfidence),
            merged: 1,
            archived: 2,
            demoted: 3,
            kept: 4,
            snoozed: 5,
            skipped: 6,
            total_items: 21,
            failure: Some("injected".to_string()),
        };
        let back: ReviewOutcome =
            serde_json::from_str(&serde_json::to_string(&outcome).unwrap()).unwrap();
        assert_eq!(back, outcome);

        let resolution = ReviewResolution {
            key: "near_duplicate:a:b".to_string(),
            action: "merge".to_string(),
            merged_into: Some(MemoryId::new()),
            confidence: vec![MemberConfidence {
                id: a.clone(),
                confidence: 0.75,
            }],
            snoozed_until: Some(1_800_000_000),
        };
        let back: ReviewResolution =
            serde_json::from_str(&serde_json::to_string(&resolution).unwrap()).unwrap();
        assert_eq!(back, resolution);

        // Additive wire shapes: every field serde(default) so a peer that
        // omits fields still decodes (the ForgetPlan precedent).
        let plan: ReviewPlan = serde_json::from_str("{}").unwrap();
        assert!(plan.items.is_empty());
        assert!(plan.policy.is_none());
        assert!(!plan.truncated);
        let outcome: ReviewOutcome = serde_json::from_str("{}").unwrap();
        assert_eq!(outcome.failure, None);
        assert_eq!(outcome.merged, 0);
        let resolution: ReviewResolution = serde_json::from_str("{}").unwrap();
        assert!(resolution.merged_into.is_none());
        let totals: ReviewTotals = serde_json::from_str("{}").unwrap();
        assert_eq!(totals.snoozed, 0);
        let member: ReviewMember = serde_json::from_str("{}").unwrap();
        assert_eq!(member.confidence, 0.0);
        let item: ReviewItem = serde_json::from_str("{}").unwrap();
        assert!(item.members.is_empty());
        assert_eq!(item.reason, ReviewReason::Contradiction);
    }

    #[test]
    // Deliberate constant pins: the test exists to make changing a documented
    // PRD value a conscious, reviewed act (the `contract_version_is_two`
    // precedent).
    #[allow(clippy::assertions_on_constants)]
    fn documented_constants_hold_their_prd_values() {
        // PRD REV-1: the low-confidence bound is < 0.4.
        assert!((REVIEW_LOW_CONFIDENCE_BOUND - 0.4).abs() < f32::EPSILON);
        // The near-dup default matches the consolidation job's documented 0.95.
        assert!((REVIEW_DEFAULT_THRESHOLD - 0.95).abs() < f32::EPSILON);
        // A caller-supplied threshold can only be MORE conservative than the
        // floor, never loose enough to call everything a dup.
        assert!(REVIEW_MIN_THRESHOLD >= 0.8);
        // The bounded knobs are sane.
        assert!(REVIEW_KEEP_BUMP > 0.0 && REVIEW_DEMOTE_STEP > 0.0);
        assert!(REVIEW_DEFAULT_SNOOZE_DAYS >= 1);
        assert!(
            REVIEW_DEFAULT_LIMIT >= 1
                && u64::from(REVIEW_DEFAULT_LIMIT) <= u64::from(REVIEW_MAX_LIMIT)
        );
    }
}
