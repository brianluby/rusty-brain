//! Consolidation job: merge near-duplicate memories by superseding every
//! duplicate of a cluster into a single deterministically-chosen survivor.
//! Bounded, idempotent, and namespace-isolated (see `run`).

use crate::jobs::ConsolidationConfig;
use crate::jobs::JobSummary;
use crate::StoreHandle;
use rb_types::{MemoryId, Result};
use std::collections::HashSet;

/// The consolidation scan row, re-exported from the store so the daemon read
/// helper and the job share exactly one type.
pub use rb_store::ConsolidationCandidate as Candidate;

/// The minimal metadata the survivor policy needs. Kept tiny so `pick_survivor`
/// is a pure function over plain data, independent of the store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryMeta {
    pub id: MemoryId,
    pub importance: u8,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Choose the survivor of a duplicate cluster deterministically.
///
/// Order of preference (each later key only breaks ties of all earlier keys):
/// 1. highest `importance`,
/// 2. newest `created_at`,
/// 3. lexicographically-smallest id string (total, stable final tiebreak).
///
/// Returns the chosen id. `candidates` must be non-empty; an empty slice is a
/// caller bug and yields `None` so the caller can skip the cluster rather than
/// panic.
pub fn pick_survivor(candidates: &[MemoryMeta]) -> Option<MemoryId> {
    candidates
        .iter()
        .max_by(|a, b| {
            a.importance
                .cmp(&b.importance)
                .then_with(|| a.created_at.cmp(&b.created_at))
                // For the FINAL tiebreak we want the SMALLEST id to win. `max_by`
                // returns the greatest element, so invert the id comparison:
                // a "greater" element here is the one with the smaller id.
                .then_with(|| b.id.to_string().cmp(&a.id.to_string()))
        })
        .map(|m| m.id.clone())
}

/// Run ONE bounded, idempotent, namespace-isolated consolidation pass.
///
/// Algorithm: read up to `batch_limit` active, non-superseded candidates
/// (deterministic order). For each candidate `m` not already consumed by an
/// earlier cluster, look up its near-duplicates WITHIN m's own namespace; if it
/// has none, it is `skipped`. Otherwise form the cluster from `m` plus the
/// near-duplicates that also appear in this pass's candidate window, pick a
/// deterministic survivor, and supersede every OTHER member into the survivor,
/// marking each consumed so it is never revisited. `scanned` counts candidates
/// examined; `changed` counts supersede writes; `skipped` counts candidates with
/// no duplicate.
///
/// Idempotency: a superseded/archived member is excluded from the candidate scan
/// AND from `near_duplicates` (both filter `archived_at IS NULL`), so a second
/// run over unchanged data performs zero writes. Never merges across namespaces:
/// `near_duplicates` is namespace-scoped, so a cluster only ever contains
/// same-namespace members.
///
/// Convergence (bounded batches): both the candidate scan and `near_duplicates`
/// are capped at `batch_limit`. When a namespace holds more active memories than
/// `batch_limit`, a near-duplicate whose `created_at` sorts outside the candidate
/// window is deferred, not dropped — it is scanned as its own anchor on a later
/// pass and re-pairs with its still-active twin (similarity is symmetric), and
/// every merge only shrinks the active set. The job therefore converges across
/// repeated scheduled passes; set `batch_limit` >= the per-namespace active count
/// for single-pass completeness.
pub async fn run(store: &StoreHandle, config: &ConsolidationConfig) -> Result<JobSummary> {
    let candidates = store
        .candidates_for_consolidation(config.batch_limit)
        .await?;

    let mut summary = JobSummary::default();
    let mut consumed: HashSet<String> = HashSet::new();

    for cand in &candidates {
        summary.scanned += 1;

        // Already absorbed into an earlier cluster this pass: skip silently.
        if consumed.contains(&cand.id.to_string()) {
            continue;
        }

        // Same-namespace near-duplicates only (namespace isolation guaranteed by
        // the store read). `limit` is the batch budget — a cluster cannot exceed
        // the scan size.
        let dups = store
            .near_duplicates(
                cand.namespace.clone(),
                cand.id.clone(),
                config.similarity_threshold,
                config.batch_limit,
            )
            .await?;

        // Drop any dup already consumed this pass (it belongs to an earlier
        // cluster); also drop the anchor if it somehow appears.
        let live_dups: Vec<Candidate> = dups
            .into_iter()
            .filter(|(dup_id, _)| dup_id != &cand.id && !consumed.contains(&dup_id.to_string()))
            .filter_map(|(dup_id, _)| candidates.iter().find(|c| c.id == dup_id).cloned())
            .collect();

        if live_dups.is_empty() {
            summary.skipped += 1;
            continue;
        }

        // Build the cluster metadata and choose the survivor deterministically.
        let mut cluster: Vec<MemoryMeta> = Vec::with_capacity(live_dups.len() + 1);
        cluster.push(MemoryMeta {
            id: cand.id.clone(),
            importance: cand.importance,
            created_at: cand.created_at,
        });
        for d in &live_dups {
            cluster.push(MemoryMeta {
                id: d.id.clone(),
                importance: d.importance,
                created_at: d.created_at,
            });
        }
        let Some(survivor_id) = pick_survivor(&cluster) else {
            // Cluster is non-empty here, so this is unreachable in practice; skip
            // defensively rather than panic.
            summary.skipped += 1;
            continue;
        };

        // Supersede every member that is not the survivor into the survivor.
        for member in &cluster {
            if member.id == survivor_id {
                continue;
            }
            if supersede_member(store, &cand.namespace, &member.id, &survivor_id).await? {
                summary.changed += 1;
            } else {
                summary.skipped += 1;
            }
            consumed.insert(member.id.to_string());
        }
        // The survivor (and the anchor, if it survived) is consumed so it is not
        // re-clustered as a fresh anchor later in the same pass.
        consumed.insert(survivor_id.to_string());
        consumed.insert(cand.id.to_string());
    }

    Ok(summary)
}

/// Supersede one cluster member into the survivor, tolerating the read/write
/// race (#501 caller compatibility): the candidate window and near-dup lookup
/// are reads, so a live daemon can resolve a member (remember --supersedes, a
/// review merge, another writer) before this job's write lands. The store's
/// supersede guard turns that into [`rb_types::Error::StalePlan`]; a bounded
/// maintenance pass must skip past the benign collision (the review
/// policy-sweep discipline) instead of aborting — the winner's pointer stays
/// exactly as the winner wrote it. Every OTHER error (including a vanished
/// member: `NotFound`) still fails the pass.
///
/// Returns `Ok(true)` when the member was superseded, `Ok(false)` on a
/// benign lost race.
async fn supersede_member(
    store: &StoreHandle,
    namespace: &rb_types::Namespace,
    member: &MemoryId,
    survivor: &MemoryId,
) -> Result<bool> {
    match store
        .supersede(namespace.clone(), member.clone(), survivor.clone())
        .await
    {
        Ok(()) => Ok(true),
        Err(rb_types::Error::StalePlan(reason)) => {
            tracing::debug!(
                member = %member,
                survivor = %survivor,
                %reason,
                "consolidation lost a supersede race; skipping the member"
            );
            Ok(false)
        }
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use rb_types::MemoryId;

    fn meta(importance: u8, created_secs: i64) -> MemoryMeta {
        MemoryMeta {
            id: MemoryId::new(),
            importance,
            created_at: chrono::DateTime::<chrono::Utc>::from_timestamp(created_secs, 0)
                .expect("valid timestamp"),
        }
    }

    #[test]
    fn empty_cluster_returns_none() {
        assert!(pick_survivor(&[]).is_none());
    }

    #[test]
    fn single_candidate_is_the_survivor() {
        let only = meta(5, 100);
        assert_eq!(
            pick_survivor(std::slice::from_ref(&only)),
            Some(only.id.clone())
        );
    }

    #[test]
    fn higher_importance_wins() {
        // b has higher importance even though a is newer.
        let a = meta(3, 200);
        let b = meta(9, 100);
        assert_eq!(
            pick_survivor(&[a, b.clone()]),
            Some(b.id),
            "highest importance must win regardless of recency"
        );
    }

    #[test]
    fn equal_importance_newest_created_wins() {
        // Same importance; b is newer (larger created_at) -> b wins.
        let a = meta(7, 100);
        let b = meta(7, 500);
        assert_eq!(
            pick_survivor(&[a, b.clone()]),
            Some(b.id),
            "with equal importance the newest created_at wins"
        );
    }

    #[test]
    fn equal_importance_and_time_smallest_id_wins() {
        // Identical importance + created_at: the lexicographically-smallest id wins.
        let ts = 300;
        let one = meta(5, ts);
        let two = meta(5, ts);
        let mut both = vec![one.clone(), two.clone()];
        let expected = if one.id.to_string() < two.id.to_string() {
            one.id.clone()
        } else {
            two.id.clone()
        };
        assert_eq!(pick_survivor(&both), Some(expected.clone()));
        // Order-independence: reversing the input yields the SAME survivor.
        both.reverse();
        assert_eq!(
            pick_survivor(&both),
            Some(expected),
            "survivor must not depend on input order"
        );
    }

    use crate::jobs::ConsolidationConfig;
    use crate::StoreHandle;
    use rb_engine::MemoryBackend;
    use rb_types::{MemoryNote, MemoryType, Namespace};

    const DIM: usize = 8;

    fn vnote(ns: &Namespace, body: &str, importance: u8) -> MemoryNote {
        MemoryNote::new(
            ns.clone(),
            body.to_string(),
            MemoryType::Insight,
            importance,
        )
    }

    fn cfg(threshold: f32, batch_limit: usize) -> ConsolidationConfig {
        ConsolidationConfig {
            enabled: true,
            interval_secs: 86_400,
            similarity_threshold: threshold,
            batch_limit,
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn supersede_member_reports_changed_on_the_happy_path() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let handle = StoreHandle::start(db, DIM, 2).unwrap();
        let ns = Namespace::Project("tolerant".to_string());

        let member = vnote(&ns, "member", 3);
        let survivor = vnote(&ns, "survivor", 9);
        let (member_id, survivor_id) = (member.id.clone(), survivor.id.clone());
        handle
            .write(
                member,
                Some(vec![1.0f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
            )
            .await
            .unwrap();
        handle
            .write(
                survivor,
                Some(vec![1.0f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
            )
            .await
            .unwrap();

        let changed = supersede_member(&handle, &ns, &member_id, &survivor_id)
            .await
            .unwrap();
        assert!(changed, "an unresolved member is superseded");
        let got = handle
            .get(ns.clone(), member_id.clone())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.superseded_by.as_ref(), Some(&survivor_id));

        handle.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn supersede_member_skips_a_concurrently_resolved_member() {
        // #501 caller-compatibility: the job reads its candidate window, then
        // writes one supersede per member — a live daemon can resolve a
        // member in between (remember --supersedes, a review merge, another
        // job). The store guard turns that race into StalePlan; the job must
        // treat it as skip-and-continue (the review policy-sweep discipline),
        // NOT abort the pass, and must never rewrite the winner's pointer.
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let handle = StoreHandle::start(db, DIM, 2).unwrap();
        let ns = Namespace::Project("tolerant".to_string());

        let member = vnote(&ns, "member", 3);
        let winner = vnote(&ns, "concurrent winner", 5);
        let survivor = vnote(&ns, "survivor", 9);
        let (member_id, winner_id, survivor_id) =
            (member.id.clone(), winner.id.clone(), survivor.id.clone());
        for (n, v) in [
            (member, vec![1.0f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
            (winner, vec![0.0f32, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
            (survivor, vec![1.0f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
        ] {
            handle.write(n, Some(v)).await.unwrap();
        }

        // The "concurrent" resolution lands first: member -> winner.
        handle
            .supersede(ns.clone(), member_id.clone(), winner_id.clone())
            .await
            .unwrap();

        // The job's write loses the race: skipped (Ok(false)), never an Err.
        let changed = supersede_member(&handle, &ns, &member_id, &survivor_id)
            .await
            .unwrap();
        assert!(!changed, "a lost race is a benign skip, not a change");
        let got = handle
            .get(ns.clone(), member_id.clone())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            got.superseded_by.as_ref(),
            Some(&winner_id),
            "the winner's pointer must never be rewritten"
        );

        // A real failure still propagates: a vanished member is NOT benign.
        let err = supersede_member(&handle, &ns, &rb_types::MemoryId::new(), &survivor_id)
            .await
            .unwrap_err();
        assert!(matches!(err, rb_types::Error::NotFound(_)), "got {err:?}");

        handle.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn merges_twins_in_same_namespace_only() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let handle = StoreHandle::start(db, DIM, 2).unwrap();
        let ns_a = Namespace::Project("a".to_string());
        let ns_b = Namespace::Project("b".to_string());

        // Two near-identical memories in A (the survivor has higher importance).
        let survivor = vnote(&ns_a, "twin survivor", 9);
        let dup = vnote(&ns_a, "twin dup", 3);
        let (survivor_id, dup_id) = (survivor.id.clone(), dup.id.clone());
        handle
            .write(
                survivor,
                Some(vec![1.0f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
            )
            .await
            .unwrap();
        handle
            .write(dup, Some(vec![1.0f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]))
            .await
            .unwrap();

        // A near-identical memory in B: must NOT be merged with A's cluster.
        let foreign = vnote(&ns_b, "foreign twin", 9);
        let foreign_id = foreign.id.clone();
        handle
            .write(
                foreign,
                Some(vec![1.0f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
            )
            .await
            .unwrap();

        let summary = run(&handle, &cfg(0.95, 200)).await.unwrap();
        assert_eq!(summary.changed, 1, "exactly one duplicate superseded");

        // The lower-importance dup is archived and points at the survivor.
        let got_dup = handle
            .get(ns_a.clone(), dup_id.clone())
            .await
            .unwrap()
            .unwrap();
        assert!(got_dup.archived_at.is_some(), "dup archived");
        assert_eq!(got_dup.superseded_by.as_ref(), Some(&survivor_id));

        // The survivor stays active and is NOT superseded.
        let got_survivor = handle
            .get(ns_a.clone(), survivor_id.clone())
            .await
            .unwrap()
            .unwrap();
        assert!(got_survivor.archived_at.is_none());
        assert!(got_survivor.superseded_by.is_none());

        // The foreign memory in B is completely untouched (namespace isolation).
        let got_foreign = handle
            .get(ns_b.clone(), foreign_id.clone())
            .await
            .unwrap()
            .unwrap();
        assert!(
            got_foreign.archived_at.is_none(),
            "ns-B memory never merged"
        );
        assert!(got_foreign.superseded_by.is_none());

        handle.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn is_idempotent_second_run_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let handle = StoreHandle::start(db, DIM, 2).unwrap();
        let ns = Namespace::Project("a".to_string());

        let a = vnote(&ns, "twin a", 9);
        let b = vnote(&ns, "twin b", 3);
        handle
            .write(a, Some(vec![1.0f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]))
            .await
            .unwrap();
        handle
            .write(b, Some(vec![1.0f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]))
            .await
            .unwrap();

        let first = run(&handle, &cfg(0.95, 200)).await.unwrap();
        assert_eq!(first.changed, 1, "first run merges the duplicate");

        // Second run: the duplicate is now archived/superseded and excluded, so
        // there is nothing left to merge.
        let second = run(&handle, &cfg(0.95, 200)).await.unwrap();
        assert_eq!(second.changed, 0, "second run must be a no-op (idempotent)");

        handle.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn distinct_memories_are_not_merged_and_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let handle = StoreHandle::start(db, DIM, 2).unwrap();
        let ns = Namespace::Project("a".to_string());

        // Two orthogonal vectors: raw cosine similarity ~0.0, far below 0.95 threshold.
        let x = vnote(&ns, "topic x", 5);
        let y = vnote(&ns, "topic y", 5);
        let (x_id, y_id) = (x.id.clone(), y.id.clone());
        handle
            .write(x, Some(vec![1.0f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]))
            .await
            .unwrap();
        handle
            .write(y, Some(vec![0.0f32, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]))
            .await
            .unwrap();

        let summary = run(&handle, &cfg(0.95, 200)).await.unwrap();
        assert_eq!(summary.changed, 0, "distinct memories are never merged");
        assert!(
            summary.skipped >= 2,
            "both memories counted as skipped (no dup)"
        );

        // Both remain active.
        assert!(handle
            .get(ns.clone(), x_id)
            .await
            .unwrap()
            .unwrap()
            .archived_at
            .is_none());
        assert!(handle
            .get(ns.clone(), y_id)
            .await
            .unwrap()
            .unwrap()
            .archived_at
            .is_none());

        handle.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_once_consolidation_arm_returns_summary() {
        use crate::jobs::{run_once, JobKind, JobsConfig};
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let handle = StoreHandle::start(db, DIM, 2).unwrap();
        let ns = Namespace::Project("a".to_string());

        let a = vnote(&ns, "twin a", 9);
        let b = vnote(&ns, "twin b", 3);
        handle
            .write(a, Some(vec![1.0f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]))
            .await
            .unwrap();
        handle
            .write(b, Some(vec![1.0f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]))
            .await
            .unwrap();

        // Drive through the shared run_once entry point with the Consolidation kind.
        let config = JobsConfig {
            consolidation: cfg(0.95, 200),
            ..Default::default()
        };
        let summary = run_once(JobKind::Consolidation, &handle, &config, None)
            .await
            .unwrap();
        assert_eq!(
            summary.changed, 1,
            "run_once(Consolidation) must merge via the same path"
        );

        handle.shutdown().await;
    }
}
