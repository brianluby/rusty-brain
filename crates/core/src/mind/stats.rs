//! Statistics computation and caching.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use types::{MindStats, ObservationType, RustyBrainError};

use super::Mind;

impl Mind {
    /// Compute memory statistics.
    ///
    /// Iterates all frames via timeline + `frame_by_id` to compute type counts,
    /// session counts, and timestamp range. Results are cached and returned on
    /// subsequent calls if the frame count has not changed.
    ///
    /// # Errors
    ///
    /// Returns `RustyBrainError::Storage` if the backend stats query fails.
    #[tracing::instrument(skip(self))]
    pub fn stats(&self) -> Result<MindStats, RustyBrainError> {
        let backend_stats = self.backend.stats()?;

        // Return cached stats if frame_count matches (no new observations).
        if let Ok(cache) = self.cached_stats.lock() {
            if let Some(ref s) = *cache {
                if s.total_observations == backend_stats.frame_count {
                    return Ok(s.clone());
                }
            }
        }

        // Compute enriched stats from timeline + frame_by_id.
        let timeline = self.backend.timeline(
            usize::try_from(backend_stats.frame_count).unwrap_or(usize::MAX),
            false,
        )?;

        let mut total_sessions: u64 = 0;
        let mut oldest_memory: Option<DateTime<Utc>> = None;
        let mut newest_memory: Option<DateTime<Utc>> = None;
        let mut type_counts: HashMap<ObservationType, u64> = HashMap::new();

        for entry in &timeline {
            let frame = self.backend.frame_by_id(entry.frame_id)?;

            // Parse obs_type from metadata, falling back to Discovery
            // (same fallback as Mind::timeline) to avoid undercounts.
            let obs_type = frame
                .metadata
                .get("obs_type")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<ObservationType>().ok())
                .unwrap_or(ObservationType::Discovery);
            *type_counts.entry(obs_type).or_insert(0) += 1;

            // Count session summaries by tag.
            if frame.tags.iter().any(|t| t == "session_summary") {
                total_sessions += 1;
            }

            // Track timestamps for oldest/newest.
            if let Some(ts_str) = frame.metadata.get("timestamp").and_then(|v| v.as_str()) {
                if let Ok(dt) = DateTime::parse_from_rfc3339(ts_str) {
                    let ts = dt.with_timezone(&Utc);
                    match oldest_memory {
                        None => oldest_memory = Some(ts),
                        Some(old) if ts < old => oldest_memory = Some(ts),
                        _ => {}
                    }
                    match newest_memory {
                        None => newest_memory = Some(ts),
                        Some(new) if ts > new => newest_memory = Some(ts),
                        _ => {}
                    }
                }
            }
        }

        let stats = MindStats {
            total_observations: backend_stats.frame_count,
            total_sessions,
            oldest_memory,
            newest_memory,
            file_size_bytes: backend_stats.file_size,
            type_counts,
        };

        if let Ok(mut cache) = self.cached_stats.lock() {
            *cache = Some(stats.clone());
        }

        tracing::debug!(
            observations = stats.total_observations,
            sessions = stats.total_sessions,
            "stats computed"
        );

        Ok(stats)
    }
}
