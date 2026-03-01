//! Session summary representing an aggregated coding session.
//!
//! A [`SessionSummary`] captures the high-level outcome of one agent coding
//! session: time span, observation count, key decisions, and modified files.
//! It is validated on construction via [`SessionSummary::new`].

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{AgentBrainError, error_codes};

/// Aggregated summary of a single agent coding session.
///
/// Serialized with camelCase keys. The `modified_files` field is serialized
/// as `"filesModified"` to match the upstream JSON contract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    /// Unique session identifier (must not be empty).
    pub id: String,
    /// UTC timestamp when the session started.
    pub start_time: DateTime<Utc>,
    /// UTC timestamp when the session ended (must be >= `start_time`).
    pub end_time: DateTime<Utc>,
    /// Number of observations recorded during this session.
    pub observation_count: u64,
    /// Notable architectural or design decisions made during the session.
    #[serde(default)]
    pub key_decisions: Vec<String>,
    /// Files modified during the session. Serialized as `"filesModified"`.
    #[serde(default, rename = "filesModified")]
    pub modified_files: Vec<String>,
    /// Human-readable summary of the session (must not be empty).
    pub summary: String,
}

impl SessionSummary {
    /// Construct a validated `SessionSummary`.
    ///
    /// # Errors
    ///
    /// Returns `AgentBrainError::InvalidInput` if `id` or `summary` is empty,
    /// or if `end_time` is before `start_time`.
    pub fn new(
        id: String,
        start_time: DateTime<Utc>,
        end_time: DateTime<Utc>,
        observation_count: u64,
        key_decisions: Vec<String>,
        modified_files: Vec<String>,
        summary: String,
    ) -> Result<Self, AgentBrainError> {
        if id.is_empty() {
            return Err(AgentBrainError::InvalidInput {
                code: error_codes::E_INPUT_EMPTY_FIELD,
                message: "id must not be empty".to_string(),
            });
        }

        if summary.is_empty() {
            return Err(AgentBrainError::InvalidInput {
                code: error_codes::E_INPUT_EMPTY_FIELD,
                message: "summary must not be empty".to_string(),
            });
        }

        if end_time < start_time {
            return Err(AgentBrainError::InvalidInput {
                code: error_codes::E_INPUT_OUT_OF_RANGE,
                message: "end_time must not be before start_time".to_string(),
            });
        }

        Ok(Self {
            id,
            start_time,
            end_time,
            observation_count,
            key_decisions,
            modified_files,
            summary,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{AgentBrainError, error_codes};
    use chrono::{Duration, Utc};

    // T006: Unit tests for SessionSummary
    // These tests define the required behaviour of SessionSummary::new().
    // They are intentionally written before the implementation (RED phase).

    // ---------------------------------------------------------------------------
    // Helpers
    // ---------------------------------------------------------------------------

    /// Returns a base start time anchored to Utc::now() for use across tests.
    fn base_start() -> chrono::DateTime<Utc> {
        Utc::now()
    }

    /// Builds a valid SessionSummary via the constructor with default-safe values.
    fn valid_summary(
        id: &str,
        start: chrono::DateTime<Utc>,
        end: chrono::DateTime<Utc>,
        summary: &str,
    ) -> Result<SessionSummary, AgentBrainError> {
        SessionSummary::new(
            id.to_string(),
            start,
            end,
            5,
            vec!["use async trait".to_string()],
            vec!["src/main.rs".to_string()],
            summary.to_string(),
        )
    }

    // ---------------------------------------------------------------------------
    // Construction — happy path
    // ---------------------------------------------------------------------------

    #[test]
    fn valid_construction_returns_ok() {
        let start = base_start();
        let end = start + Duration::hours(1);
        let result = valid_summary("session-001", start, end, "A productive session.");
        assert!(
            result.is_ok(),
            "Expected Ok for valid inputs, got: {result:?}"
        );
    }

    #[test]
    fn valid_construction_stores_all_fields() {
        let start = base_start();
        let end = start + Duration::minutes(30);
        let id = "session-abc".to_string();
        let decisions = vec!["adopt_serde".to_string(), "pin_memvid".to_string()];
        let files = vec!["crates/types/src/lib.rs".to_string()];
        let summary_text = "Defined the type system.".to_string();
        let observation_count: u64 = 42;

        let session = SessionSummary::new(
            id.clone(),
            start,
            end,
            observation_count,
            decisions.clone(),
            files.clone(),
            summary_text.clone(),
        )
        .expect("construction with valid inputs must succeed");

        assert_eq!(session.id, id);
        assert_eq!(session.start_time, start);
        assert_eq!(session.end_time, end);
        assert_eq!(session.observation_count, observation_count);
        assert_eq!(session.key_decisions, decisions);
        assert_eq!(session.modified_files, files);
        assert_eq!(session.summary, summary_text);
    }

    #[test]
    fn equal_start_and_end_time_is_valid() {
        let start = base_start();
        // end_time == start_time should be accepted (zero-duration session)
        let result = valid_summary("session-zero", start, start, "Instantaneous.");
        assert!(
            result.is_ok(),
            "end_time == start_time must be accepted, got: {result:?}"
        );
    }

    #[test]
    fn empty_vec_fields_are_valid() {
        let start = base_start();
        let end = start + Duration::seconds(10);
        let result = SessionSummary::new(
            "session-minimal".to_string(),
            start,
            end,
            0,
            vec![],
            vec![],
            "Minimal session.".to_string(),
        );
        assert!(
            result.is_ok(),
            "Empty key_decisions and modified_files must be accepted, got: {result:?}"
        );
    }

    #[test]
    fn zero_observation_count_is_valid() {
        let start = base_start();
        let end = start + Duration::minutes(5);
        let result = valid_summary("session-noop", start, end, "Nothing observed.");
        assert!(
            result.is_ok(),
            "observation_count of 0 must be accepted, got: {result:?}"
        );
    }

    // ---------------------------------------------------------------------------
    // Validation — end_time < start_time
    // ---------------------------------------------------------------------------

    #[test]
    fn end_time_before_start_time_returns_invalid_input_error() {
        let start = base_start();
        let end = start - Duration::seconds(1); // one second in the past
        let result = valid_summary("session-backwards", start, end, "Backwards time.");
        assert!(
            result.is_err(),
            "Expected Err when end_time < start_time, got Ok"
        );
        let err = result.unwrap_err();
        assert!(
            matches!(err, AgentBrainError::InvalidInput { .. }),
            "Expected InvalidInput variant, got: {err:?}"
        );
    }

    #[test]
    fn end_time_before_start_time_uses_out_of_range_code() {
        let start = base_start();
        let end = start - Duration::hours(2);
        let result = valid_summary("session-past", start, end, "Way in the past.");
        let err = result.expect_err("Expected error for end_time < start_time");
        assert_eq!(
            err.code(),
            error_codes::E_INPUT_OUT_OF_RANGE,
            "Expected error code E_INPUT_OUT_OF_RANGE for time range violation"
        );
    }

    // ---------------------------------------------------------------------------
    // Validation — empty id
    // ---------------------------------------------------------------------------

    #[test]
    fn empty_id_returns_invalid_input_error() {
        let start = base_start();
        let end = start + Duration::minutes(1);
        let result = SessionSummary::new(
            "".to_string(), // empty id
            start,
            end,
            1,
            vec![],
            vec![],
            "Has content.".to_string(),
        );
        assert!(result.is_err(), "Expected Err for empty id, got Ok");
        let err = result.unwrap_err();
        assert!(
            matches!(err, AgentBrainError::InvalidInput { .. }),
            "Expected InvalidInput variant for empty id, got: {err:?}"
        );
    }

    #[test]
    fn empty_id_uses_empty_field_code() {
        let start = base_start();
        let end = start + Duration::minutes(1);
        let result = SessionSummary::new(
            "".to_string(),
            start,
            end,
            0,
            vec![],
            vec![],
            "Non-empty summary.".to_string(),
        );
        let err = result.expect_err("Expected error for empty id");
        assert_eq!(
            err.code(),
            error_codes::E_INPUT_EMPTY_FIELD,
            "Expected error code E_INPUT_EMPTY_FIELD for empty id"
        );
    }

    // ---------------------------------------------------------------------------
    // Validation — empty summary
    // ---------------------------------------------------------------------------

    #[test]
    fn empty_summary_returns_invalid_input_error() {
        let start = base_start();
        let end = start + Duration::minutes(1);
        let result = SessionSummary::new(
            "session-nosummary".to_string(),
            start,
            end,
            1,
            vec![],
            vec![],
            "".to_string(), // empty summary
        );
        assert!(result.is_err(), "Expected Err for empty summary, got Ok");
        let err = result.unwrap_err();
        assert!(
            matches!(err, AgentBrainError::InvalidInput { .. }),
            "Expected InvalidInput variant for empty summary, got: {err:?}"
        );
    }

    #[test]
    fn empty_summary_uses_empty_field_code() {
        let start = base_start();
        let end = start + Duration::minutes(1);
        let result = SessionSummary::new(
            "session-nosummary".to_string(),
            start,
            end,
            0,
            vec![],
            vec![],
            "".to_string(),
        );
        let err = result.expect_err("Expected error for empty summary");
        assert_eq!(
            err.code(),
            error_codes::E_INPUT_EMPTY_FIELD,
            "Expected error code E_INPUT_EMPTY_FIELD for empty summary"
        );
    }

    // ---------------------------------------------------------------------------
    // Validation — precedence: id checked before summary, time checked last
    // ---------------------------------------------------------------------------

    #[test]
    fn empty_id_takes_priority_over_empty_summary() {
        // When both id and summary are empty, id validation fires first.
        let start = base_start();
        let end = start + Duration::minutes(1);
        let result = SessionSummary::new(
            "".to_string(),
            start,
            end,
            0,
            vec![],
            vec![],
            "".to_string(),
        );
        let err = result.expect_err("Expected error for both empty id and summary");
        assert_eq!(
            err.code(),
            error_codes::E_INPUT_EMPTY_FIELD,
            "Expected E_INPUT_EMPTY_FIELD when both id and summary are empty"
        );
    }

    // ---------------------------------------------------------------------------
    // Return type shape — confirms new() is a constructor, not infallible
    // ---------------------------------------------------------------------------

    #[test]
    fn new_returns_result_type() {
        // This test is trivially satisfied if the above tests compile; it makes
        // the contract explicit for readers of the test suite.
        let start = base_start();
        let end = start + Duration::seconds(5);
        let result: Result<SessionSummary, AgentBrainError> = SessionSummary::new(
            "session-type-check".to_string(),
            start,
            end,
            0,
            vec![],
            vec![],
            "Type check only.".to_string(),
        );
        // Unwrap to confirm success; the important thing is the Result type compiles.
        let _ = result.expect("Valid inputs must produce Ok");
    }

    // ---------------------------------------------------------------------------
    // T017: Round-trip serialization tests
    // ---------------------------------------------------------------------------

    #[test]
    fn session_summary_json_round_trip() {
        let start = base_start();
        let end = start + Duration::hours(2);

        let original = SessionSummary::new(
            "session-rt-001".to_string(),
            start,
            end,
            42,
            vec![
                "use async trait".to_string(),
                "pin dependencies".to_string(),
            ],
            vec![
                "src/main.rs".to_string(),
                "crates/types/src/lib.rs".to_string(),
            ],
            "Productive session implementing types crate.".to_string(),
        )
        .expect("valid session must construct");

        let json = serde_json::to_string(&original).expect("serialization must succeed");
        let deserialized: SessionSummary =
            serde_json::from_str(&json).expect("deserialization must succeed");

        assert_eq!(
            original, deserialized,
            "round-trip must preserve all SessionSummary fields"
        );
    }

    #[test]
    fn session_summary_json_round_trip_empty_vecs() {
        let start = base_start();
        let end = start + Duration::minutes(5);

        let original = SessionSummary::new(
            "session-rt-empty".to_string(),
            start,
            end,
            0,
            vec![],
            vec![],
            "Minimal session with no decisions or files.".to_string(),
        )
        .expect("valid session must construct");

        let json = serde_json::to_string(&original).expect("serialization must succeed");
        let deserialized: SessionSummary =
            serde_json::from_str(&json).expect("deserialization must succeed");

        assert_eq!(
            original, deserialized,
            "round-trip must preserve empty Vec<String> fields"
        );
        assert!(
            deserialized.key_decisions.is_empty(),
            "key_decisions must remain empty after round-trip"
        );
        assert!(
            deserialized.modified_files.is_empty(),
            "modified_files must remain empty after round-trip"
        );
    }

    #[test]
    fn session_summary_json_verify_files_modified_key() {
        let start = base_start();
        let end = start + Duration::minutes(10);

        let session = SessionSummary::new(
            "session-key-check".to_string(),
            start,
            end,
            3,
            vec![],
            vec!["src/lib.rs".to_string()],
            "Key name verification session.".to_string(),
        )
        .expect("valid session must construct");

        let json = serde_json::to_string(&session).expect("serialization must succeed");

        assert!(
            json.contains("\"filesModified\""),
            "JSON must use key 'filesModified', got: {json}"
        );
        assert!(
            !json.contains("\"modifiedFiles\""),
            "JSON must NOT use key 'modifiedFiles', got: {json}"
        );
    }
}
