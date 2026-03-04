//! Write API: remember, `save_session_summary`.

use chrono::Utc;

use types::{ObservationMetadata, ObservationType, RustyBrainError, SessionSummary, error_codes};

use super::Mind;

impl Mind {
    /// Store an observation. Returns the observation's ULID string.
    ///
    /// Required: `obs_type`, `summary` (non-empty), `tool_name` (non-empty).
    /// Optional: `content`, `metadata`.
    /// Auto-generates: ULID id, UTC timestamp.
    ///
    /// # Errors
    ///
    /// Returns `RustyBrainError::InvalidInput` if `summary` or `tool_name` is
    /// empty or whitespace-only. Returns `RustyBrainError::Storage` if the
    /// backend write or commit fails.
    ///
    /// This method acquires the cross-process file lock internally before
    /// mutating storage.
    #[tracing::instrument(skip(self, content, metadata), fields(obs_type = %obs_type, tool = tool_name))]
    pub fn remember(
        &self,
        obs_type: ObservationType,
        tool_name: &str,
        summary: &str,
        content: Option<&str>,
        metadata: Option<&ObservationMetadata>,
    ) -> Result<String, RustyBrainError> {
        self.with_lock(|mind| {
            mind.remember_unlocked(obs_type, tool_name, summary, content, metadata)
        })
    }

    fn remember_unlocked(
        &self,
        obs_type: ObservationType,
        tool_name: &str,
        summary: &str,
        content: Option<&str>,
        metadata: Option<&ObservationMetadata>,
    ) -> Result<String, RustyBrainError> {
        // Validate inputs.
        if summary.trim().is_empty() {
            return Err(RustyBrainError::InvalidInput {
                code: error_codes::E_INPUT_EMPTY_FIELD,
                message: "summary must not be empty".to_string(),
            });
        }
        if tool_name.trim().is_empty() {
            return Err(RustyBrainError::InvalidInput {
                code: error_codes::E_INPUT_EMPTY_FIELD,
                message: "tool_name must not be empty".to_string(),
            });
        }

        let id = ulid::Ulid::new().to_string().to_lowercase();
        let timestamp = Utc::now();

        // Build the text payload: summary + optional content.
        let payload = match content {
            Some(c) => format!("{summary}\n\n{c}"),
            None => summary.to_string(),
        };

        // Labels: observation type.
        let labels = vec![obs_type.to_string()];

        // Tags: tool_name + session_id.
        let tags = vec![tool_name.to_string(), self.session_id.clone()];

        // Metadata: full observation JSON for round-trip fidelity.
        let meta_json = serde_json::json!({
            "id": id,
            "obs_type": obs_type.to_string(),
            "tool_name": tool_name,
            "summary": summary,
            "content": content,
            "timestamp": timestamp.to_rfc3339(),
            "session_id": &self.session_id,
            "metadata": metadata,
        });

        self.backend
            .put(payload.as_bytes(), &labels, &tags, &meta_json)?;
        self.backend.commit()?;

        // Invalidate cached stats.
        if let Ok(mut cache) = self.cached_stats.lock() {
            *cache = None;
        }

        tracing::debug!(id = %id, "observation stored");

        Ok(id)
    }

    /// Store a session summary as a tagged, searchable observation.
    ///
    /// Returns the observation's ULID string. The summary is stored with
    /// `obs_type=Decision` and tagged as `"session_summary"` so it appears
    /// in future context injections via [`Mind::get_context`].
    ///
    /// # Errors
    ///
    /// Returns `RustyBrainError::InvalidInput` if `summary` is empty.
    /// Returns `RustyBrainError::Storage` if the backend write fails.
    #[tracing::instrument(skip(self, decisions, modified_files))]
    pub fn save_session_summary(
        &self,
        decisions: Vec<String>,
        modified_files: Vec<String>,
        summary: &str,
    ) -> Result<String, RustyBrainError> {
        let now = Utc::now();

        // observation_count is 0: the summary captures decisions and files, not
        // individual observation counts which are tracked separately by stats().
        let session_summary = SessionSummary::new(
            self.session_id.clone(),
            now,
            now,
            0,
            decisions,
            modified_files,
            summary.to_string(),
        )?;

        let content_json =
            serde_json::to_string(&session_summary).map_err(|e| RustyBrainError::Storage {
                code: error_codes::E_STORAGE_BACKEND,
                message: "failed to serialize session summary".to_string(),
                source: Some(types::StorageSource(e.to_string())),
            })?;

        // Use "session_summary" as tool_name so it becomes a tag.
        // Prefix the display summary with "session_summary:" for text searchability.
        let store_summary = format!("session_summary: {summary}");

        self.remember(
            ObservationType::Decision,
            "session_summary",
            &store_summary,
            Some(&content_json),
            None,
        )
    }
}
