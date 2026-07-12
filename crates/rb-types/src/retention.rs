//! User-facing retention/forgetting policy vocabulary (retention PRD).
//!
//! Lives in `rb-types` (the leaf crate) so `rb-config` (policy resolution),
//! `rb-store` (the candidate query and sweep), `rb-proto` (the wire
//! `Request::Forget`) and the CLI (rendering) share one definition — the
//! `MemoryStats` precedent. Every wire field is `#[serde(default)]` so the
//! shapes stay additive (no CONTRACT_VERSION bump).
//!
//! Safety posture (PRD "Risks"): forgetting is OFF by default, archive (not
//! hard delete) is the default action, and eligibility is guarded by the
//! importance floor, the author-intent prior (W1.9), protected tags, and the
//! contested exclusion. [`RetentionPolicy::validate`] fails closed on any
//! out-of-range or incoherent policy — a typo must never broaden scope or
//! silently no-op.

use crate::error::{Error, Result};
use crate::memory_id::MemoryId;
use crate::validate::validate_importance;
use serde::{Deserialize, Serialize};

/// PRD RET-1 default: never forget memories at/above importance 6.
pub const DEFAULT_IMPORTANCE_FLOOR: u8 = 6;
/// Default per-pass bound (the `link_decay` batch precedent): a sweep touches
/// at most this many memories and is re-runnable.
pub const DEFAULT_BATCH_LIMIT: u32 = 500;
/// Hard ceiling for the per-pass bound so a config typo cannot turn one pass
/// into an unbounded sweep.
pub const MAX_BATCH_LIMIT: u32 = 10_000;

/// Declarative retention policy (`[retention]` in the user config, RET-1).
///
/// Resolved by `rb-config` (fail closed) and carried verbatim on the wire in
/// `Request::Forget`, so the daemon evaluates exactly the policy the client
/// resolved — dry-run and apply cannot diverge on parameters.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct RetentionPolicy {
    /// Master opt-in. `false` (the default) means no sweep ever mutates:
    /// apply/hard are refused and the scheduled job never runs. Dry-run
    /// previews are still allowed (read-only).
    #[serde(default)]
    pub enabled: bool,
    /// Forget horizon: memories older than this many days are forget-eligible
    /// (archive on apply, purge on hard) — subject to every guard below.
    /// `None` means nothing is ever forget-eligible (hard mode is inert).
    #[serde(default)]
    pub max_age_days: Option<u32>,
    /// Archive horizon (soft stage before forget): ACTIVE memories older than
    /// this many days are archive-eligible on apply and in the scheduled job.
    /// Must not exceed `max_age_days` when both are set.
    #[serde(default)]
    pub archive_after_days: Option<u32>,
    /// Never forget memories whose effective OR author-set importance is at or
    /// above this floor (inclusive). Default 6.
    #[serde(default = "default_importance_floor")]
    pub importance_floor: u8,
    /// Memories carrying ANY of these tags are never forgotten.
    #[serde(default)]
    pub protected_tags: Vec<String>,
    /// Per-pass bound: one sweep touches at most this many memories.
    #[serde(default = "default_batch_limit")]
    pub batch_limit: u32,
}

fn default_importance_floor() -> u8 {
    DEFAULT_IMPORTANCE_FLOOR
}

fn default_batch_limit() -> u32 {
    DEFAULT_BATCH_LIMIT
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            max_age_days: None,
            archive_after_days: None,
            importance_floor: DEFAULT_IMPORTANCE_FLOOR,
            protected_tags: Vec::new(),
            batch_limit: DEFAULT_BATCH_LIMIT,
        }
    }
}

impl RetentionPolicy {
    /// Fail-closed policy validation — the single source of truth, shared by
    /// the config resolver (reject a bad `[retention]` block at load) and the
    /// daemon boundary (never trust a wire-supplied policy).
    pub fn validate(&self) -> Result<()> {
        validate_importance(self.importance_floor).map_err(|_| {
            Error::InvalidArgument(format!(
                "retention.importance_floor {} is out of range 1..=10",
                self.importance_floor
            ))
        })?;
        if self.max_age_days == Some(0) {
            return Err(Error::InvalidArgument(
                "retention.max_age_days must be at least 1".to_string(),
            ));
        }
        if self.archive_after_days == Some(0) {
            return Err(Error::InvalidArgument(
                "retention.archive_after_days must be at least 1".to_string(),
            ));
        }
        if let (Some(archive), Some(max)) = (self.archive_after_days, self.max_age_days) {
            if archive > max {
                return Err(Error::InvalidArgument(format!(
                    "retention.archive_after_days ({archive}) must not exceed \
                     retention.max_age_days ({max}): archive is the stage before forget"
                )));
            }
        }
        if self.enabled && self.max_age_days.is_none() && self.archive_after_days.is_none() {
            return Err(Error::InvalidArgument(
                "retention.enabled = true requires at least one rule: set \
                 retention.max_age_days and/or retention.archive_after_days"
                    .to_string(),
            ));
        }
        if self.protected_tags.iter().any(|t| t.trim().is_empty()) {
            return Err(Error::InvalidArgument(
                "retention.protected_tags must not contain blank entries".to_string(),
            ));
        }
        if self.batch_limit == 0 || self.batch_limit > MAX_BATCH_LIMIT {
            return Err(Error::InvalidArgument(format!(
                "retention.batch_limit {} is out of range 1..={MAX_BATCH_LIMIT}",
                self.batch_limit
            )));
        }
        Ok(())
    }
}

/// What a forget pass does to eligible memories (RET-2).
///
/// `Apply` archives (soft delete, reversible) — the default and the only mode
/// the scheduled job uses. `Hard` purges (irreversible, admin-gated) and is
/// never automatic. Defaults to `Apply` so an absent wire field can never
/// escalate to a purge.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ForgetMode {
    #[default]
    Apply,
    Hard,
}

impl ForgetMode {
    /// Stable string form for logs and oplog details.
    pub fn as_str(&self) -> &'static str {
        match self {
            ForgetMode::Apply => "apply",
            ForgetMode::Hard => "hard",
        }
    }
}

/// Which policy rule made a candidate eligible (the dry-run "reason").
#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ForgetRule {
    /// Older than `archive_after_days` (archive-eligible only).
    #[default]
    ArchiveAge,
    /// Older than `max_age_days` (forget-eligible: purge-capable on hard).
    MaxAge,
}

/// One forget candidate with the fields the dry-run listing renders as
/// reasons (age, importance, last-recalled, archived state, matched rule).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct ForgetCandidate {
    #[serde(default)]
    pub id: MemoryId,
    /// Summary line so the user can recognize what would be forgotten.
    #[serde(default)]
    pub summary: String,
    /// Unix seconds.
    #[serde(default)]
    pub created_at: i64,
    /// Whole days since creation at plan time (display convenience).
    #[serde(default)]
    pub age_days: u32,
    /// Effective importance (what ranking reads; W1.9 recalibrated).
    #[serde(default)]
    pub importance: u8,
    /// Author-set prior (`COALESCE(base_importance, importance)`).
    #[serde(default)]
    pub base_importance: u8,
    /// Unix seconds of the last recall access, if any.
    #[serde(default)]
    pub last_accessed_at: Option<i64>,
    /// Already archived (only hard mode lists these; purge is the only action
    /// left for them).
    #[serde(default)]
    pub archived: bool,
    #[serde(default)]
    pub rule: ForgetRule,
}

/// The dry-run preview: exactly the set a sweep in `mode` would touch,
/// computed by the same candidate query the sweep executes (RET-2).
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
pub struct ForgetPlan {
    #[serde(default)]
    pub mode: ForgetMode,
    /// Bounded by `batch_limit` — the exact set one pass would touch.
    #[serde(default)]
    pub candidates: Vec<ForgetCandidate>,
    /// Total eligible right now (may exceed `candidates.len()`; the excess is
    /// what re-running would pick up).
    #[serde(default)]
    pub total_eligible: u64,
}

/// What a sweep actually did (RET-2/RET-3).
#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ForgetOutcome {
    #[serde(default)]
    pub mode: ForgetMode,
    /// Memories soft-archived by this pass.
    #[serde(default)]
    pub archived: u64,
    /// Memories hard-purged by this pass (always 0 for apply).
    #[serde(default)]
    pub purged: u64,
    /// Total eligible when the sweep started.
    #[serde(default)]
    pub total_eligible: u64,
    /// Eligible memories left untouched by the per-pass bound (re-run hint).
    #[serde(default)]
    pub remaining: u64,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::{Error, MemoryId};

    #[test]
    fn policy_default_is_disabled_with_documented_values() {
        let p = RetentionPolicy::default();
        assert!(!p.enabled, "forgetting must be opt-in (off by default)");
        assert_eq!(p.max_age_days, None);
        assert_eq!(p.archive_after_days, None);
        assert_eq!(p.importance_floor, DEFAULT_IMPORTANCE_FLOOR);
        assert_eq!(p.importance_floor, 6, "PRD RET-1: floor defaults to 6");
        assert!(p.protected_tags.is_empty());
        assert_eq!(p.batch_limit, DEFAULT_BATCH_LIMIT);
    }

    fn sane_enabled_policy() -> RetentionPolicy {
        RetentionPolicy {
            enabled: true,
            max_age_days: Some(365),
            archive_after_days: Some(90),
            importance_floor: 6,
            protected_tags: vec!["architecture_decision".to_string()],
            batch_limit: 500,
        }
    }

    #[test]
    fn validate_accepts_a_sane_enabled_policy() {
        sane_enabled_policy().validate().unwrap();
    }

    #[test]
    fn validate_accepts_the_disabled_default() {
        // No [retention] rules at all is a valid (inert) policy.
        RetentionPolicy::default().validate().unwrap();
    }

    #[test]
    fn validate_rejects_floor_out_of_range() {
        for bad in [0u8, 11, 255] {
            let mut p = sane_enabled_policy();
            p.importance_floor = bad;
            let err = p.validate().unwrap_err();
            assert!(matches!(err, Error::InvalidArgument(_)), "{bad}: {err:?}");
            assert!(
                err.to_string().contains("importance_floor"),
                "message must name the key: {err}"
            );
        }
    }

    #[test]
    fn validate_rejects_zero_ages() {
        let mut p = sane_enabled_policy();
        p.max_age_days = Some(0);
        let err = p.validate().unwrap_err();
        assert!(err.to_string().contains("max_age_days"), "{err}");

        let mut p = sane_enabled_policy();
        p.archive_after_days = Some(0);
        let err = p.validate().unwrap_err();
        assert!(err.to_string().contains("archive_after_days"), "{err}");
    }

    #[test]
    fn validate_rejects_archive_after_exceeding_max_age() {
        // Archive is the stage BEFORE forget: an archive horizon past the
        // forget horizon is an incoherent policy, not a silent reorder.
        let mut p = sane_enabled_policy();
        p.archive_after_days = Some(400);
        p.max_age_days = Some(365);
        let err = p.validate().unwrap_err();
        assert!(matches!(err, Error::InvalidArgument(_)));
        assert!(
            err.to_string().contains("archive_after_days")
                && err.to_string().contains("max_age_days"),
            "message must name both keys: {err}"
        );
    }

    #[test]
    fn validate_rejects_enabled_without_any_age_rule() {
        // enabled=true with no rule would either no-op silently (data-loss
        // footgun in reverse) or sweep everything below the floor regardless
        // of age (the real footgun). Fail closed instead.
        let p = RetentionPolicy {
            enabled: true,
            ..RetentionPolicy::default()
        };
        let err = p.validate().unwrap_err();
        assert!(matches!(err, Error::InvalidArgument(_)));
        assert!(
            err.to_string().contains("max_age_days")
                || err.to_string().contains("archive_after_days"),
            "message must point at the missing rule: {err}"
        );
    }

    #[test]
    fn validate_rejects_blank_protected_tag() {
        let mut p = sane_enabled_policy();
        p.protected_tags.push("   ".to_string());
        let err = p.validate().unwrap_err();
        assert!(matches!(err, Error::InvalidArgument(_)));
        assert!(err.to_string().contains("protected_tags"), "{err}");
    }

    #[test]
    fn validate_rejects_batch_limit_out_of_bounds() {
        for bad in [0u32, MAX_BATCH_LIMIT + 1] {
            let mut p = sane_enabled_policy();
            p.batch_limit = bad;
            let err = p.validate().unwrap_err();
            assert!(matches!(err, Error::InvalidArgument(_)), "{bad}: {err:?}");
            assert!(err.to_string().contains("batch_limit"), "{err}");
        }
    }

    #[test]
    fn forget_mode_serde_is_snake_case_and_defaults_to_apply() {
        assert_eq!(
            serde_json::to_string(&ForgetMode::Apply).unwrap(),
            r#""apply""#
        );
        assert_eq!(
            serde_json::to_string(&ForgetMode::Hard).unwrap(),
            r#""hard""#
        );
        // The safe direction: an absent mode must never decode to Hard.
        assert_eq!(ForgetMode::default(), ForgetMode::Apply);
    }

    #[test]
    fn wire_types_round_trip_with_every_field() {
        let plan = ForgetPlan {
            mode: ForgetMode::Hard,
            candidates: vec![ForgetCandidate {
                id: MemoryId::new(),
                summary: "an old low-importance note".to_string(),
                created_at: 1_700_000_000,
                age_days: 400,
                importance: 3,
                base_importance: 4,
                last_accessed_at: Some(1_710_000_000),
                archived: true,
                rule: ForgetRule::MaxAge,
            }],
            total_eligible: 7,
        };
        let json = serde_json::to_string(&plan).unwrap();
        let back: ForgetPlan = serde_json::from_str(&json).unwrap();
        assert_eq!(back, plan);

        let outcome = ForgetOutcome {
            mode: ForgetMode::Apply,
            archived: 5,
            purged: 0,
            total_eligible: 9,
            remaining: 4,
        };
        let json = serde_json::to_string(&outcome).unwrap();
        let back: ForgetOutcome = serde_json::from_str(&json).unwrap();
        assert_eq!(back, outcome);

        let policy = sane_enabled_policy();
        let json = serde_json::to_string(&policy).unwrap();
        let back: RetentionPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(back, policy);
    }

    #[test]
    fn plan_and_outcome_decode_from_an_empty_object() {
        // Additive wire shapes: every field serde(default) so a peer that
        // omits fields still decodes (the MemoryStats precedent).
        let plan: ForgetPlan = serde_json::from_str("{}").unwrap();
        assert_eq!(plan.mode, ForgetMode::Apply);
        assert!(plan.candidates.is_empty());
        assert_eq!(plan.total_eligible, 0);

        let outcome: ForgetOutcome = serde_json::from_str("{}").unwrap();
        assert_eq!(outcome.mode, ForgetMode::Apply);
        assert_eq!(outcome.archived, 0);
        assert_eq!(outcome.purged, 0);

        let policy: RetentionPolicy = serde_json::from_str("{}").unwrap();
        assert_eq!(policy, RetentionPolicy::default());
        assert!(!policy.enabled);
    }

    #[test]
    fn forget_rule_serde_is_snake_case() {
        assert_eq!(
            serde_json::to_string(&ForgetRule::ArchiveAge).unwrap(),
            r#""archive_age""#
        );
        assert_eq!(
            serde_json::to_string(&ForgetRule::MaxAge).unwrap(),
            r#""max_age""#
        );
    }
}
