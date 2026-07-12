//! Decision-history timeline payloads (PRD 2026-07-02 decision-history
//! timeline). Lives in `rb-types` (the leaf crate) so `rb-store` (the
//! derivation queries), `rb-proto` (the wire `Response::History`) and the CLI
//! (rendering) share one definition without a dependency cycle — the
//! `MemoryStats` precedent.

use crate::{LinkType, MemoryId};
use serde::{Deserialize, Serialize};

/// A memory's evolution derived ENTIRELY from existing rows (HIST-2: the
/// `memories.superseded_by` pointer, `archived_at`/`confidence`/`origin_*`
/// columns, and `memory_links`) — no new persistence. Computed on the daemon's
/// READ pool: the history path issues zero writer ops (W1.8).
///
/// Every field is `#[serde(default)]` so the payload stays additive: a peer
/// that predates (or postdates) a field decodes the rest — the `MemoryStats`
/// precedent, no CONTRACT_VERSION bump.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
pub struct MemoryHistory {
    /// Namespace the timeline is scoped to (db string form). Both endpoints of
    /// every hop and edge live here — the walk never crosses namespaces.
    #[serde(default)]
    pub namespace: String,
    /// The applied per-direction chain-walk bound (post server clamp).
    #[serde(default)]
    pub depth: u32,
    /// The supersede chain ordered oldest first: ancestors (what led to the
    /// requested memory), the requested memory itself (`is_target`), then its
    /// successors. Never empty on success — the target is always present.
    #[serde(default)]
    pub chain: Vec<HistoryEntry>,
    /// Active `contradicts`/`extends`/`references` edges touching a chain
    /// member. "Active" mirrors the `contested` semantics: BOTH endpoints
    /// non-archived and in this namespace.
    #[serde(default)]
    pub edges: Vec<HistoryEdge>,
    /// `true` when the chain walk stopped at the depth bound (or the edge list
    /// at the server's edge cap) with more rows remaining.
    #[serde(default)]
    pub truncated: bool,
}

/// One supersede-chain member with the metadata the timeline renders (HIST-1:
/// summary, importance, confidence, created_at, origin_*) plus its read-side
/// flags. The supersede op records no reason in v1 storage, so hops carry none.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct HistoryEntry {
    pub id: MemoryId,
    /// The memory's summary, falling back to its content when the summary is
    /// empty (the projection rule every other read surface applies).
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub importance: u8,
    #[serde(default)]
    pub confidence: f32,
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// `archived_at IS NOT NULL` — a superseded/deleted chain member.
    #[serde(default)]
    pub archived: bool,
    /// At least one ACTIVE in-namespace `contradicts` edge touches this member
    /// (the same predicate as the `MemoryNote.contested` read flag).
    #[serde(default)]
    pub contested: bool,
    /// The "current truth" pointer: the newest non-archived chain member.
    /// At most one entry carries it; none when the whole chain is archived.
    #[serde(default)]
    pub current: bool,
    /// The chain member the caller asked about.
    #[serde(default)]
    pub is_target: bool,
    /// Raw supersede pointer (`None` for the chain head). May name a memory
    /// outside this namespace or beyond the depth bound; the walk itself never
    /// follows a cross-namespace pointer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<MemoryId>,
    #[serde(default)]
    pub origin_user: Option<String>,
    #[serde(default)]
    pub origin_host: Option<String>,
    #[serde(default)]
    pub origin_agent: Option<String>,
    #[serde(default)]
    pub origin_source: Option<String>,
}

/// One active link edge touching a chain member. `local` is the chain member;
/// `other` is the far endpoint, carried with the summary/confidence/contested
/// state HIST-1 asks for so the timeline reads without a second fetch.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct HistoryEdge {
    pub link_type: LinkType,
    /// The chain member this edge touches (the link's source when both
    /// endpoints are chain members).
    pub local: MemoryId,
    /// The far endpoint (active and in-namespace by construction).
    pub other: MemoryId,
    /// `true` when `local` is the link's source (`local -> other`); `false`
    /// for an inbound edge (`other -> local`).
    #[serde(default)]
    pub outbound: bool,
    /// Why the link exists (stored on the edge at link time).
    #[serde(default)]
    pub reason: String,
    /// The far endpoint's summary (content fallback, like `HistoryEntry`).
    #[serde(default)]
    pub other_summary: String,
    #[serde(default)]
    pub other_confidence: f32,
    #[serde(default)]
    pub other_contested: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::MemoryId;

    fn entry(summary: &str) -> HistoryEntry {
        HistoryEntry {
            id: MemoryId::new(),
            summary: summary.to_string(),
            importance: 7,
            confidence: 0.8,
            created_at: chrono::Utc::now(),
            archived: false,
            contested: true,
            current: true,
            is_target: true,
            superseded_by: None,
            origin_user: Some("alice".to_string()),
            origin_host: Some("devbox".to_string()),
            origin_agent: Some("claude-code".to_string()),
            origin_source: Some("cli".to_string()),
        }
    }

    #[test]
    fn memory_history_round_trips_with_every_field() {
        let mut old = entry("we use rabbitmq");
        old.archived = true;
        old.contested = false;
        old.current = false;
        old.is_target = false;
        let current = entry("we use kafka");
        old.superseded_by = Some(current.id.clone());
        let history = MemoryHistory {
            namespace: "project:rusty-brain".to_string(),
            depth: 100,
            chain: vec![old, current.clone()],
            edges: vec![HistoryEdge {
                link_type: LinkType::Contradicts,
                local: current.id.clone(),
                other: MemoryId::new(),
                outbound: false,
                reason: "bench results disagree".to_string(),
                other_summary: "kafka too slow for us".to_string(),
                other_confidence: 0.6,
                other_contested: true,
                created_at: chrono::Utc::now(),
            }],
            truncated: true,
        };
        let json = serde_json::to_string(&history).unwrap();
        let back: MemoryHistory = serde_json::from_str(&json).unwrap();
        assert_eq!(back, history);
    }

    #[test]
    fn memory_history_decodes_from_an_empty_object() {
        // Every field is additive + serde(default): a payload from an
        // older/newer peer that omits fields must still decode (the
        // MemoryStats precedent), yielding the zero-valued default.
        let back: MemoryHistory = serde_json::from_str("{}").unwrap();
        assert_eq!(back, MemoryHistory::default());
        assert!(back.chain.is_empty());
        assert!(back.edges.is_empty());
        assert!(!back.truncated);
    }

    #[test]
    fn history_entry_tolerates_absent_optional_fields() {
        // A minimal entry (id + created_at only) decodes with defaulted flags
        // and provenance — the additive-payload tolerance the wire relies on.
        let id = MemoryId::new();
        let json = format!("{{\"id\":\"{id}\",\"created_at\":\"2026-07-01T00:00:00Z\"}}");
        let back: HistoryEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, id);
        assert_eq!(back.summary, "");
        assert_eq!(back.importance, 0);
        assert!(!back.archived && !back.contested && !back.current && !back.is_target);
        assert!(back.superseded_by.is_none());
        assert!(back.origin_user.is_none());
    }

    #[test]
    fn history_entry_omits_a_none_supersede_pointer() {
        // skip_serializing_if keeps a chain-head entry free of a null pointer
        // key (the `Remember.supersedes` wire-frugality precedent).
        let e = entry("head");
        let json = serde_json::to_string(&e).unwrap();
        assert!(!json.contains("superseded_by"), "{json}");
    }

    #[test]
    fn history_edge_round_trips_and_defaults() {
        let id = MemoryId::new();
        let other = MemoryId::new();
        let json = format!(
            "{{\"link_type\":\"Contradicts\",\"local\":\"{id}\",\"other\":\"{other}\",\
             \"created_at\":\"2026-07-01T00:00:00Z\"}}"
        );
        let back: HistoryEdge = serde_json::from_str(&json).unwrap();
        assert_eq!(back.link_type, LinkType::Contradicts);
        assert!(!back.outbound);
        assert_eq!(back.reason, "");
        assert_eq!(back.other_summary, "");
        assert!(!back.other_contested);
    }
}
