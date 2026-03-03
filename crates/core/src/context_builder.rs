//! Token-budgeted context assembly for agent session startup.
//!
//! Combines recent observations, query-relevant memories, and session
//! summaries into an [`InjectedContext`](types::InjectedContext)
//! payload bounded by a configurable token budget.

use std::collections::HashSet;

use crate::backend::MemvidBackend;
use crate::token::estimate_tokens;
use chrono::{DateTime, Utc};
use types::{
    InjectedContext, MindConfig, Observation, ObservationMetadata, ObservationType,
    RustyBrainError, SessionSummary,
};

/// Maximum number of relevant memories to retrieve when a query is provided.
const MAX_RELEVANT_MEMORIES: usize = 10;

/// Maximum number of session summaries to include.
const MAX_SESSION_SUMMARIES: usize = 5;

/// Build an [`InjectedContext`] from backend queries within token budget.
///
/// Steps:
/// 1. Get recent observations from timeline (newest-first, `max_context_observations`)
/// 2. Enrich with `frame_by_id` for full metadata, reconstruct `Observation`
/// 3. If query provided: get relevant memories from `find`
/// 4. Get session summaries from `find` (tagged `"session_summary"`)
/// 5. Apply token budget, truncating content if needed
///
/// # Errors
///
/// Returns `RustyBrainError::Storage` if any backend operation fails.
pub(crate) fn build(
    backend: &dyn MemvidBackend,
    config: &MindConfig,
    query: Option<&str>,
) -> Result<InjectedContext, RustyBrainError> {
    let budget = config.max_context_tokens as usize;
    let mut tokens_used: usize = 0;

    // 1. Recent observations (highest priority, newest-first).
    let timeline = backend.timeline(config.max_context_observations as usize, true)?;
    let mut recent_observations = Vec::new();

    for entry in &timeline {
        if tokens_used >= budget {
            break;
        }
        let frame = backend.frame_by_id(entry.frame_id)?;
        if let Some(obs) = parse_observation_from_metadata(&frame.metadata) {
            let obs_tokens = estimate_item_tokens(&obs);
            if tokens_used + obs_tokens > budget {
                // Try fitting with truncated content.
                if let Some(truncated) = truncate_observation(obs, budget - tokens_used) {
                    tokens_used += estimate_item_tokens(&truncated);
                    recent_observations.push(truncated);
                }
                break;
            }
            tokens_used += obs_tokens;
            recent_observations.push(obs);
        }
    }

    // 2. Relevant memories (only when query is provided, deduplicated).
    let mut relevant_memories = Vec::new();
    if let Some(q) = query {
        let hits = backend.find(q, MAX_RELEVANT_MEMORIES)?;
        let recent_ids: HashSet<_> = recent_observations.iter().map(|o| o.id).collect();

        for hit in &hits {
            if tokens_used >= budget {
                break;
            }
            if let Some(obs) = parse_observation_from_metadata(&hit.metadata) {
                if recent_ids.contains(&obs.id) {
                    continue;
                }
                let obs_tokens = estimate_item_tokens(&obs);
                if tokens_used + obs_tokens > budget {
                    continue;
                }
                tokens_used += obs_tokens;
                relevant_memories.push(obs);
            }
        }
    }

    // 3. Session summaries (lowest priority).
    // Use a larger intermediate limit to account for non-tagged false positives,
    // then take only the first MAX_SESSION_SUMMARIES tagged hits.
    let mut session_summaries = Vec::new();
    let summary_hits = backend.find("session_summary", MAX_SESSION_SUMMARIES * 3)?;

    for hit in &summary_hits {
        if session_summaries.len() >= MAX_SESSION_SUMMARIES || tokens_used >= budget {
            break;
        }
        // Only include hits actually tagged as session summaries.
        if !hit.tags.iter().any(|t| t == "session_summary") {
            continue;
        }
        if let Some(summary) = parse_session_summary_from_metadata(&hit.metadata) {
            let summary_tokens = estimate_summary_tokens(&summary);
            if tokens_used + summary_tokens > budget {
                continue;
            }
            tokens_used += summary_tokens;
            session_summaries.push(summary);
        }
    }

    Ok(InjectedContext {
        recent_observations,
        relevant_memories,
        session_summaries,
        token_count: tokens_used as u64,
    })
}

/// Parse an [`Observation`] from the metadata JSON stored by `Mind::remember`.
fn parse_observation_from_metadata(meta: &serde_json::Value) -> Option<Observation> {
    let id_str = meta.get("id")?.as_str()?;
    let id = ulid::Ulid::from_string(&id_str.to_uppercase()).ok()?;

    let timestamp = meta
        .get("timestamp")?
        .as_str()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc))?;

    let obs_type_str = meta.get("obs_type").and_then(|v| v.as_str())?;
    let obs_type: ObservationType = obs_type_str.parse().ok()?;

    let tool_name = meta
        .get("tool_name")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())?
        .to_string();
    let summary = meta
        .get("summary")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())?
        .to_string();
    let content = meta
        .get("content")
        .and_then(|v| v.as_str())
        .map(String::from);

    let obs_metadata: Option<ObservationMetadata> = meta
        .get("metadata")
        .filter(|v| !v.is_null())
        .and_then(|v| serde_json::from_value(v.clone()).ok());

    Some(Observation {
        id,
        timestamp,
        obs_type,
        tool_name,
        summary,
        content,
        metadata: obs_metadata,
    })
}

/// Parse a [`SessionSummary`] from the metadata JSON of a `session_summary` observation.
///
/// The `"content"` field is expected to contain a JSON-serialized [`SessionSummary`].
fn parse_session_summary_from_metadata(meta: &serde_json::Value) -> Option<SessionSummary> {
    let content = meta.get("content")?.as_str()?;
    serde_json::from_str(content).ok()
}

/// Estimate the token count of a serialized `Observation`.
fn estimate_item_tokens(obs: &Observation) -> usize {
    let json = serde_json::to_string(obs).unwrap_or_default();
    estimate_tokens(&json)
}

/// Estimate the token count of a serialized `SessionSummary`.
fn estimate_summary_tokens(summary: &SessionSummary) -> usize {
    let json = serde_json::to_string(summary).unwrap_or_default();
    estimate_tokens(&json)
}

/// Try to truncate an observation's content to fit within a token budget.
///
/// Returns `None` if the observation won't fit even without content.
fn truncate_observation(mut obs: Observation, remaining_tokens: usize) -> Option<Observation> {
    // Check if it already fits.
    if estimate_item_tokens(&obs) <= remaining_tokens {
        return Some(obs);
    }

    // Check if it fits without content.
    let mut no_content = obs.clone();
    no_content.content = None;
    let base_tokens = estimate_item_tokens(&no_content);

    if base_tokens > remaining_tokens {
        return None;
    }

    let content = obs.content.take()?;

    // Calculate how many content bytes we can keep.
    // Subtract a buffer for JSON serialization overhead ("..." suffix, quotes).
    let available_bytes = (remaining_tokens - base_tokens).saturating_mul(4);
    let safe_bytes = available_bytes.saturating_sub(20);

    if safe_bytes == 0 {
        obs.content = None;
    } else {
        // Safe UTF-8 truncation at a char boundary.
        let end = truncate_at_char_boundary(&content, safe_bytes);
        obs.content = if end == 0 {
            None
        } else {
            Some(format!("{}...", &content[..end]))
        };
    }

    // Final check — remove content entirely if still over budget.
    if estimate_item_tokens(&obs) > remaining_tokens {
        obs.content = None;
    }

    Some(obs)
}

/// Find the largest byte index `<= max_bytes` that is a valid UTF-8 char boundary.
fn truncate_at_char_boundary(s: &str, max_bytes: usize) -> usize {
    if max_bytes >= s.len() {
        return s.len();
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    end
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::MockBackend;
    use std::path::Path;

    /// Store an observation in `MockBackend` using the same format as `Mind::remember`.
    fn store_observation(
        backend: &MockBackend,
        obs_type: ObservationType,
        tool_name: &str,
        summary: &str,
        content: Option<&str>,
    ) {
        let id = ulid::Ulid::new().to_string().to_lowercase();
        let timestamp = chrono::Utc::now();

        let payload = match content {
            Some(c) => format!("{summary}\n\n{c}"),
            None => summary.to_string(),
        };

        let meta = serde_json::json!({
            "id": id,
            "obs_type": obs_type.to_string(),
            "tool_name": tool_name,
            "summary": summary,
            "content": content,
            "timestamp": timestamp.to_rfc3339(),
            "session_id": "test-session",
            "metadata": null,
        });

        let labels = vec![obs_type.to_string()];
        let tags = vec![tool_name.to_string(), "test-session".to_string()];

        backend
            .put(payload.as_bytes(), &labels, &tags, &meta)
            .unwrap();
        backend.commit().unwrap();
    }

    /// Store a session summary in `MockBackend`.
    fn store_session_summary(backend: &MockBackend, summary_text: &str) {
        let session_summary = SessionSummary::new(
            "test-session".to_string(),
            chrono::Utc::now(),
            chrono::Utc::now(),
            5,
            vec!["decision1".to_string()],
            vec!["file1.rs".to_string()],
            summary_text.to_string(),
        )
        .unwrap();

        let content_json = serde_json::to_string(&session_summary).unwrap();
        let payload = format!("session_summary\n\n{content_json}");

        let meta = serde_json::json!({
            "id": ulid::Ulid::new().to_string().to_lowercase(),
            "obs_type": "decision",
            "tool_name": "system",
            "summary": "Session summary",
            "content": content_json,
            "timestamp": chrono::Utc::now().to_rfc3339(),
            "session_id": "test-session",
            "metadata": null,
        });

        let labels = vec!["decision".to_string()];
        let tags = vec![
            "system".to_string(),
            "test-session".to_string(),
            "session_summary".to_string(),
        ];

        backend
            .put(payload.as_bytes(), &labels, &tags, &meta)
            .unwrap();
        backend.commit().unwrap();
    }

    fn default_config() -> MindConfig {
        MindConfig {
            max_context_observations: 20,
            max_context_tokens: 2000,
            ..MindConfig::default()
        }
    }

    // =========================================================================
    // T036: Recent observations
    // =========================================================================

    #[test]
    fn build_includes_recent_observations() {
        let backend = MockBackend::new();
        backend.create(Path::new("/tmp/test.mv2")).unwrap();

        store_observation(
            &backend,
            ObservationType::Discovery,
            "Read",
            "Found caching pattern",
            Some("LRU cache"),
        );
        store_observation(
            &backend,
            ObservationType::Decision,
            "Write",
            "Chose async approach",
            None,
        );

        let ctx = build(&backend, &default_config(), None).unwrap();
        assert_eq!(ctx.recent_observations.len(), 2);
    }

    #[test]
    fn build_recent_observations_newest_first() {
        let backend = MockBackend::new();
        backend.create(Path::new("/tmp/test.mv2")).unwrap();

        store_observation(
            &backend,
            ObservationType::Discovery,
            "Read",
            "First observation",
            None,
        );
        store_observation(
            &backend,
            ObservationType::Decision,
            "Write",
            "Second observation",
            None,
        );

        let ctx = build(&backend, &default_config(), None).unwrap();
        assert_eq!(ctx.recent_observations.len(), 2);
        // Newest first (reversed timeline).
        assert_eq!(ctx.recent_observations[0].summary, "Second observation");
        assert_eq!(ctx.recent_observations[1].summary, "First observation");
    }

    #[test]
    fn build_caps_recent_at_max_context_observations() {
        let backend = MockBackend::new();
        backend.create(Path::new("/tmp/test.mv2")).unwrap();

        for i in 0..5 {
            store_observation(
                &backend,
                ObservationType::Discovery,
                "Read",
                &format!("Observation {i}"),
                None,
            );
        }

        let config = MindConfig {
            max_context_observations: 3,
            max_context_tokens: 10000,
            ..MindConfig::default()
        };

        let ctx = build(&backend, &config, None).unwrap();
        assert!(
            ctx.recent_observations.len() <= 3,
            "should cap at max_context_observations"
        );
    }

    // =========================================================================
    // T037: Relevant memories with query
    // =========================================================================

    #[test]
    fn build_includes_relevant_memories_when_query_provided() {
        let backend = MockBackend::new();
        backend.create(Path::new("/tmp/test.mv2")).unwrap();

        store_observation(
            &backend,
            ObservationType::Discovery,
            "Read",
            "Found caching pattern",
            Some("LRU cache"),
        );
        store_observation(
            &backend,
            ObservationType::Decision,
            "Write",
            "Chose async approach",
            None,
        );
        store_observation(
            &backend,
            ObservationType::Success,
            "Bash",
            "Completed setup",
            None,
        );

        // Small max_context_observations: only the newest is in recent.
        let config = MindConfig {
            max_context_observations: 1,
            max_context_tokens: 10000,
            ..MindConfig::default()
        };

        let ctx = build(&backend, &config, Some("caching")).unwrap();
        assert!(
            !ctx.relevant_memories.is_empty(),
            "should find relevant memories for query"
        );
        assert_eq!(ctx.relevant_memories[0].summary, "Found caching pattern");
    }

    #[test]
    fn build_omits_relevant_memories_when_no_query() {
        let backend = MockBackend::new();
        backend.create(Path::new("/tmp/test.mv2")).unwrap();

        store_observation(
            &backend,
            ObservationType::Discovery,
            "Read",
            "Found caching pattern",
            Some("LRU cache"),
        );

        let ctx = build(&backend, &default_config(), None).unwrap();
        assert!(
            ctx.relevant_memories.is_empty(),
            "should have no relevant memories without query"
        );
    }

    #[test]
    fn build_deduplicates_relevant_against_recent() {
        let backend = MockBackend::new();
        backend.create(Path::new("/tmp/test.mv2")).unwrap();

        store_observation(
            &backend,
            ObservationType::Discovery,
            "Read",
            "Found caching pattern",
            Some("LRU cache"),
        );

        // Same observation in both recent and relevant — should not duplicate.
        let config = MindConfig {
            max_context_observations: 10,
            max_context_tokens: 10000,
            ..MindConfig::default()
        };

        let ctx = build(&backend, &config, Some("caching")).unwrap();
        assert_eq!(ctx.recent_observations.len(), 1);
        // Relevant should be empty because the only match is already in recent.
        assert!(
            ctx.relevant_memories.is_empty(),
            "dedup should prevent observation from appearing in both lists"
        );
    }

    // =========================================================================
    // T038: Session summaries
    // =========================================================================

    #[test]
    fn build_includes_session_summaries() {
        let backend = MockBackend::new();
        backend.create(Path::new("/tmp/test.mv2")).unwrap();

        store_session_summary(&backend, "Productive session implementing types");

        let ctx = build(&backend, &default_config(), None).unwrap();
        assert!(
            !ctx.session_summaries.is_empty(),
            "should include session summaries"
        );
        assert_eq!(
            ctx.session_summaries[0].summary,
            "Productive session implementing types"
        );
    }

    #[test]
    fn build_excludes_non_summary_observations_from_summaries() {
        let backend = MockBackend::new();
        backend.create(Path::new("/tmp/test.mv2")).unwrap();

        // Store a regular observation (NOT tagged as session_summary).
        store_observation(
            &backend,
            ObservationType::Discovery,
            "Read",
            "session_summary in text but not a real summary",
            None,
        );

        let ctx = build(&backend, &default_config(), None).unwrap();
        assert!(
            ctx.session_summaries.is_empty(),
            "should not include non-summary observations"
        );
    }

    // =========================================================================
    // T039: Token budget enforcement
    // =========================================================================

    #[test]
    fn build_respects_token_budget() {
        let backend = MockBackend::new();
        backend.create(Path::new("/tmp/test.mv2")).unwrap();

        for i in 0..20 {
            store_observation(
                &backend,
                ObservationType::Discovery,
                "Read",
                &format!("Observation number {i} with some text"),
                Some(&"x".repeat(200)),
            );
        }

        let config = MindConfig {
            max_context_observations: 20,
            max_context_tokens: 100, // Very small budget.
            ..MindConfig::default()
        };

        let ctx = build(&backend, &config, None).unwrap();
        assert!(
            ctx.token_count <= 100,
            "token_count ({}) should not exceed budget (100)",
            ctx.token_count
        );
        assert!(
            ctx.recent_observations.len() < 20,
            "should not include all 20 observations with small budget"
        );
    }

    #[test]
    fn build_truncates_oversized_single_observation() {
        let backend = MockBackend::new();
        backend.create(Path::new("/tmp/test.mv2")).unwrap();

        let large_content = "x".repeat(4000);
        store_observation(
            &backend,
            ObservationType::Discovery,
            "Read",
            "Large observation",
            Some(&large_content),
        );

        let config = MindConfig {
            max_context_observations: 10,
            max_context_tokens: 200,
            ..MindConfig::default()
        };

        let ctx = build(&backend, &config, None).unwrap();
        assert!(
            ctx.token_count <= 200,
            "token_count ({}) should not exceed budget (200)",
            ctx.token_count
        );
        // The observation should be included but with truncated content.
        if !ctx.recent_observations.is_empty() {
            let obs = &ctx.recent_observations[0];
            if let Some(ref content) = obs.content {
                assert!(
                    content.len() < large_content.len(),
                    "content should be truncated"
                );
            }
        }
    }

    #[test]
    fn build_empty_store_returns_empty_context() {
        let backend = MockBackend::new();
        backend.create(Path::new("/tmp/test.mv2")).unwrap();

        let ctx = build(&backend, &default_config(), None).unwrap();
        assert!(ctx.recent_observations.is_empty());
        assert!(ctx.relevant_memories.is_empty());
        assert!(ctx.session_summaries.is_empty());
        assert_eq!(ctx.token_count, 0);
    }

    #[test]
    fn build_token_count_is_non_zero_with_content() {
        let backend = MockBackend::new();
        backend.create(Path::new("/tmp/test.mv2")).unwrap();

        store_observation(
            &backend,
            ObservationType::Discovery,
            "Read",
            "Some content here",
            None,
        );

        let ctx = build(&backend, &default_config(), None).unwrap();
        assert!(ctx.token_count > 0, "token_count should be non-zero");
    }

    // =========================================================================
    // Helper function tests
    // =========================================================================

    #[test]
    fn parse_observation_from_valid_metadata() {
        let meta = serde_json::json!({
            "id": ulid::Ulid::new().to_string().to_lowercase(),
            "obs_type": "discovery",
            "tool_name": "Read",
            "summary": "Found a pattern",
            "content": "Details here",
            "timestamp": chrono::Utc::now().to_rfc3339(),
            "session_id": "s1",
            "metadata": null,
        });

        let obs = parse_observation_from_metadata(&meta).unwrap();
        assert_eq!(obs.obs_type, ObservationType::Discovery);
        assert_eq!(obs.summary, "Found a pattern");
        assert_eq!(obs.content.as_deref(), Some("Details here"));
        assert_eq!(obs.tool_name, "Read");
    }

    #[test]
    fn parse_observation_returns_none_for_missing_id() {
        let meta = serde_json::json!({
            "obs_type": "discovery",
            "summary": "test",
        });
        assert!(parse_observation_from_metadata(&meta).is_none());
    }

    #[test]
    fn truncate_at_char_boundary_ascii() {
        assert_eq!(truncate_at_char_boundary("hello world", 5), 5);
        assert_eq!(&"hello world"[..5], "hello");
    }

    #[test]
    fn truncate_at_char_boundary_unicode() {
        // "🧠" is 4 bytes. Trying to truncate at byte 2 should fall back to 0.
        assert_eq!(truncate_at_char_boundary("🧠abc", 2), 0);
        // Truncate at byte 4 hits the char boundary.
        assert_eq!(truncate_at_char_boundary("🧠abc", 4), 4);
    }
}
