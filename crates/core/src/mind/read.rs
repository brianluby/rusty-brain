//! Read API: search, ask, timeline, `get_context`.

use chrono::{DateTime, Utc};

use crate::backend::SearchHit;
use types::{InjectedContext, ObservationType, RustyBrainError};

use super::{MemorySearchResult, Mind, TimelineEntry};

impl Mind {
    /// Search observations by text query. Returns results ranked by relevance.
    ///
    /// # Errors
    ///
    /// Returns `RustyBrainError::Storage` if the backend search fails.
    #[tracing::instrument(skip(self), fields(query_len = query.len()))]
    pub fn search(
        &self,
        query: &str,
        limit: Option<usize>,
    ) -> Result<Vec<MemorySearchResult>, RustyBrainError> {
        let requested = limit.unwrap_or(10);
        let fetch_limit = if self.config.min_confidence > 0.0 {
            requested.saturating_mul(3)
        } else {
            requested
        };
        let hits = self.backend.find(query, fetch_limit)?;
        let results: Vec<_> = hits
            .iter()
            .map(parse_search_hit)
            .filter(|r| r.score >= self.config.min_confidence)
            .take(requested)
            .collect();
        tracing::debug!(result_count = results.len(), "search complete");
        Ok(results)
    }

    /// Ask a question against stored observations.
    ///
    /// Returns `Some(answer)` when relevant memories are found, or `None`
    /// when the backend returns no results. Callers decide how to present
    /// the "no results" case instead of comparing against a sentinel string.
    ///
    /// # Errors
    ///
    /// Returns `RustyBrainError::Storage` if the backend ask fails.
    #[tracing::instrument(skip(self), fields(question_len = question.len()))]
    pub fn ask(&self, question: &str) -> Result<Option<String>, RustyBrainError> {
        let answer = self.backend.ask(question, 10)?;
        if answer.trim().is_empty() {
            Ok(None)
        } else {
            Ok(Some(answer))
        }
    }

    /// Query timeline entries.
    ///
    /// Returns observations parsed from backend frames. When `reverse` is
    /// `true`, entries are ordered most-recent-first (default CLI behavior).
    /// When `false`, entries are ordered oldest-first.
    ///
    /// The `limit` parameter controls the maximum number of entries returned.
    ///
    /// # Errors
    ///
    /// Returns [`RustyBrainError::Storage`] if the backend timeline or
    /// frame lookup fails.
    #[tracing::instrument(skip(self))]
    pub fn timeline(
        &self,
        limit: usize,
        reverse: bool,
    ) -> Result<Vec<TimelineEntry>, RustyBrainError> {
        let entries = self.backend.timeline(limit, reverse)?;
        let mut result = Vec::with_capacity(entries.len());
        for entry in &entries {
            let frame = self.backend.frame_by_id(entry.frame_id)?;
            let obs_type = frame
                .metadata
                .get("obs_type")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<ObservationType>().ok())
                .unwrap_or(ObservationType::Discovery);
            let summary = frame
                .metadata
                .get("summary")
                .and_then(|v| v.as_str())
                .unwrap_or(&entry.preview)
                .to_string();
            let timestamp = frame
                .metadata
                .get("timestamp")
                .and_then(|v| v.as_str())
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.with_timezone(&Utc))
                .or_else(|| {
                    // Fall back to backend-provided epoch timestamp before Utc::now().
                    frame
                        .timestamp
                        .and_then(|epoch| DateTime::from_timestamp(epoch, 0))
                        .map(|dt| dt.with_timezone(&Utc))
                })
                .unwrap_or_else(Utc::now);
            let tool_name = frame
                .metadata
                .get("tool_name")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            result.push(TimelineEntry {
                obs_type,
                summary,
                timestamp,
                tool_name,
            });
        }
        tracing::debug!(entry_count = result.len(), "timeline query complete");
        Ok(result)
    }

    /// Assemble session context for agent startup.
    ///
    /// Combines: recent observations (timeline), relevant memories (find),
    /// session summaries (find). Bounded by token budget (chars / 4).
    ///
    /// # Errors
    ///
    /// Returns `RustyBrainError::Storage` if any backend operation fails.
    #[tracing::instrument(skip(self))]
    pub fn get_context(&self, query: Option<&str>) -> Result<InjectedContext, RustyBrainError> {
        let ctx = crate::context_builder::build(self.backend.as_ref(), &self.config, query)?;
        tracing::debug!(
            recent = ctx.recent_observations.len(),
            relevant = ctx.relevant_memories.len(),
            summaries = ctx.session_summaries.len(),
            tokens = ctx.token_count,
            "context assembled"
        );
        Ok(ctx)
    }
}

/// Parse a backend `SearchHit` into a `MemorySearchResult`.
fn parse_search_hit(hit: &SearchHit) -> MemorySearchResult {
    let meta = &hit.metadata;

    let obs_type_str = meta["obs_type"].as_str().unwrap_or("discovery");
    let obs_type: ObservationType = obs_type_str.parse().unwrap_or(ObservationType::Discovery);

    let summary = meta["summary"].as_str().unwrap_or(&hit.text).to_string();

    let content_excerpt = meta["content"].as_str().map(|s| {
        if s.len() > 200 {
            // Find the nearest char boundary at or before byte 200.
            let mut end = 200;
            while end > 0 && !s.is_char_boundary(end) {
                end -= 1;
            }
            format!("{}...", &s[..end])
        } else {
            s.to_string()
        }
    });

    let timestamp = meta["timestamp"]
        .as_str()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map_or_else(Utc::now, |dt| dt.with_timezone(&Utc));

    let tool_name = meta["tool_name"].as_str().unwrap_or("").to_string();

    MemorySearchResult {
        obs_type,
        summary,
        content_excerpt,
        timestamp,
        score: hit.score,
        tool_name,
    }
}
