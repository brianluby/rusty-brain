//! Statistics computation and caching.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use types::{MindStats, ObservationType, RustyBrainError};

use super::Mind;

impl Mind {
    /// Compute memory statistics incrementally.
    ///
    /// Persists stats to `.stats.json` next to the memory file to avoid O(N) full
    /// timeline scans. Only scans frames that are newer than the cached count.
    /// Results are also cached in-memory.
    ///
    /// # Errors
    ///
    /// Returns `RustyBrainError::Storage` if the backend stats query fails.
    #[tracing::instrument(skip(self))]
    pub fn stats(&self) -> Result<MindStats, RustyBrainError> {
        let backend_stats = self.backend.stats()?;
        let cache_path = self.memory_path.with_extension("stats.json");

        // Return in-memory cached stats if frame_count matches.
        if let Ok(cache) = self.cached_stats.lock() {
            if let Some(ref s) = *cache {
                if s.total_observations == backend_stats.frame_count {
                    return Ok(s.clone());
                }
            }
        }

        let mut current_stats = None;
        if let Ok(data) = std::fs::read_to_string(&cache_path) {
            if let Ok(cached) = serde_json::from_str::<MindStats>(&data) {
                if cached.total_observations <= backend_stats.frame_count {
                    current_stats = Some(cached);
                }
            }
        }

        let delta = if let Some(ref cached) = current_stats {
            usize::try_from(
                backend_stats
                    .frame_count
                    .saturating_sub(cached.total_observations),
            )
            .unwrap_or(usize::MAX)
        } else {
            usize::try_from(backend_stats.frame_count).unwrap_or(usize::MAX)
        };

        let mut stats = current_stats.unwrap_or_else(|| MindStats {
            total_observations: 0,
            total_sessions: 0,
            oldest_memory: None,
            newest_memory: None,
            file_size_bytes: backend_stats.file_size,
            type_counts: HashMap::new(),
        });

        if delta > 0 {
            // Fetch delta frames. `timeline(delta, true)` gives newest first.
            let new_frames = self.backend.timeline(delta, true)?;

            for entry in &new_frames {
                let frame = self.backend.frame_by_id(entry.frame_id)?;

                let obs_type = frame
                    .metadata
                    .get("obs_type")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<ObservationType>().ok())
                    .unwrap_or(ObservationType::Discovery);
                *stats.type_counts.entry(obs_type).or_insert(0) += 1;

                if frame.tags.iter().any(|t| t == "session_summary") {
                    stats.total_sessions += 1;
                }

                if let Some(ts_str) = frame.metadata.get("timestamp").and_then(|v| v.as_str()) {
                    if let Ok(dt) = DateTime::parse_from_rfc3339(ts_str) {
                        let ts = dt.with_timezone(&Utc);
                        match stats.oldest_memory {
                            None => stats.oldest_memory = Some(ts),
                            Some(old) if ts < old => stats.oldest_memory = Some(ts),
                            _ => {}
                        }
                        match stats.newest_memory {
                            None => stats.newest_memory = Some(ts),
                            Some(new) if ts > new => stats.newest_memory = Some(ts),
                            _ => {}
                        }
                    }
                }
            }
        }

        stats.total_observations = backend_stats.frame_count;
        stats.file_size_bytes = backend_stats.file_size;

        if let Ok(data) = serde_json::to_string(&stats) {
            let _ = std::fs::write(&cache_path, data);
        }

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
