// Contract: EventPipeline and supporting functions
//
// This file defines the interface contracts for the event pipeline,
// platform detection, identity resolution, contract validation,
// path policy, and diagnostics.
// It is a design artifact — NOT compilable source code.
// Implementation will be spread across modules in crates/platforms/src/.

// ============================================================================
// Platform Detection (crates/platforms/src/detection.rs)
// ============================================================================

/// Detect which platform is running.
///
/// Priority order (FR-006):
/// 1. Explicit `platform` field in hook input (if present and non-empty)
/// 2. `MEMVID_PLATFORM` environment variable (if set and non-whitespace)
/// 3. Platform-specific indicators: `OPENCODE=1` → "opencode"
/// 4. Default: "claude"
///
/// The result is always lowercase and trimmed (FR-007).
/// Whitespace-only values at any level are treated as absent.
pub fn detect_platform(input: &HookInput) -> String;

// ============================================================================
// Contract Validation (crates/platforms/src/contract.rs)
// ============================================================================

/// Validate a contract version string against the supported major version.
///
/// Contract (FR-008):
/// - Parses the version string using semver crate
/// - Strips pre-release and build metadata before comparison
/// - Compares only major version against SUPPORTED_CONTRACT_MAJOR
/// - Returns compatible=true if major versions match
/// - Returns compatible=false with reason for mismatch or parse failure
///
/// Never panics; malformed versions return incompatible with reason
/// "invalid_contract_version" (FR-009).
pub fn validate_contract(version_str: &str) -> ContractValidationResult;

// ============================================================================
// Identity Resolution (crates/platforms/src/identity.rs)
// ============================================================================

/// Resolve a project identity from a project context.
///
/// Priority (FR-010):
/// 1. platform_project_id (if present and non-empty) → source: PlatformProjectId
/// 2. canonical_path (if present and non-empty) → use as-is → source: CanonicalPath
/// 3. cwd (if present and non-empty) → use as-is → source: Cwd
/// 4. None of the above → key: None, source: Unresolved
///
/// No filesystem I/O — path strings are used as provided by the caller.
/// Always reports the source used (FR-011).
pub fn resolve_project_identity(context: &ProjectContext) -> ProjectIdentity;

// ============================================================================
// Memory Path Policy (crates/platforms/src/path_policy.rs)
// ============================================================================

/// Resolve the memory file path based on path policy.
///
/// Contract:
/// - Default (no opt-in): legacy path `.agent-brain/mind.mv2` (FR-015, mode: LegacyFirst)
/// - Platform opt-in: platform-namespaced path e.g. `.claude/mind-claude.mv2` (FR-015, mode: PlatformOptIn)
/// - Platform name is sanitized: non-alphanumeric chars (except - and _) replaced with - (FR-016)
/// - Resolved path MUST stay within project_dir (FR-014)
/// - Returns Err with E_PLATFORM_PATH_TRAVERSAL if path escapes project dir
///
/// Does NOT perform filesystem I/O. Returns a PathBuf.
pub fn resolve_memory_path(
    project_dir: &Path,
    platform_name: &str,
    platform_opt_in: bool,
) -> Result<ResolvedMemoryPath, AgentBrainError>;

// ============================================================================
// Diagnostic Record (crates/types/src/diagnostic.rs)
// ============================================================================

/// Create a diagnostic record for a skipped or errored event.
///
/// Contract (FR-019 through FR-022):
/// - Auto-generates id (UUID v4) and timestamp
/// - Deduplicates affected_fields (FR-020)
/// - Caps affected_fields at MAX_DIAGNOSTIC_FIELDS (20) (FR-020)
/// - Sets redacted = true (FR-022)
/// - Sets retention_days = DIAGNOSTIC_RETENTION_DAYS (30) (FR-021)
/// - Computes expires_at = timestamp + retention_days (FR-021)
impl DiagnosticRecord {
    pub fn new(
        platform: String,
        error_type: String,
        affected_fields: Vec<String>,
        severity: DiagnosticSeverity,
    ) -> Self;
}

// ============================================================================
// Event Pipeline (crates/platforms/src/pipeline.rs)
// ============================================================================

/// The central coordination point for event processing.
///
/// Contract (FR-023):
/// 1. Validate the event's contract version
///    - If incompatible → skip with diagnostic (reason from validation)
/// 2. Resolve project identity from event's project_context
///    - If unresolved → skip with diagnostic (reason: "missing_project_identity")
/// 3. If both pass → not skipped, return identity
///
/// NEVER raises an unrecoverable error (SC-005). All failures produce
/// skip results with diagnostics.
pub struct EventPipeline {
    // No state needed — pure function composition
}

impl EventPipeline {
    pub fn new() -> Self;

    /// Process a platform event through validation and identity resolution.
    pub fn process(&self, event: &PlatformEvent) -> PipelineResult;
}
