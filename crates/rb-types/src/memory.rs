use crate::link::MemoryLink;
use crate::memory_id::MemoryId;
use crate::memory_type::MemoryType;
use crate::namespace::Namespace;
use serde::{Deserialize, Serialize};

/// A single unit of memory: content plus enrichment, metadata, and links.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryNote {
    pub id: MemoryId,
    pub namespace: Namespace,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub content: String,
    pub summary: String,
    pub keywords: Vec<String>,
    pub tags: Vec<String>,
    pub context: String,
    pub memory_type: MemoryType,
    /// 1-10 (validated at storage boundary).
    pub importance: u8,
    /// 0.0..=1.0 (validated at storage boundary).
    pub confidence: f32,
    pub related_files: Vec<String>,
    pub access_count: u64,
    pub last_accessed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub archived_at: Option<chrono::DateTime<chrono::Utc>>,
    pub superseded_by: Option<MemoryId>,
    pub embedding_model: String,
    pub links: Vec<MemoryLink>,
}

impl MemoryNote {
    /// Construct a fresh active memory. Generates an id, sets created_at == updated_at
    /// to now, empties all collections, and applies spine defaults
    /// (summary/context empty, confidence 1.0, access_count 0, embedding_model empty).
    pub fn new(
        namespace: Namespace,
        content: String,
        memory_type: MemoryType,
        importance: u8,
    ) -> Self {
        let now = chrono::Utc::now();
        Self {
            id: MemoryId::new(),
            namespace,
            created_at: now,
            updated_at: now,
            content,
            summary: String::new(),
            keywords: Vec::new(),
            tags: Vec::new(),
            context: String::new(),
            memory_type,
            importance,
            confidence: 1.0,
            related_files: Vec::new(),
            access_count: 0,
            last_accessed_at: None,
            archived_at: None,
            superseded_by: None,
            embedding_model: String::new(),
            links: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::memory_type::MemoryType;
    use crate::namespace::Namespace;

    fn sample() -> MemoryNote {
        MemoryNote::new(
            Namespace::Project("rusty-brain".into()),
            "always use one DB and one transaction".to_string(),
            MemoryType::ArchitectureDecision,
            8,
        )
    }

    #[test]
    fn new_sets_constructor_args() {
        let m = sample();
        assert_eq!(m.namespace, Namespace::Project("rusty-brain".into()));
        assert_eq!(m.content, "always use one DB and one transaction");
        assert_eq!(m.memory_type, MemoryType::ArchitectureDecision);
        assert_eq!(m.importance, 8);
    }

    #[test]
    fn new_applies_spine_defaults() {
        let m = sample();
        assert_eq!(m.summary, "");
        assert_eq!(m.context, "");
        assert!((m.confidence - 1.0).abs() < f32::EPSILON);
        assert_eq!(m.access_count, 0);
        assert_eq!(m.embedding_model, "");
        assert!(m.keywords.is_empty());
        assert!(m.tags.is_empty());
        assert!(m.related_files.is_empty());
        assert!(m.links.is_empty());
        assert!(m.last_accessed_at.is_none());
        assert!(m.archived_at.is_none());
        assert!(m.superseded_by.is_none());
    }

    #[test]
    fn new_sets_created_and_updated_equal() {
        let m = sample();
        assert_eq!(m.created_at, m.updated_at);
    }

    #[test]
    fn new_generates_unique_ids() {
        let a = sample();
        let b = sample();
        assert_ne!(a.id, b.id);
    }

    #[test]
    fn serde_json_round_trip_preserves_all_fields() {
        let m = sample();
        let json = serde_json::to_string(&m).unwrap();
        let back: MemoryNote = serde_json::from_str(&json).unwrap();
        assert_eq!(m, back);
    }
}
