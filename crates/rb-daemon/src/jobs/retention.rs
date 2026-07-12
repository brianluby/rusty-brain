//! The scheduled retention/forget job (retention PRD RET-3): one bounded,
//! APPLY-ONLY (archive, never purge) pass per namespace under the single
//! user-global `[retention]` policy. Hard purge is not reachable from a job —
//! only from the admin-gated `Forget` wire op.
//!
//! Off-by-default is enforced at THREE layers on this path: the scheduler
//! never spawns the job without an enabled policy, this `run` returns a
//! zero-work summary for a missing/disabled policy (the on-demand `RunJob`
//! path ignores the scheduler gate), and the store's `retention_sweep`
//! refuses a disabled policy outright.

use crate::jobs::JobSummary;
use crate::StoreHandle;
use rb_types::{ForgetMode, RetentionPolicy};

/// How often the scheduled retention pass runs. Deliberately NOT a config
/// knob (the PRD defines none): a daily bounded sweep is conservative, and a
/// tunable interval would be one more way to typo a destructive surface.
pub const RETENTION_JOB_INTERVAL_SECS: u64 = 86_400;

/// One retention pass: sweep every namespace, archive-only, each bounded by
/// the policy's `batch_limit`. `scanned` counts the memories eligible at
/// sweep time, `changed` the ones actually archived, `skipped` the eligible
/// overflow left for the next pass.
pub async fn run(
    store: &StoreHandle,
    policy: Option<&RetentionPolicy>,
) -> rb_types::Result<JobSummary> {
    let Some(policy) = policy else {
        return Ok(JobSummary::default());
    };
    if !policy.enabled {
        return Ok(JobSummary::default());
    }
    let mut summary = JobSummary::default();
    for namespace in store.retention_namespaces().await? {
        let outcome = store
            .retention_sweep(namespace, policy.clone(), ForgetMode::Apply)
            .await?;
        summary.scanned += outcome.total_eligible;
        summary.changed += outcome.archived;
        summary.skipped += outcome.remaining;
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::jobs::{run_once, JobKind, JobsConfig};
    use crate::StoreHandle;
    use rb_engine::MemoryBackend;
    use rb_types::{MemoryNote, MemoryType, Namespace, RetentionPolicy};

    const DIM: usize = 8;

    fn policy() -> RetentionPolicy {
        RetentionPolicy {
            enabled: true,
            max_age_days: Some(30),
            ..RetentionPolicy::default()
        }
    }

    async fn seed_old_low(store: &StoreHandle, ns: &Namespace) -> rb_types::MemoryId {
        let mut m = MemoryNote::new(
            ns.clone(),
            format!("old low-importance note in {}", ns.as_db_string()),
            MemoryType::Insight,
            2,
        );
        m.created_at = chrono::Utc::now() - chrono::Duration::days(60);
        let id = m.id.clone();
        store.write(m, Some(vec![0.1f32; DIM])).await.unwrap();
        id
    }

    // Off-by-default at the job layer: no policy (or a disabled one) is a
    // zero-work pass even through the on-demand RunJob path, which ignores
    // the scheduler's enabled gate for every other job.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_without_policy_or_disabled_is_a_noop() {
        let dir = tempfile::tempdir().unwrap();
        let handle = StoreHandle::start(dir.path().join("rb.db"), DIM, 1).unwrap();
        let ns = Namespace::Project("job-noop".to_string());
        let id = seed_old_low(&handle, &ns).await;

        let summary = run(&handle, None).await.unwrap();
        assert_eq!(summary, crate::jobs::JobSummary::default());

        let mut disabled = policy();
        disabled.enabled = false;
        let summary = run(&handle, Some(&disabled)).await.unwrap();
        assert_eq!(summary, crate::jobs::JobSummary::default());

        let got = handle.get(ns, id).await.unwrap().unwrap();
        assert!(got.archived_at.is_none(), "nothing was archived");
        handle.shutdown().await;
    }

    // The scheduled job is apply-mode only and sweeps every namespace under
    // the one user-global policy.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_sweeps_every_namespace_apply_only() {
        let dir = tempfile::tempdir().unwrap();
        let handle = StoreHandle::start(dir.path().join("rb.db"), DIM, 2).unwrap();
        let ns_a = Namespace::Project("job-a".to_string());
        let ns_b = Namespace::Global;
        let victim_a = seed_old_low(&handle, &ns_a).await;
        let victim_b = seed_old_low(&handle, &ns_b).await;

        let summary = run(&handle, Some(&policy())).await.unwrap();
        assert_eq!(summary.changed, 2, "one archive per namespace");
        assert_eq!(summary.scanned, 2);

        let got_a = handle
            .get(ns_a.clone(), victim_a.clone())
            .await
            .unwrap()
            .unwrap();
        let got_b = handle.get(ns_b, victim_b).await.unwrap().unwrap();
        assert!(got_a.archived_at.is_some(), "archived, not purged");
        assert!(got_b.archived_at.is_some(), "archived, not purged");

        // Idempotent second pass.
        let summary = run(&handle, Some(&policy())).await.unwrap();
        assert_eq!(summary.changed, 0);
        handle.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_once_dispatches_retention_arm() {
        let dir = tempfile::tempdir().unwrap();
        let handle = StoreHandle::start(dir.path().join("rb.db"), DIM, 1).unwrap();
        let ns = Namespace::Project("job-dispatch".to_string());
        seed_old_low(&handle, &ns).await;

        let summary = run_once(
            JobKind::Retention,
            &handle,
            &JobsConfig::default(),
            Some(&policy()),
        )
        .await
        .unwrap();
        assert_eq!(summary.changed, 1);
        handle.shutdown().await;
    }
}
