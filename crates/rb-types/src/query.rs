use crate::memory::MemoryNote;
use crate::memory_type::MemoryType;
use crate::namespace::Namespace;
use serde::{Deserialize, Serialize};

/// A hybrid-search request. `Default` yields an empty, unscoped, unlimited query.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchQuery {
    pub query: String,
    /// Reserved; not honored — scope is fixed at the daemon handshake.
    pub scope: Option<Namespace>,
    pub memory_type: Option<MemoryType>,
    pub tags: Vec<String>,
    pub limit: usize,
}

/// A single ranked search hit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub memory: MemoryNote,
    pub score: f32,
}

/// Partial update for a memory; `None` fields are left unchanged.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemoryUpdates {
    pub content: Option<String>,
    pub summary: Option<String>,
    pub importance: Option<u8>,
    pub tags: Option<Vec<String>>,
    pub context: Option<String>,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::memory::MemoryNote;
    use crate::memory_type::MemoryType;
    use crate::namespace::Namespace;

    #[test]
    fn search_query_default_is_empty() {
        let q = SearchQuery::default();
        assert_eq!(q.query, "");
        assert!(q.scope.is_none());
        assert!(q.memory_type.is_none());
        assert!(q.tags.is_empty());
        assert_eq!(q.limit, 0);
    }

    #[test]
    fn search_query_round_trip() {
        let q = SearchQuery {
            query: "transactions".to_string(),
            scope: Some(Namespace::Global),
            memory_type: Some(MemoryType::BugFix),
            tags: vec!["sqlite".to_string()],
            limit: 10,
        };
        let json = serde_json::to_string(&q).unwrap();
        let back: SearchQuery = serde_json::from_str(&json).unwrap();
        assert_eq!(back.query, "transactions");
        assert_eq!(back.scope, Some(Namespace::Global));
        assert_eq!(back.memory_type, Some(MemoryType::BugFix));
        assert_eq!(back.tags, vec!["sqlite".to_string()]);
        assert_eq!(back.limit, 10);
    }

    #[test]
    fn search_result_round_trip() {
        let memory = MemoryNote::new(
            Namespace::Global,
            "content".to_string(),
            MemoryType::Insight,
            5,
        );
        let result = SearchResult {
            memory: memory.clone(),
            score: 0.9,
        };
        let json = serde_json::to_string(&result).unwrap();
        let back: SearchResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back.memory, memory);
        assert!((back.score - 0.9).abs() < f32::EPSILON);
    }

    #[test]
    fn memory_updates_default_is_all_none() {
        let u = MemoryUpdates::default();
        assert!(u.content.is_none());
        assert!(u.summary.is_none());
        assert!(u.importance.is_none());
        assert!(u.tags.is_none());
        assert!(u.context.is_none());
    }

    #[test]
    fn memory_updates_round_trip() {
        let u = MemoryUpdates {
            content: Some("new body".to_string()),
            summary: Some("new summary".to_string()),
            importance: Some(9),
            tags: Some(vec!["x".to_string(), "y".to_string()]),
            context: Some("ctx".to_string()),
        };
        let json = serde_json::to_string(&u).unwrap();
        let back: MemoryUpdates = serde_json::from_str(&json).unwrap();
        assert_eq!(back.content, Some("new body".to_string()));
        assert_eq!(back.summary, Some("new summary".to_string()));
        assert_eq!(back.importance, Some(9));
        assert_eq!(back.tags, Some(vec!["x".to_string(), "y".to_string()]));
        assert_eq!(back.context, Some("ctx".to_string()));
    }
}
