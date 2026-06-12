use rb_types::{LinkType, MemoryLink, MemoryNote};

/// Generates links for a newly-stored memory from a set of candidate
/// (note, vector_distance) pairs. No IO: the selection logic (which candidates
/// are linked, and their strengths) is deterministic given the same inputs; only
/// each link's `created_at` timestamp reflects wall-clock time.
///
/// WARNING: `link` is SYNCHRONOUS and may be called from within an async
/// context. Implementations that perform blocking I/O (e.g. an LLM linker built
/// on `reqwest::blocking`) MUST be invoked via `tokio::task::spawn_blocking`
/// (or `tokio::task::block_in_place`); calling blocking I/O directly on an async
/// reactor worker thread can stall the runtime or panic.
pub trait Linker: Send + Sync {
    /// Produce links FROM `new` TO selected candidates. `candidates` are
    /// `(note, vector_distance)` where smaller distance = more similar.
    fn link(&self, new: &MemoryNote, candidates: &[(MemoryNote, f32)]) -> Vec<MemoryLink>;
}

/// Default linker: a `References` link to every candidate within
/// `distance_threshold`, strength = `(1 - distance/2).clamp(0,1)`, capped at
/// `max_links`, skipping the new note itself. Offline and deterministic.
///
/// Distances are vec0 COSINE distances (`1 - cosine_similarity`, range
/// `[0, 2]`) since the W1.1 metric rebuild; the default threshold is
/// recalibrated accordingly (see [`Default`]).
pub struct SimilarityLinker {
    max_links: usize,
    distance_threshold: f32,
}

impl SimilarityLinker {
    pub fn new(max_links: usize, distance_threshold: f32) -> Self {
        Self {
            max_links,
            distance_threshold,
        }
    }
}

impl Default for SimilarityLinker {
    /// Conservative defaults: at most 5 links, only fairly-similar candidates.
    ///
    /// The threshold is a cosine DISTANCE
    /// ([`rb_types::SIMILARITY_LINK_MAX_COSINE_DISTANCE`] = 0.18, i.e. raw
    /// cosine similarity >= 0.82). Recalibrated for the W1.1 cosine-metric
    /// rebuild: the previous value was an L2 distance of 0.6, and for unit
    /// vectors `L2 <= 0.6` is exactly `cosine distance <= 0.6^2/2 = 0.18` —
    /// so link creation stays at parity for normalized embeddings. The same
    /// constant gates rb-store's one-shot revalidation of pre-rebuild
    /// `reason = 'similar'` links.
    fn default() -> Self {
        Self {
            max_links: 5,
            distance_threshold: rb_types::SIMILARITY_LINK_MAX_COSINE_DISTANCE,
        }
    }
}

impl Linker for SimilarityLinker {
    fn link(&self, new: &MemoryNote, candidates: &[(MemoryNote, f32)]) -> Vec<MemoryLink> {
        let now = chrono::Utc::now();
        let mut links = Vec::new();
        for (candidate, distance) in candidates {
            if links.len() >= self.max_links {
                break;
            }
            if candidate.id == new.id {
                continue; // never link to self
            }
            if *distance > self.distance_threshold {
                continue;
            }
            let strength = (1.0 - distance / 2.0).clamp(0.0, 1.0);
            links.push(MemoryLink {
                source_id: new.id.clone(),
                target_id: candidate.id.clone(),
                link_type: LinkType::References,
                strength,
                reason: "similar".to_string(),
                created_at: now,
            });
        }
        links
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use rb_types::{LinkType, MemoryNote, MemoryType, Namespace};

    fn note(content: &str) -> MemoryNote {
        MemoryNote::new(
            Namespace::Project("rb".into()),
            content.to_string(),
            MemoryType::Insight,
            5,
        )
    }

    #[test]
    fn links_candidates_within_threshold_only() {
        let new = note("new memory");
        let near = note("near");
        let far = note("far");
        let candidates = vec![(near.clone(), 0.2_f32), (far.clone(), 1.9_f32)];
        let linker = SimilarityLinker::new(10, 1.0);
        let links = linker.link(&new, &candidates);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].source_id, new.id);
        assert_eq!(links[0].target_id, near.id);
        assert_eq!(links[0].link_type, LinkType::References);
        assert_eq!(links[0].reason, "similar");
    }

    #[test]
    fn strength_is_one_minus_half_distance_clamped() {
        let new = note("new");
        let c = note("c");
        let linker = SimilarityLinker::new(10, 2.0);
        // distance 0.0 -> strength 1.0
        let s0 = linker.link(&new, &[(c.clone(), 0.0)])[0].strength;
        assert!((s0 - 1.0).abs() < 1e-6);
        // distance 1.0 -> strength 0.5
        let s1 = linker.link(&new, &[(c.clone(), 1.0)])[0].strength;
        assert!((s1 - 0.5).abs() < 1e-6);
        // distance 2.0 -> strength 0.0 (clamp floor)
        let s2 = linker.link(&new, &[(c.clone(), 2.0)])[0].strength;
        assert!((s2 - 0.0).abs() < 1e-6);
    }

    #[test]
    fn caps_at_max_links_preserving_candidate_order() {
        let new = note("new");
        let a = note("a");
        let b = note("b");
        let c = note("c");
        let candidates = vec![(a.clone(), 0.1), (b.clone(), 0.2), (c.clone(), 0.3)];
        let linker = SimilarityLinker::new(2, 1.0);
        let links = linker.link(&new, &candidates);
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].target_id, a.id);
        assert_eq!(links[1].target_id, b.id);
    }

    #[test]
    fn skips_self_candidate() {
        let new = note("new");
        // A candidate whose id equals the new note's id must be skipped.
        let mut me = note("dup");
        me.id = new.id.clone();
        let other = note("other");
        let candidates = vec![(me, 0.1), (other.clone(), 0.2)];
        let linker = SimilarityLinker::new(10, 1.0);
        let links = linker.link(&new, &candidates);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target_id, other.id);
    }

    #[test]
    fn empty_candidates_yields_no_links() {
        let new = note("new");
        let linker = SimilarityLinker::new(5, 1.0);
        assert!(linker.link(&new, &[]).is_empty());
    }

    #[test]
    fn default_threshold_is_recalibrated_cosine_distance() {
        // W1.1: the default gate is the shared cosine-distance constant, not
        // the old L2 0.6. A candidate just inside 0.18 links; just outside
        // does not.
        let new = note("new");
        let near = note("near");
        let far = note("far");
        let linker = SimilarityLinker::default();
        let links = linker.link(&new, &[(near.clone(), 0.17), (far, 0.19)]);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target_id, near.id);
        assert!(
            (rb_types::SIMILARITY_LINK_MAX_COSINE_DISTANCE - 0.18).abs() < 1e-6,
            "revalidation constant and linker default must stay at 0.18"
        );
    }
}
