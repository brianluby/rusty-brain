//! Link-decay job: exponentially decay link `strength` by age, floored. Reads
//! candidate links via the read pool; writes via the single writer. Bounded by
//! `batch_limit`; idempotent (re-running on already-decayed links no-ops once
//! the value stops moving by more than `EPSILON`).

use crate::jobs::config::LinkDecayConfig;
use crate::jobs::JobSummary;
use crate::StoreHandle;

/// Minimum strength delta that counts as a change. Below this the row is left
/// untouched so the pass is idempotent and avoids pointless writes.
const EPSILON: f32 = 1e-6;

/// Pure decay function. `age_days` is the link's age; `half_life_days` the decay
/// constant. The result is `strength * 0.5^(age/half_life)`, floored at `floor`,
/// never exceeding the input. Deterministic; the unit tests pin its invariants.
pub fn decayed_strength(strength: f32, age_days: f64, half_life_days: f64, floor: f64) -> f32 {
    // A non-positive half-life is meaningless; treat it as "no decay" rather
    // than dividing by zero (fail-safe: never panics, never NaN).
    if half_life_days <= 0.0 || age_days <= 0.0 {
        return strength.max(floor as f32);
    }
    let factor = 0.5_f64.powf(age_days / half_life_days);
    let decayed = (strength as f64) * factor;
    decayed.max(floor) as f32
}

/// Run one bounded link-decay pass using `chrono::Utc::now()` as the clock.
pub async fn run(store: &StoreHandle, config: &LinkDecayConfig) -> rb_types::Result<JobSummary> {
    run_at(store, config, chrono::Utc::now()).await
}

/// Run one bounded pass with an injected `now` (deterministic in tests).
pub async fn run_at(
    store: &StoreHandle,
    config: &LinkDecayConfig,
    now: chrono::DateTime<chrono::Utc>,
) -> rb_types::Result<JobSummary> {
    let rows = store.links_for_decay(config.batch_limit).await?;
    let mut summary = JobSummary::default();

    for row in rows {
        summary.scanned += 1;

        let age_secs = (now - row.created_at).num_seconds();
        let age_days = (age_secs as f64) / 86_400.0;
        let new_strength =
            decayed_strength(row.strength, age_days, config.half_life_days, config.floor);

        let floor = config.floor as f32;
        if config.prune_below_floor && new_strength <= floor {
            store
                .delete_link(row.source.clone(), row.target.clone(), row.link_type)
                .await?;
            summary.changed += 1;
        } else if (new_strength - row.strength).abs() > EPSILON {
            store
                .set_link_strength(
                    row.source.clone(),
                    row.target.clone(),
                    row.link_type,
                    new_strength,
                )
                .await?;
            summary.changed += 1;
        } else {
            summary.skipped += 1;
        }
    }

    Ok(summary)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::jobs::config::LinkDecayConfig;
    use rb_engine::MemoryBackend;
    use rb_types::Namespace;

    const DIM: usize = 8;

    #[test]
    fn decayed_strength_is_monotonic_non_increasing_in_age() {
        let (hl, floor) = (30.0_f64, 0.05_f64);
        let mut prev = decayed_strength(0.9, 0.0, hl, floor);
        for age in [1.0, 5.0, 30.0, 60.0, 365.0] {
            let cur = decayed_strength(0.9, age, hl, floor);
            assert!(cur <= prev, "decay must not increase with age");
            prev = cur;
        }
    }

    #[test]
    fn decayed_strength_never_below_floor_and_never_above_input() {
        let (hl, floor) = (30.0_f64, 0.05_f64);
        for age in [0.0, 10.0, 100.0, 10_000.0] {
            let s = decayed_strength(0.9, age, hl, floor);
            assert!(s >= floor as f32 - f32::EPSILON, "never below floor");
            assert!(s <= 0.9 + f32::EPSILON, "never above input");
        }
    }

    #[test]
    fn decayed_strength_halves_at_one_half_life() {
        let s = decayed_strength(0.8, 30.0, 30.0, 0.0);
        assert!(
            (s - 0.4).abs() < 1e-5,
            "one half-life halves strength, got {s}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_decays_an_old_link_via_the_writer() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let store = crate::StoreHandle::start(db, DIM, 2).unwrap();
        let ns = Namespace::Project("decay-run".to_string());

        let a = rb_types::MemoryNote::new(
            ns.clone(),
            "source".to_string(),
            rb_types::MemoryType::Insight,
            5,
        );
        let b = rb_types::MemoryNote::new(
            ns.clone(),
            "target".to_string(),
            rb_types::MemoryType::Insight,
            5,
        );
        let (aid, bid) = (a.id.clone(), b.id.clone());
        store.write(a, Some(vec![0.1f32; DIM])).await.unwrap();
        store.write(b, Some(vec![0.2f32; DIM])).await.unwrap();

        // Link created 60 days ago (two half-lives at hl=30).
        let created = chrono::Utc::now() - chrono::Duration::days(60);
        store
            .add_link(rb_types::MemoryLink {
                source_id: aid.clone(),
                target_id: bid.clone(),
                link_type: rb_types::LinkType::References,
                strength: 0.8,
                reason: "seed".to_string(),
                created_at: created,
            })
            .await
            .unwrap();

        let cfg = LinkDecayConfig {
            enabled: true,
            interval_secs: 86_400,
            half_life_days: 30.0,
            floor: 0.05,
            prune_below_floor: false,
            batch_limit: 1000,
        };
        let now = chrono::Utc::now();
        let summary = run_at(&store, &cfg, now).await.unwrap();

        assert_eq!(summary.scanned, 1);
        assert_eq!(summary.changed, 1);
        assert_eq!(summary.skipped, 0);

        // 0.8 over two half-lives ≈ 0.2, comfortably above the 0.05 floor.
        let rows = store.links_for_decay(10).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert!(
            (rows[0].strength - 0.2).abs() < 1e-3,
            "got {}",
            rows[0].strength
        );

        store.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_prunes_below_floor_when_configured() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let store = crate::StoreHandle::start(db, DIM, 2).unwrap();
        let ns = Namespace::Project("decay-prune".to_string());

        let a = rb_types::MemoryNote::new(
            ns.clone(),
            "source".to_string(),
            rb_types::MemoryType::Insight,
            5,
        );
        let b = rb_types::MemoryNote::new(
            ns.clone(),
            "target".to_string(),
            rb_types::MemoryType::Insight,
            5,
        );
        let (aid, bid) = (a.id.clone(), b.id.clone());
        store.write(a, Some(vec![0.1f32; DIM])).await.unwrap();
        store.write(b, Some(vec![0.2f32; DIM])).await.unwrap();

        // Very old + weak: 0.1 over ~10 half-lives -> well under the 0.05 floor.
        let created = chrono::Utc::now() - chrono::Duration::days(300);
        store
            .add_link(rb_types::MemoryLink {
                source_id: aid.clone(),
                target_id: bid.clone(),
                link_type: rb_types::LinkType::References,
                strength: 0.1,
                reason: "seed".to_string(),
                created_at: created,
            })
            .await
            .unwrap();

        let cfg = LinkDecayConfig {
            enabled: true,
            interval_secs: 86_400,
            half_life_days: 30.0,
            floor: 0.05,
            prune_below_floor: true,
            batch_limit: 1000,
        };
        let summary = run_at(&store, &cfg, chrono::Utc::now()).await.unwrap();
        assert_eq!(summary.scanned, 1);
        assert_eq!(summary.changed, 1);

        let rows = store.links_for_decay(10).await.unwrap();
        assert!(rows.is_empty(), "weak old link pruned below floor");

        store.shutdown().await;
    }
}
