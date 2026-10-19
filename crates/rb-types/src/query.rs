use crate::memory::MemoryNote;
use crate::memory_type::MemoryType;
use crate::namespace::Namespace;
use crate::trust_class::TrustClass;
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

/// Unweighted Linear channel scores before priors, confidence and source adjustment.
/// `None` means the channel did not surface the candidate; these are not final
/// score contributions (and are not RRF scores).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct ChannelScores {
    /// Reciprocal FTS rank: 1 / (1 + zero-based rank), not raw BM25.
    pub fts: Option<f32>,
    /// Cosine similarity: max(0, 1 - cosine distance), bounded to [0, 1].
    pub vector: Option<f32>,
    /// Reciprocal minimum graph hop count: 1 / max(1, hops).
    pub graph: Option<f32>,
}

/// A single ranked search hit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub memory: MemoryNote,
    pub score: f32,
    #[serde(default)]
    pub channels: ChannelHits,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_scores: Option<ChannelScores>,
    /// State-bound staleness (Vikunja #63): `true` when this memory is
    /// commit-bound and the repository state at query time has moved past
    /// the anchored commit (HEAD differs, or the worktree is dirty). A
    /// read-side annotation computed at recall time against the repo-state
    /// provider — never stored. `#[serde(default)]` (the `channels`
    /// precedent) keeps old frames decodable.
    #[serde(default)]
    pub stale: bool,
}

/// Current project navigation, validated against corpus and repository changes.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProjectContext {
    pub stable: Vec<ContextEntry>,
    pub snapshot: Option<crate::CorpusSnapshot>,
    pub repo_state: Option<crate::RepoSnapshot>,
}

/// Bounded navigation only. Full evidence and qualifications belong to get/history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextEntry {
    pub id: crate::MemoryId,
    pub memory_type: MemoryType,
    pub subject: String,
    pub origin_user: Option<String>,
    pub origin_agent: Option<String>,
    pub origin_source: Option<String>,
    pub origin_channel: Option<crate::WriteChannel>,
    pub trust_class: TrustClass,
    pub human_curated: bool,
    pub contested: bool,
    pub stale: bool,
    /// Full-evidence currentness key, excluding access metadata and relative age.
    pub fingerprint: u64,
}

impl ContextEntry {
    /// Reserve handle, safety-label and detail-pointer overhead within original budgets.
    pub const SUBJECT_CHARS: usize = 80;
    pub const SOURCE_CHARS: usize = 40;

    /// Currentness comparison without copying full evidence into a preview.
    /// SHA-256 over length-framed evidence, truncated to its first eight bytes (LE).
    /// Use the same repository snapshot as staleness evaluation; unresolved HEAD
    /// invalidates a commit-bound claim without marking unknown evidence stale.
    pub fn fingerprint_for(
        memory: &MemoryNote,
        stale: bool,
        repo: Option<&crate::RepoSnapshot>,
    ) -> u64 {
        use sha2::{Digest, Sha256};
        let mut hash = Sha256::new();
        hash.update(b"rusty-brain-context-fingerprint-v1");
        // Match embedding fingerprints: u64 LE counts and UTF-8 byte lengths.
        for fields in [
            std::slice::from_ref(&memory.content),
            std::slice::from_ref(&memory.context),
            std::slice::from_ref(&memory.summary),
            memory.keywords.as_slice(),
            memory.tags.as_slice(),
            memory.related_files.as_slice(),
        ] {
            hash.update((fields.len() as u64).to_le_bytes());
            for field in fields {
                hash.update((field.len() as u64).to_le_bytes());
                hash.update(field.as_bytes());
            }
        }
        for field in [memory.memory_type.as_str(), memory.trust_class.as_str()] {
            hash.update((field.len() as u64).to_le_bytes());
            hash.update(field.as_bytes());
        }
        hash.update([memory.importance]);
        hash.update(memory.confidence.to_bits().to_le_bytes());
        hash.update([
            u8::from(memory.human_curated),
            u8::from(memory.contested),
            u8::from(memory.legacy_unknown),
            u8::from(stale),
        ]);
        for field in [
            memory.origin_user.as_deref(),
            memory.origin_host.as_deref(),
            memory.origin_agent.as_deref(),
            memory.origin_source.as_deref(),
            memory.origin_channel.map(|channel| channel.as_str()),
            memory.session_id.as_deref(),
        ] {
            hash.update([u8::from(field.is_some())]);
            if let Some(field) = field {
                hash.update((field.len() as u64).to_le_bytes());
                hash.update(field.as_bytes());
            }
        }
        hash.update((memory.anchors.len() as u64).to_le_bytes());
        for anchor in &memory.anchors {
            for field in [crate::anchor_kind_str(anchor.kind), anchor.value.as_str()] {
                hash.update((field.len() as u64).to_le_bytes());
                hash.update(field.as_bytes());
            }
            for line in [anchor.start_line, anchor.end_line] {
                hash.update([u8::from(line.is_some())]);
                if let Some(line) = line {
                    hash.update(line.to_le_bytes());
                }
            }
            if anchor.kind == AnchorKind::Commit {
                // Unknown currentness is not stale, but invalidates earlier clean evidence.
                hash.update([u8::from(
                    repo.and_then(|state| state.head.as_ref()).is_some(),
                )]);
            }
        }
        let digest = hash.finalize();
        let mut bytes = [0_u8; 8];
        bytes.copy_from_slice(&digest[..8]);
        u64::from_le_bytes(bytes)
    }

    pub fn from_memory(
        memory: MemoryNote,
        stale: bool,
        repo: Option<&crate::RepoSnapshot>,
    ) -> Self {
        let fingerprint = Self::fingerprint_for(&memory, stale, repo);
        let subject = if memory.summary.trim().is_empty() {
            &memory.content
        } else {
            &memory.summary
        };
        let subject = subject
            .trim()
            .chars()
            .take(Self::SUBJECT_CHARS)
            .map(|ch| {
                if ch.is_control() || matches!(ch, '\u{2028}' | '\u{2029}') {
                    ' '
                } else {
                    ch
                }
            })
            .collect();
        let bounded =
            |text: Option<String>| text.map(|text| text.chars().take(Self::SOURCE_CHARS).collect());
        Self {
            id: memory.id,
            memory_type: memory.memory_type,
            subject,
            origin_user: bounded(memory.origin_user),
            origin_agent: bounded(memory.origin_agent),
            origin_source: bounded(memory.origin_source),
            origin_channel: memory.origin_channel,
            trust_class: memory.trust_class,
            human_curated: memory.human_curated,
            contested: memory.contested,
            stale,
            fingerprint,
        }
    }
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
/// The MODEL shipped with search-filter parity (PRD 2026-07-02); evaluation
/// shipped with typed code anchors: the value is compared against a memory's
/// [`crate::MemoryAnchor`]s under the same normalization
/// ([`crate::normalize_anchor_value`]) on both sides, so `./src/a.rs` and
/// `src/a.rs` are the same file anchor. Multiple anchor filters compose
/// all-of (like tags): every listed anchor must match. File filters match by
/// PATH only — a stored line range never narrows a filter in v1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnchorFilter {
    pub kind: AnchorKind,
    pub value: String,
}

impl AnchorFilter {
    /// Whether `anchor` satisfies this constraint: same kind, same normalized
    /// value (file line ranges are ignored — path-level matching in v1).
    #[must_use]
    pub fn matches_anchor(&self, anchor: &crate::MemoryAnchor) -> bool {
        self.kind == anchor.kind
            && crate::normalize_anchor_value(self.kind, &self.value)
                == crate::normalize_anchor_value(anchor.kind, &anchor.value)
    }

    /// Fail-fast boundary validation: the value must be non-empty after
    /// normalization (an empty anchor filter can never match and is a caller
    /// mistake, not an empty result set).
    pub fn validate(&self) -> crate::error::Result<()> {
        if crate::normalize_anchor_value(self.kind, &self.value).is_empty() {
            return Err(crate::error::Error::InvalidArgument(format!(
                "{} anchor filter value must not be empty",
                crate::anchor_kind_str(self.kind)
            )));
        }
        Ok(())
    }
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
/// Semantics: `types`/`sources` are any-of; `tags` and `anchors` are all-of;
/// numeric and time ranges are inclusive; `contested` is tri-state (`None` =
/// no constraint); `state` defaults to active-only. `contested` is NOT
/// decided by [`RecallFilter::matches`] (it needs a link lookup) — callers
/// evaluate that dimension where the data lives. `anchors` IS decided by
/// `matches` (the note carries its anchors since typed code anchors).
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
    /// Restrict to any of these trust classes (Vikunja #63 trust ladder;
    /// empty = no constraint). Any-of, like `types`/`sources`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trust_classes: Vec<TrustClass>,
    /// Code-anchor constraints (see [`AnchorFilter`]; all-of, like `tags`):
    /// wire plumbing shipped with search-filter parity; evaluation shipped
    /// with typed code anchors (PRD 2026-07-02).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub anchors: Vec<AnchorFilter>,
    /// Internal injection eligibility, never accepted from a wire filter.
    /// Audit list/get deliberately retain quarantined and exhausted rows.
    #[serde(skip)]
    pub recall_eligible: bool,
    /// Recording-time point-in-time view (Vikunja #102), honored by `list`
    /// ONLY: membership becomes
    /// `created_at <= T AND (archived_at IS NULL OR archived_at > T)` and
    /// REPLACES the `state` predicate (so `state` must stay `Active`).
    /// Whole seconds only; ranked recall rejects it (archived rows have no
    /// vectors). Results order by `(created_at, memory_id)` ascending. A
    /// pre-`as_of` daemon would drop this field and answer an active-only
    /// list, so clients gate it on the `as_of` handshake capability.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub as_of: Option<chrono::DateTime<chrono::Utc>>,
    /// Keyset cursor continuing an `as_of` list: only rows strictly after
    /// this `(created_at, memory_id)` pair. Requires `as_of`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub as_of_after: Option<crate::AsOfCursor>,
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
        for anchor in &self.anchors {
            anchor.validate()?;
        }
        self.validate_as_of()
    }

    /// The `as_of` rules (Vikunja #102): `as_of` replaces the state predicate,
    /// and because `state` has no presence bit on the wire it must be the
    /// default `Active` (which is then ignored); `Archived`/`All` are
    /// errors. Instants are whole seconds — a sub-second value is rejected
    /// rather than silently truncated. A cursor needs an `as_of`.
    fn validate_as_of(&self) -> crate::error::Result<()> {
        let whole_second = |what: &str, t: chrono::DateTime<chrono::Utc>| {
            if t.timestamp_subsec_nanos() == 0 {
                Ok(())
            } else {
                Err(crate::error::Error::InvalidArgument(format!(
                    "{what} {t} has sub-second precision; as_of is exact to whole \
                     seconds (precision: second)"
                )))
            }
        };
        let Some(as_of) = self.as_of else {
            if self.as_of_after.is_some() {
                return Err(crate::error::Error::InvalidArgument(
                    "as_of_after is a cursor for an as_of list and requires as_of".to_string(),
                ));
            }
            return Ok(());
        };
        if !self.state.is_active() {
            return Err(crate::error::Error::InvalidArgument(
                "as_of replaces the archived-state filter: it cannot be combined with \
                 state archived/all (or --archived)"
                    .to_string(),
            ));
        }
        whole_second("as_of", as_of)?;
        if let Some(cursor) = &self.as_of_after {
            whole_second("as_of_after.created_at", cursor.created_at)?;
        }
        Ok(())
    }

    /// Whether `note` satisfies every note-decidable dimension of this filter:
    /// types (any-of), tags (all-of), importance/confidence ranges
    /// (inclusive), created-at window (inclusive), sources (any-of), trust
    /// classes (any-of, Vikunja #63), archived state, and anchors (all-of
    /// over `note.anchors`, normalized on both sides — kept in lockstep with
    /// the store's SQL by the `anchor_filter_agrees_with_recall_filter_matches`
    /// drift test).
    /// `contested` is intentionally NOT evaluated here — it requires a link
    /// lookup the note does not carry, so callers handle it where the data
    /// lives.
    #[must_use]
    pub fn matches(&self, note: &MemoryNote) -> bool {
        if self.recall_eligible && (note.is_quarantined() || note.below_recall_confidence_floor()) {
            return false;
        }
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
        // Any-of, like types/sources: an empty list constrains nothing.
        if !self.trust_classes.is_empty() && !self.trust_classes.contains(&note.trust_class) {
            return false;
        }
        // All-of (like tags): every anchor constraint must be satisfied by
        // at least one of the note's anchors.
        if !self
            .anchors
            .iter()
            .all(|af| note.anchors.iter().any(|a| af.matches_anchor(a)))
        {
            return false;
        }
        if let Some(as_of) = self.as_of {
            // `as_of` REPLACES the state predicate (Vikunja #102).
            return crate::as_of::member_as_of(note.created_at, note.archived_at, as_of)
                && self
                    .as_of_after
                    .as_ref()
                    .is_none_or(|c| c.admits(note.created_at, &note.id));
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
    /// Explicit curation intent for deliberate user updates; `None` preserves
    /// the current marker, while `Some(false)` deliberately removes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub human_curated: Option<bool>,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::memory::MemoryNote;
    use crate::memory_type::MemoryType;
    use crate::namespace::Namespace;

    fn context_fingerprint_note() -> MemoryNote {
        let mut note = MemoryNote::new(
            Namespace::Global,
            "Use SQLite.".into(),
            MemoryType::ArchitectureDecision,
            8,
        );
        note.context = "UNTESTED; revalidate before use.".into();
        note.summary = "Use SQLite.".into();
        note.keywords = vec!["sqlite".into(), "storage".into()];
        note.tags = vec!["decision".into()];
        note.related_files = vec!["src/db.rs".into()];
        note.confidence = 0.75;
        note.trust_class = TrustClass::HumanConfirmed;
        note.human_curated = true;
        note.contested = true;
        note.origin_user = Some("Ada".into());
        note.origin_agent = Some("omp".into());
        note.origin_source = Some("cli".into());
        note.origin_channel = Some(crate::WriteChannel::Cli);
        note.session_id = Some(String::new());
        note.anchors = vec![
            crate::MemoryAnchor::parse_file_spec("src/db.rs:12-18").unwrap(),
            crate::MemoryAnchor::new(AnchorKind::Symbol, "Db::open").unwrap(),
        ];
        note
    }

    #[test]
    fn context_fingerprint_has_a_portable_serialized_fixture() {
        let mut note = context_fingerprint_note();
        // Independently framed with Python hashlib.sha256: 30c4c9831380d233
        // 57525d424ee2cb3e881b389b467fd852b190195638d333f9; first 8 bytes LE.
        let expected = 3_734_187_862_347_334_704;
        let entry = ContextEntry::from_memory(note.clone(), true, None);
        assert_eq!(entry.fingerprint, expected);
        let serialized = serde_json::to_string(&entry).unwrap();
        let restored: ContextEntry = serde_json::from_str(&serialized).unwrap();
        assert_eq!(restored.fingerprint, expected);
        note.access_count += 1;
        note.last_accessed_at = Some(note.created_at);
        note.created_at -= chrono::Duration::days(30);
        note.updated_at = note.created_at;
        assert_eq!(ContextEntry::fingerprint_for(&note, true, None), expected);
        assert_ne!(ContextEntry::fingerprint_for(&note, false, None), expected);
    }

    #[test]
    fn context_fingerprint_retains_full_evidence_and_qualifications() {
        let note = context_fingerprint_note();
        let expected = ContextEntry::fingerprint_for(&note, true, None);
        let mutations: [fn(&mut MemoryNote); 23] = [
            |n| n.content.push_str(" Exception: read-only."),
            |n| n.context.push_str(" Outcome unknown."),
            |n| n.summary.push_str(" (proposal)"),
            |n| n.keywords.push("caveat".into()),
            |n| n.tags.push("untested".into()),
            |n| n.related_files.push("src/legacy.rs".into()),
            |n| n.memory_type = MemoryType::Constraint,
            |n| n.importance = 9,
            |n| n.confidence = 0.5,
            |n| n.trust_class = TrustClass::AgentAttested,
            |n| n.human_curated = false,
            |n| n.contested = false,
            |n| n.legacy_unknown = true,
            |n| n.origin_user = Some("Grace".into()),
            |n| n.origin_host = Some("titan".into()),
            |n| n.origin_agent = Some("other".into()),
            |n| n.origin_source = Some("hook".into()),
            |n| n.origin_channel = Some(crate::WriteChannel::Hook),
            |n| n.session_id = None,
            |n| n.anchors[0].kind = AnchorKind::Symbol,
            |n| n.anchors[0].value.push_str(".old"),
            |n| n.anchors[0].start_line = Some(13),
            |n| n.anchors[0].end_line = Some(19),
        ];
        for (index, mutate) in mutations.into_iter().enumerate() {
            let mut changed = note.clone();
            mutate(&mut changed);
            assert_ne!(
                ContextEntry::fingerprint_for(&changed, true, None),
                expected,
                "evidence mutation {index}"
            );
        }
    }

    #[test]
    fn context_fingerprint_frames_lists_options_and_anchor_ranges() {
        let mut left = context_fingerprint_note();
        let mut right = left.clone();
        let distinct = |left: &MemoryNote, right: &MemoryNote| {
            assert_ne!(
                ContextEntry::fingerprint_for(left, true, None),
                ContextEntry::fingerprint_for(right, true, None)
            );
        };
        left.keywords = vec!["ab".into(), "c".into()];
        right.keywords = vec!["a".into(), "bc".into()];
        distinct(&left, &right);
        right.keywords = vec!["ab".into(), "c".into(), String::new()];
        distinct(&left, &right);
        right = left.clone();
        right.origin_host = Some(String::new());
        distinct(&left, &right);
        right = left.clone();
        right.anchors.reverse();
        distinct(&left, &right);
        right = left.clone();
        right.anchors.push(right.anchors[0].clone());
        distinct(&left, &right);
        right = left.clone();
        left.anchors[0].start_line = None;
        right.anchors[0].start_line = Some(0);
        distinct(&left, &right);
        right.anchors[0].start_line = None;
        left.anchors[0].end_line = None;
        right.anchors[0].end_line = Some(0);
        distinct(&left, &right);
    }

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
            stale: false,
            channel_scores: None,
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
            stale: false,
            channel_scores: None,
        })
        .unwrap();
        value.as_object_mut().unwrap().remove("channels").unwrap();
        value.as_object_mut().unwrap().remove("channel_scores");
        let back: SearchResult = serde_json::from_value(value).unwrap();
        assert_eq!(back.channels, ChannelHits::default());
        assert_eq!(back.channel_scores, None);
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
            human_curated: Some(true),
        };
        let json = serde_json::to_string(&u).unwrap();
        let back: MemoryUpdates = serde_json::from_str(&json).unwrap();
        assert_eq!(back.content, Some("new body".to_string()));
        assert_eq!(back.summary, Some("new summary".to_string()));
        assert_eq!(back.importance, Some(9));
        assert_eq!(back.tags, Some(vec!["x".to_string(), "y".to_string()]));
        assert_eq!(back.context, Some("ctx".to_string()));
        assert_eq!(back.confidence, Some(0.4));
        assert_eq!(back.human_curated, Some(true));
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
            trust_classes: vec![TrustClass::MeasuredCi, TrustClass::AgentAttested],
            anchors: vec![AnchorFilter {
                kind: AnchorKind::File,
                value: "src/server.rs".to_string(),
            }],
            recall_eligible: false,
            as_of: Some(chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap()),
            as_of_after: Some(crate::AsOfCursor {
                created_at: chrono::DateTime::from_timestamp(1_600_000_000, 0).unwrap(),
                memory_id: crate::MemoryId::new(),
            }),
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
    fn matches_ignores_the_contested_dimension() {
        // `contested` needs a link lookup the note does not carry, so
        // `matches` must not reject on it — callers handle that dimension.
        let filter = RecallFilter {
            contested: Some(true),
            ..Default::default()
        };
        assert!(filter.matches(&note_with(|_| {})));
    }

    fn anchor_filter(kind: AnchorKind, value: &str) -> RecallFilter {
        RecallFilter {
            anchors: vec![AnchorFilter {
                kind,
                value: value.to_string(),
            }],
            ..Default::default()
        }
    }

    #[test]
    fn matches_filters_by_anchor_per_kind() {
        let anchored = note_with(|n| {
            n.anchors = vec![
                crate::MemoryAnchor::parse_file_spec("src/server.rs:10-20").unwrap(),
                crate::MemoryAnchor::new(AnchorKind::Commit, "abc123").unwrap(),
                crate::MemoryAnchor::new(AnchorKind::Symbol, "Engine::recall").unwrap(),
            ];
        });
        let bare = note_with(|_| {});

        // Acceptance criterion (PRD): anchored memory matches its file filter
        // and is absent under a different file.
        assert!(anchor_filter(AnchorKind::File, "src/server.rs").matches(&anchored));
        assert!(!anchor_filter(AnchorKind::File, "src/other.rs").matches(&anchored));
        assert!(!anchor_filter(AnchorKind::File, "src/server.rs").matches(&bare));

        assert!(anchor_filter(AnchorKind::Commit, "abc123").matches(&anchored));
        assert!(!anchor_filter(AnchorKind::Commit, "def456").matches(&anchored));
        assert!(anchor_filter(AnchorKind::Symbol, "Engine::recall").matches(&anchored));

        // Kinds never cross-match, even on an equal value.
        assert!(!anchor_filter(AnchorKind::Symbol, "abc123").matches(&anchored));
    }

    #[test]
    fn matches_normalizes_anchor_values_on_both_sides() {
        // `./src/a.rs` (filter) matches `src/a.rs` (stored) and vice versa;
        // a stored line range never narrows a path-level filter (v1).
        let anchored = note_with(|n| {
            n.anchors = vec![crate::MemoryAnchor::parse_file_spec("./src/a.rs:5").unwrap()];
        });
        assert!(anchor_filter(AnchorKind::File, "src/a.rs").matches(&anchored));
        assert!(anchor_filter(AnchorKind::File, " ./src/a.rs ").matches(&anchored));
    }

    #[test]
    fn matches_requires_every_anchor_filter() {
        // All-of composition (like tags): both constraints must be satisfied.
        let filter = RecallFilter {
            anchors: vec![
                AnchorFilter {
                    kind: AnchorKind::File,
                    value: "src/a.rs".to_string(),
                },
                AnchorFilter {
                    kind: AnchorKind::Symbol,
                    value: "Foo::bar".to_string(),
                },
            ],
            ..Default::default()
        };
        let both = note_with(|n| {
            n.anchors = vec![
                crate::MemoryAnchor::new(AnchorKind::File, "src/a.rs").unwrap(),
                crate::MemoryAnchor::new(AnchorKind::Symbol, "Foo::bar").unwrap(),
            ];
        });
        let file_only = note_with(|n| {
            n.anchors = vec![crate::MemoryAnchor::new(AnchorKind::File, "src/a.rs").unwrap()];
        });
        assert!(filter.matches(&both));
        assert!(!filter.matches(&file_only));
    }

    #[test]
    fn validate_rejects_empty_anchor_filter_values() {
        for value in ["", "   ", "./"] {
            let filter = anchor_filter(AnchorKind::File, value);
            assert!(
                filter.validate().is_err(),
                "empty-after-normalization anchor value {value:?} must be rejected"
            );
        }
        assert!(anchor_filter(AnchorKind::File, "src/a.rs")
            .validate()
            .is_ok());
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

    #[test]
    fn recall_filter_matches_by_trust_class_any_of() {
        let note = |class: TrustClass| {
            let mut n = MemoryNote::new(
                Namespace::Global,
                "content".to_string(),
                MemoryType::Insight,
                5,
            );
            n.trust_class = class;
            n
        };
        let filter = RecallFilter {
            trust_classes: vec![TrustClass::MeasuredCi, TrustClass::HumanConfirmed],
            ..Default::default()
        };
        assert!(filter.matches(&note(TrustClass::MeasuredCi)));
        assert!(filter.matches(&note(TrustClass::HumanConfirmed)));
        assert!(!filter.matches(&note(TrustClass::MeasuredLocal)));
        assert!(!filter.matches(&note(TrustClass::AgentAttested)));
        // Empty list = no constraint (the historical behavior).
        let unconstrained = RecallFilter::default();
        for class in TrustClass::all() {
            assert!(unconstrained.matches(&note(class)), "{class:?}");
        }
    }

    #[test]
    fn recall_filter_wire_carries_trust_classes_snake_case() {
        let filter = RecallFilter {
            trust_classes: vec![TrustClass::MeasuredLocal],
            ..Default::default()
        };
        let json = serde_json::to_string(&filter).unwrap();
        assert_eq!(json, r#"{"trust_classes":["measured_local"]}"#);
        let back: RecallFilter = serde_json::from_str(&json).unwrap();
        assert_eq!(back, filter);
    }

    #[test]
    fn memory_note_decodes_without_trust_class_to_agent_attested() {
        // Wire compat: a pre-ladder frame carries no trust_class key and must
        // decode to the backfill default (migration 012 agreement).
        let mut value = serde_json::to_value(MemoryNote::new(
            Namespace::Global,
            "content".to_string(),
            MemoryType::Insight,
            5,
        ))
        .unwrap();
        assert!(value
            .as_object_mut()
            .unwrap()
            .remove("trust_class")
            .is_some());
        let back: MemoryNote = serde_json::from_value(value).unwrap();
        assert_eq!(back.trust_class, TrustClass::AgentAttested);
    }
}
