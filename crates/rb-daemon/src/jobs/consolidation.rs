//! Consolidation job: merge near-duplicate memories by superseding every
//! duplicate of a cluster into a single deterministically-chosen survivor.
//! Bounded, idempotent, and namespace-isolated (see `run`).

use rb_types::MemoryId;

/// The minimal metadata the survivor policy needs. Kept tiny so `pick_survivor`
/// is a pure function over plain data, independent of the store.
// Constructed by `run` (added in the next task) and by the survivor-policy
// tests; allow dead_code until the `run` job wires it in.
#[allow(dead_code)]
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
// Called by `run` (added in the next task) and the survivor-policy tests; allow
// dead_code until the `run` job wires it in.
#[allow(dead_code)]
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
}
