//! Importance recalibration job: modulate EFFECTIVE `importance` around the
//! immutable author-set prior (`base_importance`), within a bounded ±2 delta,
//! from access_count and last_accessed_at (recency). W1.9 (F33): author intent
//! is the anchor — access signals may nudge a memory, never re-author it.
//!
//! Disabled by default (and intended to STAY disabled until the W3.7
//! usefulness signal exists): today `access_count` counts
//! returned-from-recall, which is "retrieved", not "useful". The bounded
//! formula below is implemented and ready, so enabling the job once a real
//! usefulness signal feeds it is a config flip, not a code change.

use crate::jobs::config::ImportanceConfig;
use crate::jobs::JobSummary;
use crate::StoreHandle;
use rb_types::Result;

/// Hard bound on how far recalibration may move the effective importance from
/// the author-set prior, in either direction (the W1.9 author-intent
/// guarantee: an importance-10 memory can never fall below 8 from access
/// signals alone). Deliberately NOT configurable — a tunable bound would let a
/// config typo erase author intent.
const MAX_DELTA: f64 = 2.0;

/// Recompute an effective importance from the author-set prior and the access
/// signals, bounded to the prior's ±[`MAX_DELTA`] band.
///
/// `base` is the AUTHOR-SET PRIOR (`base_importance`), never the current
/// effective importance. Because the prior is immutable (only an explicit
/// user update re-stamps it), this is a pure function of stable inputs: the
/// job is idempotent across runs by construction — recomputing over unchanged
/// access data re-derives the same target, and repeated runs can never ratchet
/// a memory out of its author band. `now` and `last_accessed_at` are unix
/// seconds.
///
/// Formula (documented contract; cf. plan W1.9 `clamp(base + k·tanh(signal) −
/// decay, base−2, base+2)` with `k = MAX_DELTA`):
///   recency = last_accessed_at.map(|t| 0.5^(((now - t).max(0)/86400)/half_life_days)).unwrap_or(0.0)
///   access  = ln(1 + access_count.max(0))
///   signal  = access_weight*access + recency_weight*recency
///   if signal <= 0 => new = clamp(base, 1, 10)        // untouched: author's prior verbatim
///   else:
///     boost = MAX_DELTA * tanh(signal)                // (0, 2): diminishing returns
///     decay = MAX_DELTA * (1 - recency)               // [0, 2]: staleness penalty
///     new   = clamp(round(base + boost - decay), max(base-2, 1), min(base+2, 10))
///
/// Properties: monotone in `access_count` at fixed recency; bounded to
/// `[base-2, base+2]` (then 1..=10) for ALL signals. Note the staleness
/// asymmetry is intentional: a memory accessed once long ago carries
/// EVIDENCE of abandonment and may land below the prior, while a
/// never-accessed memory carries no evidence and keeps the prior exactly.
pub fn recalibrate(
    base: u8,
    access_count: i64,
    last_accessed_at: Option<i64>,
    now: i64,
    cfg: &ImportanceConfig,
) -> u8 {
    // Recency in [0,1]: exponential decay by elapsed days over the half-life.
    // A future (clock-skewed) timestamp clamps to age 0 => recency 1.0.
    // None (never accessed) contributes no recency.
    let recency = match last_accessed_at {
        Some(t) => {
            let age_days = (now - t).max(0) as f64 / 86_400.0;
            0.5_f64.powf(age_days / cfg.half_life_days.max(f64::MIN_POSITIVE))
        }
        None => 0.0,
    };

    // Access contribution: ln(1 + n) gives diminishing returns and ln_1p(0) == 0.
    let access = (access_count.max(0) as f64).ln_1p();

    // Combined access signal in [0, +inf); EXACTLY 0.0 only when never accessed.
    let signal = cfg.access_weight * access + cfg.recency_weight * recency;

    // A fully-decayed recency does not underflow to exactly 0.0 in f64 (e.g.
    // 0.5^121 is a subnormal ~1e-37), so a negligible signal must still be
    // treated as untouched. The documented contract is "decayed-to-zero signal
    // matches never-accessed": any real access keeps the signal far above this
    // epsilon (the smallest touched-branch signal in practice is ~0.35,
    // one fully-decayed access at the default access_weight of 0.5).
    const SIGNAL_EPS: f64 = 1e-9;

    if signal <= SIGNAL_EPS {
        // Untouched memory: the author's prior verbatim (clamped for safety).
        return (base as f64).clamp(1.0, 10.0) as u8;
    }

    // Touched memory: nudge the author's prior, never re-author it. The boost
    // saturates (tanh) so heavy access cannot exceed +MAX_DELTA; the decay
    // penalizes staleness (recency 1.0 => none, fully decayed => -MAX_DELTA).
    let prior = base as f64;
    let boost = MAX_DELTA * signal.tanh();
    let decay = MAX_DELTA * (1.0 - recency);

    // The band clamp is the author-intent guarantee (W1.9): whatever the
    // signals do, the result stays within ±MAX_DELTA of the prior — and inside
    // validate_importance's 1..=10 range.
    let floor = (prior - MAX_DELTA).max(1.0);
    let ceil = (prior + MAX_DELTA).min(10.0);
    (prior + boost - decay).round().clamp(floor, ceil) as u8
}

/// Run one bounded, idempotent recalibration pass.
///
/// Reads up to `cfg.batch_limit` active memories via the read pool, recomputes
/// each EFFECTIVE importance with [`recalibrate`] from the row's immutable
/// `base_importance` author prior, and — only when the value actually changes —
/// writes it back through the single-writer `set_recalibrated_importance` path
/// (which never touches the prior) using the row's OWN namespace. Idempotent:
/// the target is a pure function of the prior and the access signals, so a
/// second pass over unchanged access data recomputes the same values and
/// writes nothing — and repeated runs can never drift a memory out of its
/// author band. Fail-safe: each update is its own writer transaction; a single
/// failed update aborts the pass with an error rather than leaving a
/// half-applied batch.
pub async fn run(store: &StoreHandle, cfg: &ImportanceConfig) -> Result<JobSummary> {
    let now = chrono::Utc::now().timestamp();
    let rows = store.memories_for_recalibration(cfg.batch_limit).await?;

    let mut summary = JobSummary::default();
    for row in rows {
        summary.scanned += 1;
        let new = recalibrate(
            row.base_importance,
            row.access_count,
            row.last_accessed_at,
            now,
            cfg,
        );
        if new == row.importance {
            summary.skipped += 1;
            continue;
        }
        store
            .set_recalibrated_importance(row.namespace.clone(), row.id.clone(), new)
            .await?;
        summary.changed += 1;
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn cfg() -> ImportanceConfig {
        ImportanceConfig {
            enabled: true,
            interval_secs: 86_400,
            access_weight: 0.5,
            recency_weight: 0.5,
            half_life_days: 30.0,
            batch_limit: 1000,
        }
    }

    const NOW: i64 = 1_700_000_000;
    const DAY: i64 = 86_400;

    /// Wide signal sweep shared by the property tests below: access counts
    /// (including pathological negatives) crossed with ages from "never" to
    /// "a decade stale" to "clock-skewed future".
    fn signal_sweep() -> Vec<(i64, Option<i64>)> {
        let accesses = [0i64, 1, 2, 5, 10, 100, 10_000, 1_000_000, -5];
        let lasts = [
            None,
            Some(NOW),
            Some(NOW - DAY),
            Some(NOW - 7 * DAY),
            Some(NOW - 30 * DAY),
            Some(NOW - 90 * DAY),
            Some(NOW - 365 * DAY),
            Some(NOW - 3650 * DAY),
            Some(0),
            Some(NOW + DAY),
        ];
        let mut sweep = Vec::new();
        for access in accesses {
            for last in lasts {
                sweep.push((access, last));
            }
        }
        sweep
    }

    #[test]
    fn output_is_always_a_valid_importance() {
        // Property: every output must pass the 1..=10 validator, for the full
        // base x signal sweep.
        for base in 1u8..=10 {
            for (access, last) in signal_sweep() {
                let out = recalibrate(base, access, last, NOW, &cfg());
                assert!(
                    rb_types::validate_importance(out).is_ok(),
                    "recalibrate({base},{access},{last:?}) = {out} must be 1..=10"
                );
            }
        }
    }

    #[test]
    fn recalibration_never_leaves_the_author_band() {
        // Property (W1.9 author-intent guarantee): for EVERY combination of
        // author prior and access signals, the effective importance stays
        // within ±2 of the prior.
        for base in 1u8..=10 {
            let floor = base.saturating_sub(2).max(1);
            let ceil = base.saturating_add(2).min(10);
            for (access, last) in signal_sweep() {
                let out = recalibrate(base, access, last, NOW, &cfg());
                assert!(
                    (floor..=ceil).contains(&out),
                    "recalibrate({base},{access},{last:?}) = {out} escaped the \
                     author band [{floor},{ceil}]"
                );
            }
        }
    }

    #[test]
    fn importance_ten_never_falls_below_eight_from_access_signals_alone() {
        // Property (spec W1.9, verbatim): importance-10 never falls below 8
        // from access signals alone — no combination of access count and
        // staleness may demote a memory its author marked critical below 8.
        for (access, last) in signal_sweep() {
            let out = recalibrate(10, access, last, NOW, &cfg());
            assert!(
                out >= 8,
                "recalibrate(10,{access},{last:?}) = {out} fell below 8"
            );
        }
    }

    #[test]
    fn is_a_fixed_point_over_the_immutable_prior() {
        // Idempotency: the target is a pure function of the AUTHOR PRIOR and
        // the access signals. The prior never changes (only the effective
        // value is written), so recomputing with unchanged inputs re-derives
        // the same target — the job-level no-op on a second pass.
        for (access, last) in [
            (50i64, Some(NOW)),         // touched: heavy + fresh
            (3, Some(NOW - 10 * DAY)),  // touched: light + slightly stale
            (1, Some(NOW - 365 * DAY)), // touched: stale (negative delta)
            (0, None),                  // untouched
        ] {
            for base in 1u8..=10 {
                let once = recalibrate(base, access, last, NOW, &cfg());
                let twice = recalibrate(base, access, last, NOW, &cfg());
                assert_eq!(
                    once, twice,
                    "recalibrate must be deterministic over the prior: base={base}, \
                     access={access}, last={last:?}, once={once}, twice={twice}"
                );
            }
        }
    }

    #[test]
    fn clamps_to_upper_bound_ten() {
        // Heavy access + fresh recency saturates the touched-branch target at 10.
        let out = recalibrate(10, 1_000_000, Some(NOW), NOW, &cfg());
        assert_eq!(out, 10, "must clamp at the upper bound");
    }

    #[test]
    fn clamps_to_lower_bound_one() {
        // Minimum base, never accessed: untouched branch keeps base, never below 1.
        let out = recalibrate(1, 0, None, NOW, &cfg());
        assert_eq!(out, 1, "must clamp at the lower bound");
    }

    #[test]
    fn more_access_never_lowers_importance_at_fixed_recency() {
        // Property: monotone in access_count at fixed recency (the decay term
        // depends only on recency, the boost only grows with access). Checked
        // across every prior and a spread of ages. NB: monotonicity is scoped
        // to fixed recency by design — an untouched memory keeps its prior,
        // which may sit ABOVE a stale once-touched one's target (staleness is
        // evidence of abandonment; absence of access is no evidence).
        for base in 1u8..=10 {
            for last in [
                Some(NOW),
                Some(NOW - 7 * DAY),
                Some(NOW - 90 * DAY),
                Some(NOW - 3650 * DAY),
            ] {
                let mut prev: Option<u8> = None;
                for access in [1i64, 2, 5, 10, 100, 10_000] {
                    let out = recalibrate(base, access, last, NOW, &cfg());
                    if let Some(p) = prev {
                        assert!(
                            out >= p,
                            "more access lowered importance: base={base}, last={last:?}, \
                             access={access}: {out} < {p}"
                        );
                    }
                    prev = Some(out);
                }
            }
        }
    }

    #[test]
    fn stale_and_unaccessed_falls_back_to_base() {
        // A very old last_accessed_at with zero access decays the signal to 0,
        // so it lands in the untouched branch exactly like a never-accessed
        // memory: both keep `base`.
        let stale = recalibrate(5, 0, Some(NOW - 3650 * DAY), NOW, &cfg());
        let never = recalibrate(5, 0, None, NOW, &cfg());
        assert_eq!(stale, 5, "stale + unaccessed keeps base");
        assert_eq!(
            stale, never,
            "decayed-to-zero signal must match never-accessed: stale={stale}, never={never}"
        );
    }

    #[test]
    fn none_last_accessed_is_treated_as_never_accessed() {
        // None contributes zero recency; with zero access the signal is 0 and the
        // untouched branch keeps base verbatim.
        let out = recalibrate(6, 0, None, NOW, &cfg());
        assert_eq!(out, 6, "no access and no recency leaves base unchanged");
    }

    #[test]
    fn future_last_accessed_is_clamped_to_zero_age_not_negative() {
        // (now - t).max(0) guards a clock-skewed future timestamp: recency is the
        // maximum (1.0), never a NaN/negative blow-up. The recency alone makes the
        // signal positive => touched branch, identical to an exactly-now access.
        let future = recalibrate(5, 0, Some(NOW + 10 * DAY), NOW, &cfg());
        let now_exact = recalibrate(5, 0, Some(NOW), NOW, &cfg());
        assert_eq!(
            future, now_exact,
            "future timestamp clamps to age 0, same as now: future={future}, now={now_exact}"
        );
        assert!(rb_types::validate_importance(future).is_ok());
    }

    #[test]
    fn touched_target_matches_documented_formula() {
        // Fresh single access: access=ln(2)=0.69315, recency=1.0,
        // signal = 0.5*0.69315 + 0.5*1.0 = 0.84657; boost = 2*tanh(0.84657)
        // = 2*0.68915 = 1.37830; decay = 2*(1-1.0) = 0.
        // target = round(base + 1.37830) = base + 1 (band permitting).
        let out = recalibrate(3, 1, Some(NOW), NOW, &cfg());
        assert_eq!(out, 4, "fresh access nudges the prior up by the boost");
        // The SAME signals move a different prior to a different target — the
        // prior is the anchor (contrast with the pre-W1.9 pure-access target).
        let out_high = recalibrate(9, 1, Some(NOW), NOW, &cfg());
        assert_eq!(
            out_high, 10,
            "round(9 + 1.378) = 10, capped by the band/range"
        );

        // Stale single access (90 days, half-life 30): recency = 0.5^3 = 0.125,
        // access = 0.69315, signal = 0.5*0.69315 + 0.5*0.125 = 0.40907;
        // boost = 2*tanh(0.40907) = 0.77536; decay = 2*(1-0.125) = 1.75.
        // target = round(5 + 0.77536 - 1.75) = round(4.02536) = 4.
        let stale = recalibrate(5, 1, Some(NOW - 90 * DAY), NOW, &cfg());
        assert_eq!(stale, 4, "staleness decays the prior within the band");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_recalibrates_accessed_memories_and_is_idempotent() {
        use crate::StoreHandle;
        use rb_engine::MemoryBackend;
        use rb_types::{MemoryNote, MemoryType, Namespace};

        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let handle = StoreHandle::start(db, 8, 2).unwrap();
        let ns = Namespace::Project("recal-job".to_string());

        // hot: low base importance, will be accessed many times.
        let hot = MemoryNote::new(ns.clone(), "hot memory".into(), MemoryType::Insight, 3);
        let hot_id = hot.id.clone();
        handle.write(hot, Some(vec![0.1f32; 8])).await.unwrap();

        // cold: never accessed, importance should not change (delta 0 => skipped).
        let cold = MemoryNote::new(ns.clone(), "cold memory".into(), MemoryType::Reference, 3);
        let cold_id = cold.id.clone();
        handle.write(cold, Some(vec![0.2f32; 8])).await.unwrap();

        for _ in 0..50 {
            handle.record_access(hot_id.clone()).await.unwrap();
        }
        // record_access buffers (W1.8); flush so the job's scan sees the counts.
        handle.flush_accesses().await.unwrap();

        let cfg = ImportanceConfig {
            enabled: true,
            interval_secs: 86_400,
            access_weight: 1.0,
            recency_weight: 1.0,
            half_life_days: 30.0,
            batch_limit: 1000,
        };

        // First pass: hot rises, cold unchanged.
        let summary = run(&handle, &cfg).await.unwrap();
        assert_eq!(summary.scanned, 2, "both active rows scanned");
        assert_eq!(summary.changed, 1, "only the hot memory changed");
        assert_eq!(summary.skipped, 1, "the cold memory was skipped");

        let hot_after = handle
            .get(ns.clone(), hot_id.clone())
            .await
            .unwrap()
            .expect("hot memory present");
        assert_eq!(
            hot_after.importance, 5,
            "50 fresh accesses saturate the boost: round(3 + 2*tanh(big)) = 5"
        );
        assert!(
            hot_after.importance <= 3 + 2,
            "the effective value must stay within the author band (base 3 + 2)"
        );
        assert_eq!(
            hot_after.namespace, ns,
            "update must preserve the row's own namespace"
        );
        // The author prior is untouched by the job write (W1.9): the bound
        // stays anchored at 3 forever, however many passes run.
        let hot_row = handle
            .memories_for_recalibration(10)
            .await
            .unwrap()
            .into_iter()
            .find(|r| r.id == hot_id)
            .expect("hot row present");
        assert_eq!(
            hot_row.base_importance, 3,
            "the job must never re-stamp the author prior"
        );

        let cold_after = handle
            .get(ns.clone(), cold_id.clone())
            .await
            .unwrap()
            .expect("cold memory present");
        assert_eq!(
            cold_after.importance, 3,
            "never-accessed memory keeps its base importance"
        );

        // Second pass with unchanged access data: nothing changes (idempotent).
        let again = run(&handle, &cfg).await.unwrap();
        assert_eq!(again.scanned, 2, "second pass still scans both rows");
        assert_eq!(
            again.changed, 0,
            "idempotent: re-running with unchanged access data changes nothing"
        );
        assert_eq!(
            again.skipped, 2,
            "both rows already at their recalibrated value"
        );

        handle.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_once_dispatches_importance_arm() {
        use crate::jobs::{run_once, JobKind, JobsConfig};
        use crate::StoreHandle;
        use rb_engine::MemoryBackend;
        use rb_types::{MemoryNote, MemoryType, Namespace};

        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let handle = StoreHandle::start(db, 8, 2).unwrap();
        let ns = Namespace::Global;

        let m = MemoryNote::new(ns.clone(), "dispatched".into(), MemoryType::Insight, 4);
        handle.write(m, Some(vec![0.1f32; 8])).await.unwrap();

        let config = JobsConfig::default();
        let summary = run_once(JobKind::ImportanceRecalibration, &handle, &config)
            .await
            .unwrap();
        assert_eq!(
            summary.scanned, 1,
            "run_once must route ImportanceRecalibration through importance::run"
        );

        handle.shutdown().await;
    }
}
