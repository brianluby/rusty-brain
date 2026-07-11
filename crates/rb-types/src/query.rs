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

/// Archived-state scope for recall/list filtering (PRD 2026-07-02
/// search-filter parity). `Active` is the default and preserves pre-filter
/// behavior (archived rows never surface unless explicitly requested).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryState {
    #[default]
    Active,
    Archived,
    All,
}

impl MemoryState {
    /// `true` for the default (active-only) scope; used by serde
    /// `skip_serializing_if` so a default state adds nothing to the wire.
    #[must_use]
    pub fn is_active(&self) -> bool {
        matches!(self, MemoryState::Active)
    }

    /// Whether a memory with the given archived flag falls inside this scope.
    #[must_use]
    pub fn admits_archived(self, archived: bool) -> bool {
        match self {
            MemoryState::Active => !archived,
            MemoryState::Archived => archived,
            MemoryState::All => true,
        }
    }
}

/// What a code anchor filter points at (PRD 2026-07-02 typed-code-anchors).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnchorKind {
    File,
    Commit,
    Symbol,
}

/// One anchor constraint: scope results to memories anchored to `value`
/// (a file path, commit SHA, or symbol name, per `kind`).
///
/// Wire plumbing only for now: the filter MODEL ships with search-filter
/// parity (PRD 2026-07-02) so the contract needs no second change, but
/// evaluating it requires the `memory_anchors` table from the typed-code-anchors
/// PRD — until that lands, engines/stores reject a non-empty anchor filter
/// with `Error::InvalidArgument` (fail fast, never silently unfiltered).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnchorFilter {
    pub kind: AnchorKind,
    pub value: String,
}

/// The unified recall/list filter model (PRD 2026-07-02 search-filter parity):
/// ONE shape shared by the CLI, MCP, proto, engine, and store so the surfaces
/// can never disagree about what is filterable.
///
/// Every field is optional and `#[serde(default)]` with a skip-serializing
/// default, so:
/// - an old frame without any filter key decodes to the unconstrained default,
/// - an all-default filter serializes to `{}` (and the proto skips the whole
///   field), keeping frames byte-identical to the pre-filter shape — additive,
///   NO CONTRACT_VERSION bump (the `contested` precedent).
///
/// Semantics: `types`/`sources` are any-of; `tags` is all-of; numeric and time
/// ranges are inclusive; `contested` is tri-state (`None` = no constraint);
/// `state` defaults to active-only. `contested` and `anchors` are NOT decided
/// by [`RecallFilter::matches`] (they need link/anchor lookups) — callers
/// evaluate those dimensions where the data lives.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RecallFilter {
    /// Restrict to any of these memory types (empty = no constraint).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub types: Vec<MemoryType>,
    /// Require EVERY listed tag to be present (empty = no constraint).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_importance: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_importance: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_confidence: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_confidence: Option<f32>,
    /// Only memories created at or after this instant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<chrono::DateTime<chrono::Utc>>,
    /// Only memories created at or before this instant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<chrono::DateTime<chrono::Utc>>,
    /// Restrict to any of these producer surfaces (`hook`/`mcp`/`cli`/`job`).
    /// A row without provenance (`origin_source` = `None`) never matches a
    /// non-empty source constraint.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<String>,
    /// Tri-state contested constraint: `Some(true)` = only contested,
    /// `Some(false)` = only uncontested, `None` = no constraint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contested: Option<bool>,
    /// Archived-state scope (default: active-only, the pre-filter behavior).
    #[serde(default, skip_serializing_if = "MemoryState::is_active")]
    pub state: MemoryState,
    /// Code-anchor constraints (see [`AnchorFilter`]): wire plumbing shipped
    /// with search-filter parity; evaluation lands with typed code anchors.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub anchors: Vec<AnchorFilter>,
}

impl RecallFilter {
    /// `true` when every dimension is at its unconstrained default. Used by
    /// serde `skip_serializing_if` on the proto so a no-filter request stays
    /// byte-identical to the pre-filter frame.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Fail-fast boundary validation: bounds must be in their canonical ranges
    /// (importance 1..=10, confidence 0.0..=1.0) and min/max / since/until
    /// pairs must not be inverted. An inverted range is a caller mistake, not
    /// an empty result set.
    pub fn validate(&self) -> crate::error::Result<()> {
        if let Some(v) = self.min_importance {
            crate::validate::validate_importance(v)?;
        }
        if let Some(v) = self.max_importance {
            crate::validate::validate_importance(v)?;
        }
        if let Some(v) = self.min_confidence {
            crate::validate::validate_confidence(v)?;
        }
        if let Some(v) = self.max_confidence {
            crate::validate::validate_confidence(v)?;
        }
        if let (Some(min), Some(max)) = (self.min_importance, self.max_importance) {
            if min > max {
                return Err(crate::error::Error::InvalidArgument(format!(
                    "min_importance {min} exceeds max_importance {max}"
                )));
            }
        }
        if let (Some(min), Some(max)) = (self.min_confidence, self.max_confidence) {
            if min > max {
                return Err(crate::error::Error::InvalidArgument(format!(
                    "min_confidence {min} exceeds max_confidence {max}"
                )));
            }
        }
        if let (Some(since), Some(until)) = (self.since, self.until) {
            if since > until {
                return Err(crate::error::Error::InvalidArgument(format!(
                    "since {since} is after until {until}"
                )));
            }
        }
        Ok(())
    }

    /// Whether `note` satisfies every metadata dimension of this filter:
    /// types (any-of), tags (all-of), importance/confidence ranges
    /// (inclusive), created-at window (inclusive), sources (any-of), and
    /// archived state. `contested` and `anchors` are intentionally NOT
    /// evaluated here — they require link/anchor lookups the note does not
    /// carry, so callers handle those dimensions where the data lives.
    #[must_use]
    pub fn matches(&self, note: &MemoryNote) -> bool {
        if !self.types.is_empty() && !self.types.contains(&note.memory_type) {
            return false;
        }
        if !self.tags.iter().all(|t| note.tags.contains(t)) {
            return false;
        }
        if self.min_importance.is_some_and(|min| note.importance < min) {
            return false;
        }
        if self.max_importance.is_some_and(|max| note.importance > max) {
            return false;
        }
        if self.min_confidence.is_some_and(|min| note.confidence < min) {
            return false;
        }
        if self.max_confidence.is_some_and(|max| note.confidence > max) {
            return false;
        }
        if self.since.is_some_and(|since| note.created_at < since) {
            return false;
        }
        if self.until.is_some_and(|until| note.created_at > until) {
            return false;
        }
        if !self.sources.is_empty()
            && !note
                .origin_source
                .as_ref()
                .is_some_and(|s| self.sources.contains(s))
        {
            return false;
        }
        self.state.admits_archived(note.archived_at.is_some())
    }

    /// Fold the pre-filter wire fields of a `Recall` request into this filter
    /// (daemon side). Any-of union for the type; deduped all-of union for
    /// tags. Idempotent when the client mirrored the same values.
    #[must_use]
    pub fn fold_recall_legacy(
        mut self,
        memory_type: Option<MemoryType>,
        tags: Vec<String>,
    ) -> Self {
        if let Some(t) = memory_type {
            if !self.types.contains(&t) {
                self.types.push(t);
            }
        }
        for tag in tags {
            if !self.tags.contains(&tag) {
                self.tags.push(tag);
            }
        }
        self
    }

    /// Fold the pre-filter `min_importance` wire field of a `List` request
    /// into this filter (daemon side); both are lower bounds so the stricter
    /// (larger) one wins.
    #[must_use]
    pub fn fold_list_legacy(mut self, min_importance: Option<u8>) -> Self {
        self.min_importance = match (self.min_importance, min_importance) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };
        self
    }

    /// Split out the dimensions a pre-filter `Recall` frame can express
    /// (client side): a SINGLE type and the tag list move to the legacy wire
    /// slots (old daemons still honor them), everything else stays in the
    /// returned filter. Inverse of [`RecallFilter::fold_recall_legacy`].
    #[must_use]
    pub fn split_recall_legacy(mut self) -> (Option<MemoryType>, Vec<String>, Self) {
        let legacy_type = if self.types.len() == 1 {
            self.types.pop()
        } else {
            None
        };
        let legacy_tags = std::mem::take(&mut self.tags);
        (legacy_type, legacy_tags, self)
    }

    /// Split out the `min_importance` a pre-filter `List` frame can express
    /// (client side). Inverse of [`RecallFilter::fold_list_legacy`].
    #[must_use]
    pub fn split_list_legacy(mut self) -> (Option<u8>, Self) {
        let legacy_min = self.min_importance.take();
        (legacy_min, self)
    }
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

    fn note_with(f: impl FnOnce(&mut MemoryNote)) -> MemoryNote {
        let mut note = MemoryNote::new(
            Namespace::Project("rusty-brain".into()),
            "content".to_string(),
            MemoryType::Insight,
            5,
        );
        f(&mut note);
        note
    }

    #[test]
    fn recall_filter_default_is_empty_and_matches_any_active_note() {
        let filter = RecallFilter::default();
        assert!(filter.is_empty());
        assert!(filter.validate().is_ok());
        assert!(filter.matches(&note_with(|_| {})));
    }

    #[test]
    fn recall_filter_default_serializes_to_an_empty_object() {
        // The additive-wire contract: an all-default filter must add NOTHING to
        // a frame (every field is skip-serialized), so requests without new
        // filters stay byte-identical to the pre-filter shape.
        let json = serde_json::to_value(RecallFilter::default()).unwrap();
        assert_eq!(json, serde_json::json!({}));
    }

    #[test]
    fn recall_filter_decodes_from_an_empty_object_to_default() {
        // Old frames carry no filter fields; `{}` (and per-field absence) must
        // decode to the unconstrained default.
        let back: RecallFilter = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(back, RecallFilter::default());
    }

    #[test]
    fn recall_filter_round_trips_every_field() {
        let since = chrono::Utc::now() - chrono::Duration::days(7);
        let until = chrono::Utc::now();
        let filter = RecallFilter {
            types: vec![MemoryType::BugFix, MemoryType::Insight],
            tags: vec!["sqlite".to_string()],
            min_importance: Some(3),
            max_importance: Some(9),
            min_confidence: Some(0.2),
            max_confidence: Some(0.9),
            since: Some(since),
            until: Some(until),
            sources: vec!["hook".to_string(), "cli".to_string()],
            contested: Some(true),
            state: MemoryState::All,
            anchors: vec![AnchorFilter {
                kind: AnchorKind::File,
                value: "src/server.rs".to_string(),
            }],
        };
        let json = serde_json::to_string(&filter).unwrap();
        let back: RecallFilter = serde_json::from_str(&json).unwrap();
        assert_eq!(back, filter);
        assert!(!back.is_empty());
    }

    #[test]
    fn memory_state_admits_archived_per_scope() {
        assert!(MemoryState::Active.admits_archived(false));
        assert!(!MemoryState::Active.admits_archived(true));
        assert!(!MemoryState::Archived.admits_archived(false));
        assert!(MemoryState::Archived.admits_archived(true));
        assert!(MemoryState::All.admits_archived(false));
        assert!(MemoryState::All.admits_archived(true));
        assert_eq!(MemoryState::default(), MemoryState::Active);
    }

    #[test]
    fn memory_state_serializes_snake_case() {
        assert_eq!(
            serde_json::to_value(MemoryState::Archived).unwrap(),
            serde_json::json!("archived")
        );
        assert_eq!(
            serde_json::to_value(MemoryState::All).unwrap(),
            serde_json::json!("all")
        );
    }

    #[test]
    fn matches_filters_by_type_any_of() {
        let filter = RecallFilter {
            types: vec![MemoryType::BugFix, MemoryType::Constraint],
            ..Default::default()
        };
        assert!(filter.matches(&note_with(|n| n.memory_type = MemoryType::BugFix)));
        assert!(filter.matches(&note_with(|n| n.memory_type = MemoryType::Constraint)));
        assert!(!filter.matches(&note_with(|n| n.memory_type = MemoryType::Insight)));
    }

    #[test]
    fn matches_requires_every_tag() {
        let filter = RecallFilter {
            tags: vec!["a".to_string(), "b".to_string()],
            ..Default::default()
        };
        assert!(filter.matches(&note_with(|n| {
            n.tags = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        })));
        assert!(!filter.matches(&note_with(|n| n.tags = vec!["a".to_string()])));
    }

    #[test]
    fn matches_filters_by_importance_range_inclusive() {
        let filter = RecallFilter {
            min_importance: Some(4),
            max_importance: Some(6),
            ..Default::default()
        };
        assert!(!filter.matches(&note_with(|n| n.importance = 3)));
        assert!(filter.matches(&note_with(|n| n.importance = 4)));
        assert!(filter.matches(&note_with(|n| n.importance = 6)));
        assert!(!filter.matches(&note_with(|n| n.importance = 7)));
    }

    #[test]
    fn matches_filters_by_confidence_range_inclusive() {
        let filter = RecallFilter {
            min_confidence: Some(0.5),
            max_confidence: Some(0.8),
            ..Default::default()
        };
        assert!(!filter.matches(&note_with(|n| n.confidence = 0.49)));
        assert!(filter.matches(&note_with(|n| n.confidence = 0.5)));
        assert!(filter.matches(&note_with(|n| n.confidence = 0.8)));
        assert!(!filter.matches(&note_with(|n| n.confidence = 0.81)));
    }

    #[test]
    fn matches_filters_by_created_at_window_inclusive() {
        let t0 = chrono::Utc::now();
        let filter = RecallFilter {
            since: Some(t0 - chrono::Duration::days(2)),
            until: Some(t0 - chrono::Duration::days(1)),
            ..Default::default()
        };
        assert!(!filter.matches(&note_with(|n| n.created_at = t0 - chrono::Duration::days(3))));
        assert!(filter.matches(&note_with(|n| n.created_at = t0 - chrono::Duration::days(2))));
        assert!(filter.matches(&note_with(|n| n.created_at = t0 - chrono::Duration::days(1))));
        assert!(!filter.matches(&note_with(|n| n.created_at = t0)));
    }

    #[test]
    fn matches_filters_by_source_any_of_and_missing_provenance_never_matches() {
        let filter = RecallFilter {
            sources: vec!["hook".to_string(), "mcp".to_string()],
            ..Default::default()
        };
        assert!(filter.matches(&note_with(|n| n.origin_source = Some("hook".to_string()))));
        assert!(filter.matches(&note_with(|n| n.origin_source = Some("mcp".to_string()))));
        assert!(!filter.matches(&note_with(|n| n.origin_source = Some("cli".to_string()))));
        // A pre-W0.5 row (no provenance) cannot satisfy a source constraint.
        assert!(!filter.matches(&note_with(|n| n.origin_source = None)));
    }

    #[test]
    fn matches_filters_by_archived_state() {
        let archived = note_with(|n| n.archived_at = Some(chrono::Utc::now()));
        let active = note_with(|_| {});

        let default_filter = RecallFilter::default();
        assert!(default_filter.matches(&active));
        assert!(
            !default_filter.matches(&archived),
            "default scope is active-only"
        );

        let archived_only = RecallFilter {
            state: MemoryState::Archived,
            ..Default::default()
        };
        assert!(archived_only.matches(&archived));
        assert!(!archived_only.matches(&active));

        let all = RecallFilter {
            state: MemoryState::All,
            ..Default::default()
        };
        assert!(all.matches(&archived));
        assert!(all.matches(&active));
    }

    #[test]
    fn matches_ignores_contested_and_anchors_dimensions() {
        // `contested` needs a link lookup and `anchors` needs the (PRD 4)
        // anchors table; neither is decidable from the note alone, so
        // `matches` must not reject on them — callers handle those dimensions.
        let filter = RecallFilter {
            contested: Some(true),
            ..Default::default()
        };
        assert!(filter.matches(&note_with(|_| {})));
    }

    #[test]
    fn validate_rejects_out_of_range_bounds() {
        for filter in [
            RecallFilter {
                min_importance: Some(0),
                ..Default::default()
            },
            RecallFilter {
                max_importance: Some(11),
                ..Default::default()
            },
            RecallFilter {
                min_confidence: Some(-0.1),
                ..Default::default()
            },
            RecallFilter {
                max_confidence: Some(1.5),
                ..Default::default()
            },
        ] {
            assert!(filter.validate().is_err(), "must reject {filter:?}");
        }
    }

    #[test]
    fn validate_rejects_inverted_ranges() {
        let t0 = chrono::Utc::now();
        for filter in [
            RecallFilter {
                min_importance: Some(8),
                max_importance: Some(3),
                ..Default::default()
            },
            RecallFilter {
                min_confidence: Some(0.9),
                max_confidence: Some(0.1),
                ..Default::default()
            },
            RecallFilter {
                since: Some(t0),
                until: Some(t0 - chrono::Duration::hours(1)),
                ..Default::default()
            },
        ] {
            assert!(filter.validate().is_err(), "must reject {filter:?}");
        }
    }

    #[test]
    fn fold_recall_legacy_unions_type_and_tags() {
        let filter = RecallFilter {
            types: vec![MemoryType::BugFix],
            tags: vec!["a".to_string()],
            ..Default::default()
        };
        let folded = filter.fold_recall_legacy(
            Some(MemoryType::Insight),
            vec!["a".to_string(), "b".to_string()],
        );
        assert_eq!(folded.types, vec![MemoryType::BugFix, MemoryType::Insight]);
        // Tags are all-of; the union adds `b` once and keeps `a` deduped.
        assert_eq!(folded.tags, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn fold_list_legacy_keeps_the_stricter_min_importance() {
        let filter = RecallFilter {
            min_importance: Some(3),
            ..Default::default()
        };
        assert_eq!(
            filter.clone().fold_list_legacy(Some(7)).min_importance,
            Some(7)
        );
        assert_eq!(filter.fold_list_legacy(None).min_importance, Some(3));
        assert_eq!(
            RecallFilter::default()
                .fold_list_legacy(Some(5))
                .min_importance,
            Some(5)
        );
    }

    #[test]
    fn split_recall_legacy_moves_expressible_filters_to_legacy_slots() {
        // One type + tags are expressible in the pre-filter wire fields, so the
        // client keeps sending them there (old daemons still honor them) and
        // the additive `filter` stays empty -> the frame stays byte-identical.
        let filter = RecallFilter {
            types: vec![MemoryType::BugFix],
            tags: vec!["a".to_string()],
            ..Default::default()
        };
        let (legacy_type, legacy_tags, rest) = filter.split_recall_legacy();
        assert_eq!(legacy_type, Some(MemoryType::BugFix));
        assert_eq!(legacy_tags, vec!["a".to_string()]);
        assert!(rest.is_empty());
    }

    #[test]
    fn split_recall_legacy_keeps_multi_type_and_new_dimensions_in_the_filter() {
        let filter = RecallFilter {
            types: vec![MemoryType::BugFix, MemoryType::Insight],
            min_confidence: Some(0.5),
            ..Default::default()
        };
        let (legacy_type, legacy_tags, rest) = filter.split_recall_legacy();
        assert_eq!(
            legacy_type, None,
            "any-of over two types has no legacy slot"
        );
        assert!(legacy_tags.is_empty());
        assert_eq!(rest.types, vec![MemoryType::BugFix, MemoryType::Insight]);
        assert_eq!(rest.min_confidence, Some(0.5));
    }

    #[test]
    fn split_list_legacy_moves_min_importance_to_the_legacy_slot() {
        let filter = RecallFilter {
            min_importance: Some(6),
            ..Default::default()
        };
        let (legacy_min, rest) = filter.split_list_legacy();
        assert_eq!(legacy_min, Some(6));
        assert!(rest.is_empty());
    }

    #[test]
    fn split_then_fold_round_trips_to_an_equivalent_filter() {
        let original = RecallFilter {
            types: vec![MemoryType::BugFix],
            tags: vec!["a".to_string(), "b".to_string()],
            min_importance: Some(4),
            sources: vec!["hook".to_string()],
            ..Default::default()
        };
        let (legacy_type, legacy_tags, rest) = original.clone().split_recall_legacy();
        assert_eq!(rest.fold_recall_legacy(legacy_type, legacy_tags), original);

        let (legacy_min, rest) = original.clone().split_list_legacy();
        assert_eq!(rest.fold_list_legacy(legacy_min), original);
    }

    #[test]
    fn anchor_filter_round_trips_with_snake_case_kind() {
        let anchor = AnchorFilter {
            kind: AnchorKind::Commit,
            value: "abc123".to_string(),
        };
        let json = serde_json::to_value(&anchor).unwrap();
        assert_eq!(json["kind"], "commit");
        assert_eq!(json["value"], "abc123");
        let back: AnchorFilter = serde_json::from_value(json).unwrap();
        assert_eq!(back, anchor);
        assert_eq!(
            serde_json::to_value(AnchorKind::File).unwrap(),
            serde_json::json!("file")
        );
        assert_eq!(
            serde_json::to_value(AnchorKind::Symbol).unwrap(),
            serde_json::json!("symbol")
        );
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
