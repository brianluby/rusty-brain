//! Importance recalibration job: recompute `importance` from access_count and
//! last_accessed_at (recency), clamped to the validated 1..=10 range.

use crate::jobs::config::ImportanceConfig;
use crate::jobs::JobSummary;
use crate::StoreHandle;
use rb_engine::MemoryBackend;
use rb_types::{MemoryUpdates, Result};

/// Recompute an importance value from access frequency and recency.
///
/// Deterministic, monotonic in access, and a FIXED POINT (idempotent): for a
/// touched memory the result depends only on the access signals, never on the
/// current importance, so re-running over unchanged access data is a no-op.
/// `now` and `last_accessed_at` are unix seconds.
///
/// Formula (documented contract):
///   recency = last_accessed_at.map(|t| 0.5^(((now - t).max(0)/86400)/half_life_days)).unwrap_or(0.0)
///   access  = ln(1 + access_count.max(0))
///   signal  = access_weight*access + recency_weight*recency
///   if signal <= 0 => new = clamp(round(base))                  // untouched: keep author's value
///   else           => new = clamp(round(1 + 9*tanh(signal)))    // touched: pure access target
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
    // epsilon (the smallest touched-branch signal in practice is ~0.5).
    const SIGNAL_EPS: f64 = 1e-9;

    let value = if signal <= SIGNAL_EPS {
        // Untouched memory: keep the author's importance (still clamped for safety).
        base as f64
    } else {
        // Touched memory: target is a PURE function of the access signal on the
        // full 1..=10 band, independent of `base`. This is what makes the job a
        // fixed point — re-running re-derives the same target, so nothing changes.
        const FLOOR: f64 = 1.0;
        const SPAN: f64 = 9.0; // 10.0 - 1.0
        FLOOR + SPAN * signal.tanh()
    };

    // Always a valid importance (1..=10): the clamp is the single source of truth
    // that keeps the output inside validate_importance's range.
    value.round().clamp(1.0, 10.0) as u8
}

/// Run one bounded, idempotent recalibration pass.
///
/// Reads up to `cfg.batch_limit` active memories via the read pool, recomputes
/// each importance with [`recalibrate`], and — only when the value actually
/// changes — writes it back through the single-writer `update` path using the
/// row's OWN namespace. Idempotent: a second pass over unchanged access data
/// recomputes the same values and writes nothing. Fail-safe: each update is its
/// own writer transaction; a single failed update aborts the pass with an error
/// rather than leaving a half-applied batch.
pub async fn run(store: &StoreHandle, cfg: &ImportanceConfig) -> Result<JobSummary> {
    let now = chrono::Utc::now().timestamp();
    let rows = store.memories_for_recalibration(cfg.batch_limit).await?;

    let mut summary = JobSummary::default();
    for row in rows {
        summary.scanned += 1;
        let new = recalibrate(
            row.importance,
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
            .update(
                row.namespace.clone(),
                row.id.clone(),
                MemoryUpdates {
                    importance: Some(new),
                    ..Default::default()
                },
            )
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

    #[test]
    fn output_is_always_a_valid_importance() {
        // Every output must pass the 1..=10 validator, for a wide input sweep.
        for base in 1u8..=10 {
            for access in [0i64, 1, 10, 100, 10_000, -5] {
                for last in [
                    None,
                    Some(NOW),
                    Some(NOW - 365 * DAY),
                    Some(0),
                    Some(NOW + DAY),
                ] {
                    let out = recalibrate(base, access, last, NOW, &cfg());
                    assert!(
                        rb_types::validate_importance(out).is_ok(),
                        "recalibrate({base},{access},{last:?}) = {out} must be 1..=10"
                    );
                }
            }
        }
    }

    #[test]
    fn is_a_fixed_point_for_touched_and_untouched() {
        // Idempotency: feeding the output back in as `base` must yield the same
        // value, both for a touched memory (access-derived target) and an
        // untouched one (author's base preserved).
        for (access, last) in [
            (50i64, Some(NOW)),        // touched: heavy + fresh
            (3, Some(NOW - 10 * DAY)), // touched: light + slightly stale
            (0, None),                 // untouched
        ] {
            for base in 1u8..=10 {
                let once = recalibrate(base, access, last, NOW, &cfg());
                let twice = recalibrate(once, access, last, NOW, &cfg());
                assert_eq!(
                    once, twice,
                    "recalibrate must be a fixed point: base={base}, access={access}, \
                     last={last:?}, once={once}, twice={twice}"
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
    fn more_access_never_lowers_importance() {
        // Monotonic in access_count: more accesses => importance >= fewer accesses.
        let few = recalibrate(5, 1, Some(NOW), NOW, &cfg());
        let many = recalibrate(5, 10_000, Some(NOW), NOW, &cfg());
        assert!(
            many >= few,
            "more access must not lower importance: few={few}, many={many}"
        );
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
        // access_count=1, fresh: access=ln(2)=0.6931, recency=1.0,
        // signal = 0.5*0.6931 + 0.5*1.0 = 0.84657; tanh(0.84657)=0.68915;
        // target = 1 + 9*0.68915 = 7.2024 => round => 7. base is irrelevant here.
        let out = recalibrate(3, 1, Some(NOW), NOW, &cfg());
        assert_eq!(
            out, 7,
            "touched target is a pure function of access, not base"
        );
        // Same access signal with a different base yields the SAME touched target
        // (proves base does not enter the touched branch — the fixed-point property).
        let out_other_base = recalibrate(9, 1, Some(NOW), NOW, &cfg());
        assert_eq!(
            out, out_other_base,
            "touched target ignores base: {out} vs {out_other_base}"
        );
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
        assert!(
            hot_after.importance > 3,
            "accessed memory's importance must rise above base 3, got {}",
            hot_after.importance
        );
        assert_eq!(
            hot_after.namespace, ns,
            "update must preserve the row's own namespace"
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
