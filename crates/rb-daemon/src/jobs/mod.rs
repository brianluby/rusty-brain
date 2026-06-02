//! Opt-in background "evolution" jobs: bounded, idempotent maintenance passes
//! that read via the read pool and mutate ONLY through the single writer.

mod config;
pub mod consolidation;
mod importance;
mod link_decay;
pub mod scheduler;

pub use config::{ConsolidationConfig, ImportanceConfig, JobsConfig, LinkDecayConfig};
pub use rb_types::JobKind;

use crate::StoreHandle;
use serde::{Deserialize, Serialize};

/// What a single job pass touched. Returned by `run_once`, logged by the
/// scheduler, and surfaced to the CLI via `Response::JobRan`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobSummary {
    pub scanned: u64,
    pub changed: u64,
    pub skipped: u64,
}

/// Run ONE bounded, idempotent pass of `kind`. Reads via the read pool; every
/// mutation goes through `store` (the single writer). Fail-safe: returns `Err`
/// on failure without leaving partial state (each write is its own txn); never
/// panics. Dispatches to the per-job `run` with the matching sub-config.
pub async fn run_once(
    kind: JobKind,
    store: &StoreHandle,
    config: &JobsConfig,
) -> rb_types::Result<JobSummary> {
    match kind {
        JobKind::LinkDecay => link_decay::run(store, &config.link_decay).await,
        JobKind::Consolidation => consolidation::run(store, &config.consolidation).await,
        JobKind::ImportanceRecalibration => Err(rb_types::Error::InvalidArgument(
            "importance recalibration job is not implemented yet".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn job_summary_default_is_all_zero() {
        let s = JobSummary::default();
        assert_eq!(s.scanned, 0);
        assert_eq!(s.changed, 0);
        assert_eq!(s.skipped, 0);
    }

    #[test]
    fn job_summary_round_trips_json() {
        let s = JobSummary {
            scanned: 9,
            changed: 4,
            skipped: 5,
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: JobSummary = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_once_link_decay_on_empty_store_scans_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let store = crate::StoreHandle::start(db, 8, 1).unwrap();
        let config = JobsConfig::default();

        let summary = run_once(JobKind::LinkDecay, &store, &config).await.unwrap();
        assert_eq!(summary, JobSummary::default());

        store.shutdown().await;
    }
}
