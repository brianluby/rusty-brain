//! Event processing pipeline composing contract validation and identity resolution.
//!
//! The [`EventPipeline`] is the central coordinator for event intake. It receives
//! a [`PlatformEvent`], validates its contract version, resolves project identity,
//! and returns a [`PipelineResult`] indicating whether the event should be
//! processed or skipped.
//!
//! The pipeline never panics on any input — all error conditions produce a
//! well-formed [`PipelineResult`] with `skipped = true` and a diagnostic record.

use serde::{Deserialize, Serialize};
use types::{DiagnosticRecord, DiagnosticSeverity, IdentitySource, PlatformEvent, ProjectIdentity};

use crate::contract::validate_contract;
use crate::identity::resolve_project_identity;

/// Result of processing an event through the pipeline.
///
/// A non-skipped result carries the resolved [`ProjectIdentity`].
/// A skipped result carries a reason string and a [`DiagnosticRecord`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineResult {
    /// Whether this event should be skipped.
    pub skipped: bool,
    /// Reason for skipping (`None` if not skipped).
    pub reason: Option<String>,
    /// Resolved project identity (`None` if skipped).
    pub identity: Option<ProjectIdentity>,
    /// Diagnostic record for skip cases (`None` if not skipped).
    pub diagnostic: Option<DiagnosticRecord>,
}

/// Event processing pipeline composing contract validation and identity resolution.
///
/// Stateless processor: call [`EventPipeline::process`] for each incoming event.
/// The pipeline validates the contract version, resolves project identity, and
/// returns a [`PipelineResult`] indicating the outcome. Never panics on any input.
#[derive(Default)]
pub struct EventPipeline;

impl EventPipeline {
    /// Create a new event pipeline.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Process a platform event through validation and identity resolution.
    ///
    /// Steps:
    /// 1. Validate the contract version — skip if incompatible or malformed.
    /// 2. Resolve project identity — skip if unresolved.
    /// 3. Both pass — return success with the resolved identity.
    ///
    /// Never panics on any input.
    #[must_use]
    pub fn process(&self, event: &PlatformEvent) -> PipelineResult {
        // Step 1: Contract validation.
        let contract_result = validate_contract(&event.contract_version);
        if !contract_result.compatible {
            let reason = contract_result
                .reason
                .unwrap_or_else(|| "unknown_contract_error".to_string());
            let diagnostic = DiagnosticRecord::new(
                event.platform.clone(),
                reason.clone(),
                vec!["contract_version".to_string()],
                DiagnosticSeverity::Warning,
            );
            return PipelineResult {
                skipped: true,
                reason: Some(reason),
                identity: None,
                diagnostic: Some(diagnostic),
            };
        }

        // Step 2: Identity resolution.
        let identity = resolve_project_identity(&event.project_context);
        if identity.source == IdentitySource::Unresolved {
            let reason = "missing_project_identity".to_string();
            let diagnostic = DiagnosticRecord::new(
                event.platform.clone(),
                reason.clone(),
                vec![
                    "platform_project_id".to_string(),
                    "canonical_path".to_string(),
                    "cwd".to_string(),
                ],
                DiagnosticSeverity::Warning,
            );
            return PipelineResult {
                skipped: true,
                reason: Some(reason),
                identity: None,
                diagnostic: Some(diagnostic),
            };
        }

        // Step 3: Both pass.
        PipelineResult {
            skipped: false,
            reason: None,
            identity: Some(identity),
            diagnostic: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use types::{EventKind, ProjectContext};
    use uuid::Uuid;

    // -------------------------------------------------------------------------
    // Helper: build a test event with configurable fields
    // -------------------------------------------------------------------------

    fn make_event(
        contract_version: &str,
        platform_project_id: Option<&str>,
        canonical_path: Option<&str>,
        cwd: Option<&str>,
    ) -> PlatformEvent {
        PlatformEvent {
            event_id: Uuid::new_v4(),
            timestamp: Utc::now(),
            platform: "claude".to_string(),
            contract_version: contract_version.to_string(),
            session_id: "test-session".to_string(),
            project_context: ProjectContext {
                platform_project_id: platform_project_id.map(String::from),
                canonical_path: canonical_path.map(String::from),
                cwd: cwd.map(String::from),
            },
            kind: EventKind::SessionStart,
        }
    }

    // -------------------------------------------------------------------------
    // T023-1: Valid event — compatible contract + resolvable identity
    // -------------------------------------------------------------------------

    #[test]
    fn valid_event_not_skipped_with_identity() {
        let pipeline = EventPipeline::new();
        let event = make_event("1.0.0", Some("proj-42"), None, None);

        let result = pipeline.process(&event);

        assert!(!result.skipped, "valid event must not be skipped");
        assert_eq!(result.reason, None, "reason must be None for valid event");
        assert!(
            result.identity.is_some(),
            "identity must be present for valid event"
        );
        let identity = result.identity.unwrap();
        assert_eq!(identity.key, Some("proj-42".to_string()));
        assert_eq!(identity.source, IdentitySource::PlatformProjectId);
        assert_eq!(
            result.diagnostic, None,
            "diagnostic must be None for valid event"
        );
    }

    #[test]
    fn valid_event_with_canonical_path_identity() {
        let pipeline = EventPipeline::new();
        let event = make_event("1.2.3", None, Some("/home/user/project"), None);

        let result = pipeline.process(&event);

        assert!(!result.skipped, "valid event must not be skipped");
        assert_eq!(result.reason, None);
        let identity = result.identity.unwrap();
        assert_eq!(identity.key, Some("/home/user/project".to_string()));
        assert_eq!(identity.source, IdentitySource::CanonicalPath);
    }

    #[test]
    fn valid_event_with_cwd_identity() {
        let pipeline = EventPipeline::new();
        let event = make_event("1.0.0", None, None, Some("/tmp/project"));

        let result = pipeline.process(&event);

        assert!(!result.skipped, "valid event must not be skipped");
        let identity = result.identity.unwrap();
        assert_eq!(identity.key, Some("/tmp/project".to_string()));
    }

    // -------------------------------------------------------------------------
    // T023-2: Incompatible contract version → skipped
    // -------------------------------------------------------------------------

    #[test]
    fn incompatible_contract_version_skipped() {
        let pipeline = EventPipeline::new();
        let event = make_event("2.0.0", Some("proj-42"), None, None);

        let result = pipeline.process(&event);

        assert!(result.skipped, "incompatible contract must be skipped");
        assert_eq!(
            result.reason.as_deref(),
            Some("incompatible_contract_major"),
            "reason must be 'incompatible_contract_major'"
        );
        assert_eq!(result.identity, None, "identity must be None when skipped");

        let diag = result.diagnostic.expect("diagnostic must be present");
        assert_eq!(diag.severity, DiagnosticSeverity::Warning);
        assert_eq!(diag.error_type, "incompatible_contract_major");
        assert_eq!(diag.affected_fields, vec!["contract_version".to_string()]);
        assert_eq!(diag.platform, "claude");
    }

    #[test]
    fn incompatible_contract_v0_skipped() {
        let pipeline = EventPipeline::new();
        let event = make_event("0.9.0", Some("proj-42"), None, None);

        let result = pipeline.process(&event);

        assert!(result.skipped);
        assert_eq!(
            result.reason.as_deref(),
            Some("incompatible_contract_major")
        );
    }

    // -------------------------------------------------------------------------
    // T023-3: Compatible contract but unresolvable identity → skipped
    // -------------------------------------------------------------------------

    #[test]
    fn unresolvable_identity_skipped() {
        let pipeline = EventPipeline::new();
        let event = make_event("1.0.0", None, None, None);

        let result = pipeline.process(&event);

        assert!(result.skipped, "unresolvable identity must be skipped");
        assert_eq!(
            result.reason.as_deref(),
            Some("missing_project_identity"),
            "reason must be 'missing_project_identity'"
        );
        assert_eq!(result.identity, None, "identity must be None when skipped");

        let diag = result.diagnostic.expect("diagnostic must be present");
        assert_eq!(diag.severity, DiagnosticSeverity::Warning);
        assert_eq!(diag.error_type, "missing_project_identity");
        assert_eq!(
            diag.affected_fields,
            vec![
                "platform_project_id".to_string(),
                "canonical_path".to_string(),
                "cwd".to_string(),
            ]
        );
        assert_eq!(diag.platform, "claude");
    }

    #[test]
    fn unresolvable_identity_empty_strings_skipped() {
        let pipeline = EventPipeline::new();
        let event = make_event("1.0.0", Some(""), Some(""), Some(""));

        let result = pipeline.process(&event);

        assert!(
            result.skipped,
            "empty-string fields must be treated as absent"
        );
        assert_eq!(result.reason.as_deref(), Some("missing_project_identity"));
    }

    // -------------------------------------------------------------------------
    // T023-4: Malformed contract version → skipped
    // -------------------------------------------------------------------------

    #[test]
    fn malformed_contract_version_skipped() {
        let pipeline = EventPipeline::new();
        let event = make_event("not-a-version", Some("proj-42"), None, None);

        let result = pipeline.process(&event);

        assert!(result.skipped, "malformed contract must be skipped");
        assert_eq!(
            result.reason.as_deref(),
            Some("invalid_contract_version"),
            "reason must be 'invalid_contract_version'"
        );
        assert_eq!(result.identity, None);
        assert!(result.diagnostic.is_some());
    }

    #[test]
    fn empty_contract_version_skipped() {
        let pipeline = EventPipeline::new();
        let event = make_event("", Some("proj-42"), None, None);

        let result = pipeline.process(&event);

        assert!(result.skipped, "empty contract version must be skipped");
        assert_eq!(result.reason.as_deref(), Some("invalid_contract_version"));
    }

    // -------------------------------------------------------------------------
    // T023-5: Pipeline never panics on any input
    // -------------------------------------------------------------------------

    #[test]
    fn never_panics_on_extreme_inputs() {
        let pipeline = EventPipeline::new();

        // Empty everything
        let event = make_event("", None, None, None);
        let _ = pipeline.process(&event);

        // Very long strings
        let long = "a".repeat(10_000);
        let event = make_event(&long, Some(&long), Some(&long), Some(&long));
        let _ = pipeline.process(&event);

        // Unicode and special characters
        let event = make_event(
            "\u{0000}\u{FFFF}",
            Some("\u{1F4A9}"),
            Some("path/with spaces/and\ttabs"),
            Some("/nul\0embedded"),
        );
        let _ = pipeline.process(&event);

        // Version-like but invalid strings
        let event = make_event("1.2.3.4.5", Some("proj"), None, None);
        let _ = pipeline.process(&event);

        let event = make_event("-1.0.0", Some("proj"), None, None);
        let _ = pipeline.process(&event);

        let event = make_event("v1.0.0", Some("proj"), None, None);
        let _ = pipeline.process(&event);
    }

    // -------------------------------------------------------------------------
    // Contract check happens before identity resolution
    // -------------------------------------------------------------------------

    #[test]
    fn contract_check_before_identity_resolution() {
        let pipeline = EventPipeline::new();
        // Both contract and identity are invalid — contract should be checked first
        let event = make_event("2.0.0", None, None, None);

        let result = pipeline.process(&event);

        assert!(result.skipped);
        assert_eq!(
            result.reason.as_deref(),
            Some("incompatible_contract_major"),
            "contract validation must run before identity resolution"
        );
    }

    // -------------------------------------------------------------------------
    // PipelineResult serde round-trip
    // -------------------------------------------------------------------------

    #[test]
    fn pipeline_result_serde_round_trip_success() {
        let identity = ProjectIdentity {
            key: Some("proj-42".to_string()),
            source: IdentitySource::PlatformProjectId,
        };
        let result = PipelineResult {
            skipped: false,
            reason: None,
            identity: Some(identity),
            diagnostic: None,
        };

        let json = serde_json::to_string(&result).expect("serialization must succeed");
        let deserialized: PipelineResult =
            serde_json::from_str(&json).expect("deserialization must succeed");

        assert_eq!(result, deserialized, "round-trip must preserve all fields");

        // Verify camelCase keys in PipelineResult itself
        assert!(
            json.contains("\"skipped\""),
            "JSON should have camelCase key 'skipped', got: {json}"
        );
    }

    #[test]
    fn pipeline_result_serde_round_trip_skipped() {
        let diag = DiagnosticRecord::new(
            "claude".to_string(),
            "incompatible_contract_major".to_string(),
            vec!["contract_version".to_string()],
            DiagnosticSeverity::Warning,
        );
        let result = PipelineResult {
            skipped: true,
            reason: Some("incompatible_contract_major".to_string()),
            identity: None,
            diagnostic: Some(diag),
        };

        let json = serde_json::to_string(&result).expect("serialization must succeed");
        let deserialized: PipelineResult =
            serde_json::from_str(&json).expect("deserialization must succeed");

        assert_eq!(result, deserialized, "round-trip must preserve all fields");
    }

    // -------------------------------------------------------------------------
    // EventPipeline::default() works
    // -------------------------------------------------------------------------

    #[test]
    fn pipeline_default_works() {
        let pipeline = EventPipeline::default();
        let event = make_event("1.0.0", Some("proj-1"), None, None);
        let result = pipeline.process(&event);
        assert!(!result.skipped);
    }

    // -------------------------------------------------------------------------
    // Diagnostic record carries correct platform from event
    // -------------------------------------------------------------------------

    #[test]
    fn diagnostic_carries_event_platform() {
        let pipeline = EventPipeline::new();
        let mut event = make_event("2.0.0", Some("proj"), None, None);
        event.platform = "opencode".to_string();

        let result = pipeline.process(&event);

        let diag = result.diagnostic.expect("diagnostic must be present");
        assert_eq!(
            diag.platform, "opencode",
            "diagnostic platform must match event platform"
        );
    }

    // -------------------------------------------------------------------------
    // T027: Integration tests — diagnostic records from pipeline skip cases
    // -------------------------------------------------------------------------

    #[test]
    fn diagnostic_has_correct_platform_from_contract_skip() {
        let pipeline = EventPipeline::new();
        let mut event = make_event("2.0.0", Some("proj"), None, None);
        event.platform = "custom-platform".to_string();

        let diag = pipeline
            .process(&event)
            .diagnostic
            .expect("diagnostic must be present");
        assert_eq!(
            diag.platform, "custom-platform",
            "diagnostic platform must match event platform"
        );
    }

    #[test]
    fn diagnostic_has_correct_error_type_for_contract_skip() {
        let pipeline = EventPipeline::new();
        let event = make_event("2.0.0", Some("proj"), None, None);

        let result = pipeline.process(&event);
        let diag = result.diagnostic.expect("diagnostic must be present");
        assert_eq!(
            diag.error_type,
            result.reason.as_deref().unwrap(),
            "diagnostic error_type must match skip reason"
        );
    }

    #[test]
    fn diagnostic_severity_is_warning_for_contract_skip() {
        let pipeline = EventPipeline::new();
        let event = make_event("2.0.0", Some("proj"), None, None);

        let diag = pipeline
            .process(&event)
            .diagnostic
            .expect("diagnostic must be present");
        assert_eq!(
            diag.severity,
            DiagnosticSeverity::Warning,
            "contract skip severity must be Warning"
        );
    }

    #[test]
    fn diagnostic_severity_is_warning_for_identity_skip() {
        let pipeline = EventPipeline::new();
        let event = make_event("1.0.0", None, None, None);

        let diag = pipeline
            .process(&event)
            .diagnostic
            .expect("diagnostic must be present");
        assert_eq!(
            diag.severity,
            DiagnosticSeverity::Warning,
            "identity skip severity must be Warning"
        );
    }

    #[test]
    fn diagnostic_redacted_is_true() {
        let pipeline = EventPipeline::new();

        // Contract skip
        let event = make_event("2.0.0", Some("proj"), None, None);
        let diag = pipeline
            .process(&event)
            .diagnostic
            .expect("diagnostic must be present");
        assert!(diag.redacted, "diagnostic must be redacted");

        // Identity skip
        let event = make_event("1.0.0", None, None, None);
        let diag = pipeline
            .process(&event)
            .diagnostic
            .expect("diagnostic must be present");
        assert!(diag.redacted, "diagnostic must be redacted");
    }

    #[test]
    fn diagnostic_expires_at_is_30_days() {
        let pipeline = EventPipeline::new();
        let event = make_event("2.0.0", Some("proj"), None, None);

        let diag = pipeline
            .process(&event)
            .diagnostic
            .expect("diagnostic must be present");

        let expected = diag.timestamp + chrono::TimeDelta::days(30);
        let diff = (diag.expires_at - expected).abs();
        assert!(
            diff < chrono::TimeDelta::seconds(1),
            "expires_at must be ~30 days from timestamp"
        );
        assert_eq!(diag.retention_days, 30);
    }

    #[test]
    fn diagnostic_affected_fields_for_identity_skip() {
        let pipeline = EventPipeline::new();
        let event = make_event("1.0.0", None, None, None);

        let diag = pipeline
            .process(&event)
            .diagnostic
            .expect("diagnostic must be present");
        assert_eq!(
            diag.affected_fields,
            vec![
                "platform_project_id".to_string(),
                "canonical_path".to_string(),
                "cwd".to_string(),
            ],
            "identity skip must list all three missing fields"
        );
    }
}
