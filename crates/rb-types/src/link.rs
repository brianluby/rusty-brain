use crate::link_type::LinkType;
use crate::memory_id::MemoryId;
use serde::{Deserialize, Serialize};

/// Maximum cosine DISTANCE (`1 - cosine_similarity`, range `[0, 2]`) at which
/// the similarity linker creates a `references` link (W1.1 recalibration).
///
/// Derivation: the pre-cosine linker threshold was an L2 distance of `0.6`.
/// For unit vectors `L2 = sqrt(2 - 2*cos_sim)`, so `L2 <= 0.6` is exactly
/// `cos_sim >= 1 - 0.6^2/2 = 0.82`, i.e. cosine distance `<= 0.18`. Using the
/// equivalent value keeps link creation at parity with the old behavior for
/// the normalized embeddings every shipped provider produces.
///
/// Single source of truth: `rb-engine`'s `SimilarityLinker::default` gates new
/// links with it, and `rb-store`'s one-shot vector-schema rebuild re-scores
/// existing `reason = 'similar'` links against it (dropping those above it).
/// It lives here because rb-engine and rb-store do not depend on each other.
pub const SIMILARITY_LINK_MAX_COSINE_DISTANCE: f32 = 0.18;

/// A directed, typed relationship between two memories with a confidence/strength.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryLink {
    pub source_id: MemoryId,
    pub target_id: MemoryId,
    pub link_type: LinkType,
    pub strength: f32,
    pub reason: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::link_type::LinkType;
    use crate::memory_id::MemoryId;
    use chrono::Utc;

    fn sample() -> MemoryLink {
        MemoryLink {
            source_id: MemoryId::new(),
            target_id: MemoryId::new(),
            link_type: LinkType::Extends,
            strength: 0.75,
            reason: "builds on prior decision".to_string(),
            created_at: Utc::now(),
        }
    }

    #[test]
    fn fields_are_accessible() {
        let link = sample();
        assert_eq!(link.link_type, LinkType::Extends);
        assert_eq!(link.reason, "builds on prior decision");
        assert!((link.strength - 0.75).abs() < f32::EPSILON);
        assert_ne!(link.source_id, link.target_id);
    }

    #[test]
    fn serde_json_round_trip_preserves_all_fields() {
        let link = sample();
        let json = serde_json::to_string(&link).unwrap();
        let back: MemoryLink = serde_json::from_str(&json).unwrap();
        assert_eq!(link, back);
    }

    #[test]
    fn clone_equals_original() {
        let link = sample();
        assert_eq!(link.clone(), link);
    }
}
