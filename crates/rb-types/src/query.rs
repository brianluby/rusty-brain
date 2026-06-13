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

/// Which retrieval channels surfaced a recall hit (W1.0 hit-contribution
/// attribution). A result can be multi-attributed: each flag is `true` when
/// that channel's candidate set contained the memory *before* fusion, so the
/// flags describe contribution, not exclusivity. `Default` (all `false`) is
/// the wire-compat value for frames produced before this field existed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelHits {
    /// The FTS keyword channel surfaced this candidate.
    #[serde(default)]
    pub fts: bool,
    /// The vector (embedding KNN) channel surfaced this candidate.
    #[serde(default)]
    pub vector: bool,
    /// The graph-expansion channel surfaced this candidate.
    #[serde(default)]
    pub graph: bool,
}

/// A single ranked search hit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub memory: MemoryNote,
    pub score: f32,
    /// Per-channel hit attribution (W1.0). `#[serde(default)]` (all-`false`)
    /// keeps old frames decodable — the `contested` additive-field precedent.
    #[serde(default)]
    pub channels: ChannelHits,
}

/// Partial update for a memory; `None` fields are left unchanged.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemoryUpdates {
    pub content: Option<String>,
    pub summary: Option<String>,
    pub importance: Option<u8>,
    pub tags: Option<Vec<String>>,
    pub context: Option<String>,
    /// Trust prior in `0.0..=1.0` (W2.2: the update-path confidence producer).
    /// `#[serde(default)]` (`None`) keeps pre-W2.2 frames decodable in both
    /// directions — the `contested` additive-field precedent. Range-validated
    /// by the engine and again by the store.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
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
            channels: ChannelHits {
                fts: true,
                vector: true,
                graph: false,
            },
        };
        let json = serde_json::to_string(&result).unwrap();
        let back: SearchResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back.memory, memory);
        assert!((back.score - 0.9).abs() < f32::EPSILON);
        assert!(back.channels.fts);
        assert!(back.channels.vector);
        assert!(!back.channels.graph);
    }

    #[test]
    fn search_result_without_channels_field_decodes_to_default() {
        // Wire compat: a frame serialized before `channels` existed must still
        // decode, with all-false attribution (the additive-field precedent).
        let memory = MemoryNote::new(
            Namespace::Global,
            "content".to_string(),
            MemoryType::Insight,
            5,
        );
        let mut value = serde_json::to_value(SearchResult {
            memory,
            score: 0.5,
            channels: ChannelHits::default(),
        })
        .unwrap();
        value.as_object_mut().unwrap().remove("channels").unwrap();
        let back: SearchResult = serde_json::from_value(value).unwrap();
        assert_eq!(back.channels, ChannelHits::default());
        assert!(!back.channels.fts && !back.channels.vector && !back.channels.graph);
    }

    #[test]
    fn memory_updates_default_is_all_none() {
        let u = MemoryUpdates::default();
        assert!(u.content.is_none());
        assert!(u.summary.is_none());
        assert!(u.importance.is_none());
        assert!(u.tags.is_none());
        assert!(u.context.is_none());
        assert!(u.confidence.is_none());
    }

    #[test]
    fn memory_updates_round_trip() {
        let u = MemoryUpdates {
            content: Some("new body".to_string()),
            summary: Some("new summary".to_string()),
            importance: Some(9),
            tags: Some(vec!["x".to_string(), "y".to_string()]),
            context: Some("ctx".to_string()),
            confidence: Some(0.4),
        };
        let json = serde_json::to_string(&u).unwrap();
        let back: MemoryUpdates = serde_json::from_str(&json).unwrap();
        assert_eq!(back.content, Some("new body".to_string()));
        assert_eq!(back.summary, Some("new summary".to_string()));
        assert_eq!(back.importance, Some(9));
        assert_eq!(back.tags, Some(vec!["x".to_string(), "y".to_string()]));
        assert_eq!(back.context, Some("ctx".to_string()));
        assert_eq!(back.confidence, Some(0.4));
    }

    #[test]
    fn memory_updates_confidence_is_wire_compatible_in_both_directions() {
        // Old frame (no `confidence` key) decodes to None; a None confidence
        // serializes WITHOUT the key, keeping the frame byte-identical to the
        // pre-W2.2 shape — the `contested` additive-field precedent.
        let old = serde_json::json!({ "summary": "s" });
        let back: MemoryUpdates = serde_json::from_value(old).unwrap();
        assert!(back.confidence.is_none());

        let none = MemoryUpdates {
            summary: Some("s".to_string()),
            ..Default::default()
        };
        let json = serde_json::to_value(&none).unwrap();
        assert!(
            json.as_object().unwrap().get("confidence").is_none(),
            "None confidence must not serialize: {json}"
        );
    }
}
