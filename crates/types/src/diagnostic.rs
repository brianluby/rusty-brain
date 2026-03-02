//! Diagnostic types for structured, redacted error and warning records.
//!
//! A [`DiagnosticRecord`] captures a structured, privacy-safe snapshot of an
//! error or warning that occurred during event processing. All records are
//! created with `redacted = true` by default and carry a computed expiry based
//! on [`DIAGNOSTIC_RETENTION_DAYS`].
//!
//! [`DiagnosticSeverity`] classifies the urgency of a diagnostic event.

use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Maximum number of unique field names retained in a diagnostic record.
pub const MAX_DIAGNOSTIC_FIELDS: usize = 20;
/// Default retention period for diagnostic records, in days.
pub const DIAGNOSTIC_RETENTION_DAYS: u32 = 30;

/// Severity level for diagnostic records.
///
/// Serialized as lowercase strings (e.g. `"info"`, `"warning"`, `"error"`).
/// The enum is `#[non_exhaustive]` to allow new variants in future releases.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticSeverity {
    /// Informational diagnostic — no action required.
    Info,
    /// Warning diagnostic — potential issue worth attention.
    Warning,
    /// Error diagnostic — a processing failure occurred.
    Error,
}

/// A structured, redacted record of an error or warning during event processing.
///
/// Created via [`DiagnosticRecord::new`], which auto-generates an ID,
/// deduplicates and caps `affected_fields`, and computes expiry. All records
/// default to `redacted = true` to ensure privacy safety.
///
/// Serialized with camelCase keys.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticRecord {
    /// Unique identifier for this diagnostic record (UUID v4).
    pub id: Uuid,
    /// Timestamp when the diagnostic was created (UTC).
    pub timestamp: DateTime<Utc>,
    /// Platform that generated the diagnostic (e.g. `"claude-code"`).
    pub platform: String,
    /// Classification of the error (e.g. `"SchemaViolation"`).
    pub error_type: String,
    /// Field names affected by the error, deduplicated and capped.
    pub affected_fields: Vec<String>,
    /// Severity of the diagnostic event.
    pub severity: DiagnosticSeverity,
    /// Whether the record content has been redacted for privacy.
    pub redacted: bool,
    /// Number of days this record should be retained.
    pub retention_days: u32,
    /// Computed expiry time (`timestamp + retention_days`).
    pub expires_at: DateTime<Utc>,
}

impl DiagnosticRecord {
    /// Create a new diagnostic record.
    ///
    /// Automatically:
    /// - Generates a UUID v4 id
    /// - Sets timestamp to now (UTC)
    /// - Deduplicates `affected_fields` while preserving insertion order
    /// - Caps `affected_fields` at [`MAX_DIAGNOSTIC_FIELDS`] (20)
    /// - Sets `redacted = true`
    /// - Sets `retention_days` to [`DIAGNOSTIC_RETENTION_DAYS`] (30)
    /// - Computes `expires_at = timestamp + 30 days`
    #[must_use]
    pub fn new(
        platform: String,
        error_type: String,
        affected_fields: Vec<String>,
        severity: DiagnosticSeverity,
    ) -> Self {
        let now = Utc::now();
        // Deduplicate while preserving insertion order
        let mut seen = std::collections::HashSet::new();
        let deduped: Vec<String> = affected_fields
            .into_iter()
            .filter(|f| seen.insert(f.clone()))
            .take(MAX_DIAGNOSTIC_FIELDS)
            .collect();
        let expires = now + TimeDelta::days(i64::from(DIAGNOSTIC_RETENTION_DAYS));
        Self {
            id: Uuid::new_v4(),
            timestamp: now,
            platform,
            error_type,
            affected_fields: deduped,
            severity,
            redacted: true,
            retention_days: 30,
            expires_at: expires,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeDelta, Utc};

    // --- FR-019: Unique ID generation ---

    #[test]
    fn new_generates_unique_id() {
        let a = DiagnosticRecord::new(
            "claude-code".to_string(),
            "SchemaViolation".to_string(),
            vec!["field1".to_string()],
            DiagnosticSeverity::Error,
        );
        let b = DiagnosticRecord::new(
            "claude-code".to_string(),
            "SchemaViolation".to_string(),
            vec!["field1".to_string()],
            DiagnosticSeverity::Error,
        );

        assert_ne!(a.id, b.id, "each DiagnosticRecord must have a unique UUID");
    }

    // --- FR-019: Timestamp is current ---

    #[test]
    fn new_sets_timestamp() {
        let before = Utc::now();
        let record = DiagnosticRecord::new(
            "opencode".to_string(),
            "ParseError".to_string(),
            vec![],
            DiagnosticSeverity::Warning,
        );
        let after = Utc::now();

        assert!(
            record.timestamp >= before && record.timestamp <= after,
            "timestamp must be between before ({before}) and after ({after}), got {}",
            record.timestamp,
        );
    }

    // --- FR-020: Deduplication ---

    #[test]
    fn new_deduplicates_fields() {
        let fields = vec![
            "a".to_string(),
            "b".to_string(),
            "a".to_string(),
            "c".to_string(),
            "b".to_string(),
        ];
        let record = DiagnosticRecord::new(
            "claude-code".to_string(),
            "FieldConflict".to_string(),
            fields,
            DiagnosticSeverity::Info,
        );

        assert_eq!(
            record.affected_fields,
            vec!["a".to_string(), "b".to_string(), "c".to_string()],
            "duplicates must be removed while preserving insertion order",
        );
    }

    // --- FR-020: Cap at MAX_DIAGNOSTIC_FIELDS ---

    #[test]
    fn new_caps_fields_at_20() {
        let fields: Vec<String> = (0..25).map(|i| format!("field_{i}")).collect();
        assert_eq!(fields.len(), 25, "precondition: 25 unique fields");

        let record = DiagnosticRecord::new(
            "claude-code".to_string(),
            "TooManyFields".to_string(),
            fields,
            DiagnosticSeverity::Warning,
        );

        assert_eq!(
            record.affected_fields.len(),
            MAX_DIAGNOSTIC_FIELDS,
            "affected_fields must be capped at {MAX_DIAGNOSTIC_FIELDS}",
        );
        // Verify the first 20 were kept (order preserved)
        assert_eq!(record.affected_fields[0], "field_0");
        assert_eq!(record.affected_fields[19], "field_19");
    }

    // --- FR-022: Redacted always true ---

    #[test]
    fn new_sets_redacted_true() {
        let record = DiagnosticRecord::new(
            "claude-code".to_string(),
            "SomeError".to_string(),
            vec!["x".to_string()],
            DiagnosticSeverity::Error,
        );

        assert!(
            record.redacted,
            "redacted must always be true on new records"
        );
    }

    // --- FR-021: Retention days ---

    #[test]
    fn new_sets_retention_30_days() {
        let record = DiagnosticRecord::new(
            "opencode".to_string(),
            "Timeout".to_string(),
            vec![],
            DiagnosticSeverity::Warning,
        );

        assert_eq!(
            record.retention_days, 30,
            "retention_days must default to {DIAGNOSTIC_RETENTION_DAYS}",
        );
    }

    // --- FR-021: Expires at computation ---

    #[test]
    fn new_computes_expires_at() {
        let record = DiagnosticRecord::new(
            "claude-code".to_string(),
            "ProcessingError".to_string(),
            vec![],
            DiagnosticSeverity::Error,
        );

        let expected_expires =
            record.timestamp + TimeDelta::days(i64::from(DIAGNOSTIC_RETENTION_DAYS));
        // Allow 1-second tolerance for clock drift between timestamp and expires_at
        let diff = (record.expires_at - expected_expires).abs();
        assert!(
            diff < TimeDelta::seconds(1),
            "expires_at must be approximately timestamp + {DIAGNOSTIC_RETENTION_DAYS} days, \
             diff was {diff}",
        );
    }

    // --- DiagnosticSeverity: all variants ---

    #[test]
    fn severity_all_variants() {
        let info = DiagnosticSeverity::Info;
        let warning = DiagnosticSeverity::Warning;
        let error = DiagnosticSeverity::Error;

        // Verify they are distinct values
        assert_ne!(info, warning);
        assert_ne!(warning, error);
        assert_ne!(info, error);

        // Verify Debug formatting works
        assert_eq!(format!("{info:?}"), "Info");
        assert_eq!(format!("{warning:?}"), "Warning");
        assert_eq!(format!("{error:?}"), "Error");
    }

    // --- Serde round-trip ---

    #[test]
    fn serde_round_trip() {
        let original = DiagnosticRecord::new(
            "claude-code".to_string(),
            "SchemaViolation".to_string(),
            vec!["name".to_string(), "email".to_string()],
            DiagnosticSeverity::Error,
        );

        let json = serde_json::to_string(&original).expect("serialization must succeed");
        let deserialized: DiagnosticRecord =
            serde_json::from_str(&json).expect("deserialization must succeed");

        assert_eq!(
            original, deserialized,
            "round-trip must preserve all DiagnosticRecord fields",
        );

        // Verify camelCase key naming in JSON output
        assert!(
            json.contains("\"errorType\""),
            "JSON must use camelCase key 'errorType', got: {json}",
        );
        assert!(
            json.contains("\"affectedFields\""),
            "JSON must use camelCase key 'affectedFields', got: {json}",
        );
        assert!(
            json.contains("\"retentionDays\""),
            "JSON must use camelCase key 'retentionDays', got: {json}",
        );
        assert!(
            json.contains("\"expiresAt\""),
            "JSON must use camelCase key 'expiresAt', got: {json}",
        );

        // Verify severity serializes as lowercase
        assert!(
            json.contains("\"error\""),
            "DiagnosticSeverity::Error must serialize as lowercase 'error', got: {json}",
        );
    }
}
