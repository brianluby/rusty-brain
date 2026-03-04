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
use types::{DiagnosticRecord, DiagnosticSeverity, PlatformEvent, ProjectIdentity};

use crate::contract::{REASON_UNKNOWN_CONTRACT_ERROR, validate_contract};
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
                .unwrap_or_else(|| REASON_UNKNOWN_CONTRACT_ERROR.to_string());
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
        if identity.key.is_none() {
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
