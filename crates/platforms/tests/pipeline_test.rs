//! Tests for [`platforms::EventPipeline`] — contract validation, identity
//! resolution, pipeline result serde, and diagnostic record properties.
//!
//! Moved from `crates/platforms/src/pipeline.rs` inline tests (RB-ARCH-009).

use chrono::Utc;
use platforms::{EventPipeline, PipelineResult};
use types::{
    DiagnosticRecord, DiagnosticSeverity, EventKind, IdentitySource, PlatformEvent, ProjectContext,
    ProjectIdentity,
};
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
// T023-1: Valid event -- compatible contract + resolvable identity
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
// T023-2: Incompatible contract version -> skipped
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
// T023-3: Compatible contract but unresolvable identity -> skipped
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
// T023-4: Malformed contract version -> skipped
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
    // Both contract and identity are invalid -- contract should be checked first
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
// T027: Integration tests -- diagnostic records from pipeline skip cases
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
