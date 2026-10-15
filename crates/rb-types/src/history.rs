//! Decision-history timeline payloads (PRD 2026-07-02 decision-history
//! timeline). Lives in `rb-types` (the leaf crate) so `rb-store` (the
//! derivation queries), `rb-proto` (the wire `Response::History`) and the CLI
//! (rendering) share one definition without a dependency cycle — the
//! `MemoryStats` precedent.

use crate::{LinkType, MemoryId};
use serde::{Deserialize, Serialize};

/// A read-only memory evolution projection over existing `memories`,
/// `memory_links`, and `memory_anchors` rows. History persists no separate
/// evidence and never resolves source pointers; the daemon performs zero
/// writer ops for this payload.
///
/// Additive-payload guarantee: every field of THIS struct is
/// `#[serde(default)]`, so a peer that predates (or postdates) a top-level
/// field decodes the rest — the `MemoryStats` precedent, no CONTRACT_VERSION
/// bump. The nested [`HistoryEntry`]/[`HistoryEdge`] items keep their
/// identity fields (`id`, `local`/`other`, `link_type`, `created_at`)
/// REQUIRED — no payload version has ever omitted them, and an item without
/// an identity is meaningless; only their annotated metadata fields default.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
pub struct MemoryHistory {
    /// Namespace the timeline is scoped to (db string form). Both endpoints of
    /// every hop and edge live here — the walk never crosses namespaces.
    #[serde(default)]
    pub namespace: String,
    /// The applied per-direction chain-walk bound (post server clamp).
    #[serde(default)]
    pub depth: u32,
    /// Maximum chain members returned, including the target.
    #[serde(default)]
    pub chain_limit: usize,
    /// Maximum active reference edges returned.
    #[serde(default)]
    pub edge_limit: usize,
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
    /// `true` when the bounded result contains a supersede cycle.
    #[serde(default)]
    pub cycle_detected: bool,
    /// `true` when a depth, chain-member, or edge bound omitted rows.
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
    /// Same-namespace direct successor (which can be beyond the depth bound).
    /// Foreign-namespace pointer ids are never serialized.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<MemoryId>,
    /// Same-namespace direct predecessor ids visible in the bounded chain.
    #[serde(default)]
    pub predecessors: Vec<MemoryId>,
    /// Full stored detail for the requested target only. None means an older
    /// daemon did not provide this projection; source pointers are never
    /// opened or validated by history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<HistoryEvidence>,
    #[serde(default)]
    pub origin_user: Option<String>,
    #[serde(default)]
    pub origin_host: Option<String>,
    #[serde(default)]
    pub origin_agent: Option<String>,
    #[serde(default)]
    pub origin_source: Option<String>,
}

/// Stored target detail surfaced by history; this is a read-only projection,
/// not evidence that an assertion or its source pointers are valid.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct HistoryEvidence {
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub context: String,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub memory_type: Option<crate::MemoryType>,
    #[serde(default)]
    pub updated_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    pub last_accessed_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    pub related_files: Vec<String>,
    #[serde(default)]
    pub origin_channel: Option<crate::WriteChannel>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub anchors: Vec<crate::MemoryAnchor>,
    #[serde(default)]
    pub trust_class: crate::TrustClass,
    #[serde(default)]
    pub human_curated: bool,
    /// Source pointers are stored locators only and are never resolved by
    /// this read; legacy rows with no pointers are explicitly unavailable.
    #[serde(default)]
    pub source_status: HistorySourceStatus,
}

/// Whether a target has stored source locators. Recorded pointers are not
/// fetched or checked for validity by this inspection.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HistorySourceStatus {
    #[default]
    Unavailable,
    RecordedUnverified,
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
    use crate::{
        AnchorKind, LinkType, MemoryAnchor, MemoryId, MemoryType, TrustClass, WriteChannel,
    };

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
            predecessors: Vec::new(),
            evidence: None,
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
        let old_id = old.id.clone();
        let mut current = entry("we use kafka");
        old.superseded_by = Some(current.id.clone());
        current.predecessors = vec![old_id];
        let now = chrono::Utc::now();
        current.evidence = Some(HistoryEvidence {
            content: "Use Kafka, except for the legacy event bridge.".to_string(),
            context: "The bridge remains on RabbitMQ until migration completes.".to_string(),
            keywords: vec!["kafka".to_string()],
            tags: vec!["decision".to_string()],
            memory_type: Some(MemoryType::ArchitectureDecision),
            updated_at: Some(now),
            last_accessed_at: Some(now),
            related_files: vec!["src/events.rs".to_string()],
            origin_channel: Some(WriteChannel::Cli),
            session_id: Some("session-1".to_string()),
            anchors: vec![MemoryAnchor {
                kind: AnchorKind::File,
                value: "src/events.rs".to_string(),
                start_line: Some(12),
                end_line: Some(18),
            }],
            trust_class: TrustClass::HumanConfirmed,
            human_curated: true,
            source_status: HistorySourceStatus::RecordedUnverified,
        });
        let history = MemoryHistory {
            namespace: "project:rusty-brain".to_string(),
            depth: 100,
            chain_limit: 200,
            edge_limit: 200,
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
                created_at: now,
            }],
            cycle_detected: false,
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
        assert!(!back.cycle_detected && !back.truncated);
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
        assert!(back.predecessors.is_empty());
        assert!(back.evidence.is_none());
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
    fn history_evidence_defaults_missing_fields_and_marks_sources_unavailable() {
        let evidence: HistoryEvidence = serde_json::from_str("{\"content\":\"legacy\"}").unwrap();
        assert_eq!(evidence.content, "legacy");
        assert_eq!(evidence.source_status, HistorySourceStatus::Unavailable);
        assert!(evidence.anchors.is_empty());
        assert!(!evidence.human_curated);
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
