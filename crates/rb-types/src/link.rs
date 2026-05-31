use crate::link_type::LinkType;
use crate::memory_id::MemoryId;
use serde::{Deserialize, Serialize};

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
