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
    /// Stamp of the composition that produced this row's vector (P5 Feature A).
    /// Empty until set at write by the engine; the `reembed` batch re-embeds
    /// rows whose `(embedding_model, embedding_input_version)` stamp is stale.
    /// `#[serde(default)]` so pre-P5 payloads (which lack this field) deserialize
    /// to an empty stamp — already the valid "stale, re-embed me" sentinel.
    #[serde(default)]
    pub embedding_input_version: String,
    pub links: Vec<MemoryLink>,
    /// Read-side annotation (P5 Feature C): `true` when this memory has at least
    /// one ACTIVE `contradicts` link (inbound or outbound). NOT persisted —
    /// computed per recall/get/list/context from `memory_links`. Additive and
    /// `#[serde(default)]` so older payloads (and clients) default to `false`.
    #[serde(default)]
    pub contested: bool,
    /// Provenance (W0.5): who/where/what wrote this memory. All optional and
    /// `#[serde(default)]` (the `contested` precedent): rows written before the
    /// 004 migration — and payloads from older clients — carry `None`.
    /// `origin_user`/`origin_host` are filled daemon-side (whoami fallback);
    /// `origin_agent`/`session_id` come from the client's handshake identity.
    #[serde(default)]
    pub origin_user: Option<String>,
    #[serde(default)]
    pub origin_host: Option<String>,
    #[serde(default)]
    pub origin_agent: Option<String>,
    /// Producer surface that declared the write: `hook` | `mcp` | `cli` | `job`.
    #[serde(default)]
    pub origin_source: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
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
            embedding_input_version: String::new(),
            links: Vec::new(),
            contested: false,
            origin_user: None,
            origin_host: None,
            origin_agent: None,
            origin_source: None,
            session_id: None,
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
        assert_eq!(m.embedding_input_version, "");
        assert!(m.keywords.is_empty());
        assert!(m.tags.is_empty());
        assert!(m.related_files.is_empty());
        assert!(m.links.is_empty());
        assert!(m.last_accessed_at.is_none());
        assert!(m.archived_at.is_none());
        assert!(m.superseded_by.is_none());
        // `contested` is a computed read-side flag (Feature C), not persisted; a
        // fresh note is never contested.
        assert!(!m.contested);
    }

    #[test]
    fn contested_defaults_to_false_when_absent_from_json() {
        // Backward compatibility: an older payload without `contested` must
        // deserialize with `contested == false` (serde default), keeping old
        // clients/data correct.
        let m = sample();
        let mut value = serde_json::to_value(&m).unwrap();
        value.as_object_mut().unwrap().remove("contested");
        let back: MemoryNote = serde_json::from_value(value).unwrap();
        assert!(!back.contested);
        assert_eq!(back.id, m.id);
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

    #[test]
    fn deserializes_pre_w05_payload_missing_provenance_fields() {
        // A pre-W0.5 MemoryNote JSON lacks the five provenance fields; they must
        // deserialize to `None` (serde defaults), keeping old payloads valid.
        let m = sample();
        let mut value = serde_json::to_value(&m).unwrap();
        let obj = value.as_object_mut().unwrap();
        for key in [
            "origin_user",
            "origin_host",
            "origin_agent",
            "origin_source",
            "session_id",
        ] {
            obj.remove(key);
        }
        let back: MemoryNote = serde_json::from_value(value).unwrap();
        assert!(back.origin_user.is_none());
        assert!(back.origin_host.is_none());
        assert!(back.origin_agent.is_none());
        assert!(back.origin_source.is_none());
        assert!(back.session_id.is_none());
    }

    #[test]
    fn deserializes_pre_p5_payload_missing_additive_fields() {
        // A pre-P5 MemoryNote JSON lacks `embedding_input_version` (and
        // `contested`). `#[serde(default)]` must let it deserialize, defaulting
        // the stamp to an empty string — the valid "stale, re-embed me" sentinel.
        let m = sample();
        let mut value = serde_json::to_value(&m).unwrap();
        let obj = value.as_object_mut().unwrap();
        obj.remove("embedding_input_version");
        obj.remove("contested");
        let back: MemoryNote = serde_json::from_value(value).unwrap();
        assert_eq!(back.embedding_input_version, "");
        assert!(!back.contested);
    }
}
