//! Interval scheduler for enabled evolution jobs. Spawns one tokio task per
//! enabled job inside a `JoinSet`; each ticks on its `interval_secs` and calls
//! `run_once`, logging the summary at info and errors at warn. A job error is
//! logged and the loop continues (never fatal, never unwraps). Disabled jobs are
//! never scheduled. The returned `JoinHandle` is aborted by `Daemon::run` on
//! shutdown; aborting it drops the `JoinSet`, which aborts every job task.

use crate::jobs::{run_once, JobKind, JobsConfig};
use crate::StoreHandle;
use std::time::Duration;
use tokio::task::{JoinHandle, JoinSet};
use tracing::{info, warn};

/// Spawn the job supervisor. Returns a single `JoinHandle` whose future owns a
/// `JoinSet` of per-job tasks. Aborting the returned handle drops that future
/// (and thus the `JoinSet`); a `JoinSet` aborts all of its tasks when dropped, so
/// every job tick loop is actually cancelled on shutdown. (A bare `JoinHandle`
/// would only *detach* on drop, leaving the loop running — hence the `JoinSet`.)
pub fn spawn(store: StoreHandle, config: JobsConfig) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut jobs: JoinSet<()> = JoinSet::new();

        if config.link_decay.enabled {
            spawn_job(
                &mut jobs,
                JobKind::LinkDecay,
                config.link_decay.interval_secs,
                store.clone(),
                config.clone(),
            );
        }
        if config.consolidation.enabled {
            spawn_job(
                &mut jobs,
                JobKind::Consolidation,
                config.consolidation.interval_secs,
                store.clone(),
                config.clone(),
            );
        }
        if config.importance.enabled {
            spawn_job(
                &mut jobs,
                JobKind::ImportanceRecalibration,
                config.importance.interval_secs,
                store.clone(),
                config.clone(),
            );
        }

        if jobs.is_empty() {
            // Nothing enabled: return immediately so a disabled config spawns no
            // long-lived task.
            return;
        }

        // Keep the `JoinSet` (and thus the jobs) alive until this supervisor is
        // aborted. `join_next` only resolves when a job task ends, which happens
        // only on panic — the tick loops never return otherwise. When
        // `Daemon::run` aborts this supervisor, `jobs` is dropped and the
        // `JoinSet` aborts every remaining job task.
        while jobs.join_next().await.is_some() {}
    })
}

/// Spawn a single job's tick loop into `jobs`. The first tick fires immediately,
/// then every `max(interval_secs, 1)` seconds. Each tick is fail-safe: an error
/// is logged at warn and the loop continues.
fn spawn_job(
    jobs: &mut JoinSet<()>,
    kind: JobKind,
    interval_secs: u64,
    store: StoreHandle,
    config: JobsConfig,
) {
    jobs.spawn(async move {
        let period = Duration::from_secs(interval_secs.max(1));
        let mut ticker = tokio::time::interval(period);
        // Default MissedTickBehavior::Burst is fine: we never want to skip work,
        // and ticks are seconds apart at minimum.
        loop {
            ticker.tick().await;
            match run_once(kind, &store, &config).await {
                Ok(summary) => info!(
                    job = kind.as_str(),
                    scanned = summary.scanned,
                    changed = summary.changed,
                    skipped = summary.skipped,
                    "evolution job completed"
                ),
                Err(e) => warn!(job = kind.as_str(), error = %e, "evolution job failed"),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::jobs::{JobsConfig, LinkDecayConfig};
    use crate::StoreHandle;
    use rb_engine::MemoryBackend;
    use rb_types::Namespace;

    const DIM: usize = 8;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn enabled_link_decay_job_runs_on_its_interval() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let store = StoreHandle::start(db, DIM, 2).unwrap();
        let ns = Namespace::Project("sched".to_string());

        let a = rb_types::MemoryNote::new(
            ns.clone(),
            "s".to_string(),
            rb_types::MemoryType::Insight,
            5,
        );
        let b = rb_types::MemoryNote::new(
            ns.clone(),
            "t".to_string(),
            rb_types::MemoryType::Insight,
            5,
        );
        let (aid, bid) = (a.id.clone(), b.id.clone());
        store.write(a, Some(vec![0.1f32; DIM])).await.unwrap();
        store.write(b, Some(vec![0.2f32; DIM])).await.unwrap();
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

        // Tiny interval so the first tick fires almost immediately.
        let config = JobsConfig {
            link_decay: LinkDecayConfig {
                enabled: true,
                interval_secs: 0,
                half_life_days: 30.0,
                floor: 0.05,
                prune_below_floor: false,
                batch_limit: 1000,
            },
            ..Default::default()
        };

        let handle = spawn(store.clone(), config);

        // Poll until the strength has been reduced by the running job.
        let mut decayed = false;
        for _ in 0..50 {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            let rows = store.links_for_decay(10).await.unwrap();
            if !rows.is_empty() && rows[0].strength < 0.79 {
                decayed = true;
                break;
            }
        }
        assert!(
            decayed,
            "enabled link-decay job must run and reduce strength"
        );

        handle.abort();
        store.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn all_disabled_config_spawns_no_work() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let store = StoreHandle::start(db, DIM, 1).unwrap();

        // Default config: every job disabled -> the join handle finishes promptly
        // (no jobs scheduled, the supervisor returns immediately).
        let handle = spawn(store.clone(), JobsConfig::default());
        let joined = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;
        assert!(
            joined.is_ok(),
            "disabled config must not spawn any ticking job"
        );

        store.shutdown().await;
    }
}
