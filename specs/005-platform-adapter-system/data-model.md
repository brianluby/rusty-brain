# Data Model: Platform Adapter System

**Feature**: 005-platform-adapter-system | **Date**: 2026-03-01

## Entity Relationship Overview

```text
HookInput ─────→ PlatformAdapter::normalize() ─────→ Option<PlatformEvent>
                        │                                     │
                        │                                     ├─ event_id: Uuid
                        │                                     ├─ timestamp: DateTime<Utc>
                        │                                     ├─ platform: String
                        │                                     ├─ contract_version: String
                        │                                     ├─ session_id: String
                        │                                     ├─ project_context: ProjectContext
                        │                                     └─ kind: EventKind
                        │
detect_platform() ──→ AdapterRegistry::resolve() ──→ Option<&dyn PlatformAdapter>
                        │
PlatformEvent ────→ EventPipeline::process() ────→ PipelineResult
                        │                              │
                        ├─ validate_contract()          ├─ skipped: bool
                        └─ resolve_identity()           ├─ reason: Option<String>
                                                        ├─ identity: Option<ProjectIdentity>
                                                        └─ diagnostic: Option<DiagnosticRecord>
```

## Types (in `crates/types/src/`)

### EventKind

```rust
// platform_event.rs
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    SessionStart,
    ToolObservation { tool_name: String },
    SessionStop,
}
```

**Fields**: Variant-specific payloads inline. `ToolObservation` carries the `tool_name` (required by spec — normalization returns None if missing).

**Validation**: None at the enum level; `tool_name` presence is validated during normalization.

### PlatformEvent

```rust
// platform_event.rs
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformEvent {
    pub event_id: Uuid,
    pub timestamp: DateTime<Utc>,
    pub platform: String,
    pub contract_version: String,
    pub session_id: String,
    pub project_context: ProjectContext,
    pub kind: EventKind,
}
```

**Fields**:
- `event_id`: UUID v4, auto-generated (FR-002)
- `timestamp`: UTC, auto-generated (FR-002)
- `platform`: Lowercase platform name (FR-004)
- `contract_version`: Semver string declared by the adapter (FR-004)
- `session_id`: From raw hook input (FR-005a — required, None if missing)
- `project_context`: Extracted from hook input (FR-003)
- `kind`: One of three event kinds (FR-001)

**Relationships**: Contains `ProjectContext`. Consumed by `EventPipeline`.

### ProjectContext

```rust
// project_context.rs
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectContext {
    pub platform_project_id: Option<String>,
    pub canonical_path: Option<String>,
    pub cwd: Option<String>,
}
```

**Fields**: All optional. Populated from hook input during normalization (FR-003).

**Validation**: None at struct level; identity resolution handles missing fields.

### ProjectIdentity

```rust
// project_context.rs
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectIdentity {
    pub key: Option<String>,
    pub source: IdentitySource,
}

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentitySource {
    PlatformProjectId,
    CanonicalPath,
    Cwd,
    Unresolved,
}
```

**Fields**:
- `key`: The resolved identity string, or None if unresolved (FR-010)
- `source`: Which resolution method was used (FR-011)

**State transitions**: N/A — immutable once resolved.

### ContractValidationResult

```rust
// contract_version.rs
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContractValidationResult {
    pub compatible: bool,
    pub reason: Option<String>,
}
```

**Fields**:
- `compatible`: true if major versions match (FR-008)
- `reason`: e.g. "incompatible_contract_major", "invalid_contract_version" (FR-009)

### DiagnosticRecord

```rust
// diagnostic.rs
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticRecord {
    pub id: Uuid,
    pub timestamp: DateTime<Utc>,
    pub platform: String,
    pub error_type: String,
    pub affected_fields: Vec<String>,
    pub severity: DiagnosticSeverity,
    pub redacted: bool,
    pub retention_days: u32,
    pub expires_at: DateTime<Utc>,
}

#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticSeverity {
    Info,
    Warning,
    Error,
}
```

**Fields**:
- `id`: UUID v4, auto-generated (FR-019)
- `timestamp`: UTC, auto-generated (FR-019)
- `platform`: Which platform produced the event (FR-019)
- `error_type`: Machine-readable error category (FR-019)
- `affected_fields`: Deduplicated, max 20 (FR-020)
- `severity`: Info/Warning/Error (FR-019)
- `redacted`: Always true by default (FR-022)
- `retention_days`: Always 30 (FR-021)
- `expires_at`: timestamp + 30 days (FR-021)

**Validation**: `DiagnosticRecord::new()` deduplicates `affected_fields`, caps at 20, sets `redacted = true`, computes `expires_at`.

### ResolvedMemoryPath

```rust
// (in platforms crate, not types — it's behavioral output)
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedMemoryPath {
    pub path: PathBuf,
    pub mode: PathMode,
}

#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathMode {
    LegacyFirst,
    PlatformOptIn,
}
```

**Fields**:
- `path`: Absolute resolved path within project directory (FR-013)
- `mode`: Which policy was applied (FR-015)

**Validation**: Path traversal check during resolution (FR-014).

### PipelineResult

```rust
// (in platforms crate)
#[derive(Debug, Clone, PartialEq)]
pub struct PipelineResult {
    pub skipped: bool,
    pub reason: Option<String>,
    pub identity: Option<ProjectIdentity>,
    pub diagnostic: Option<DiagnosticRecord>,
}
```

**Fields**: Composition of contract validation + identity resolution outcomes (FR-023).

## Error Codes (additions to `types::error::error_codes`)

```rust
pub const E_PLATFORM_INCOMPATIBLE_CONTRACT: &str = "E_PLATFORM_INCOMPATIBLE_CONTRACT";
pub const E_PLATFORM_INVALID_CONTRACT_VERSION: &str = "E_PLATFORM_INVALID_CONTRACT_VERSION";
pub const E_PLATFORM_MISSING_SESSION_ID: &str = "E_PLATFORM_MISSING_SESSION_ID";
pub const E_PLATFORM_MISSING_PROJECT_IDENTITY: &str = "E_PLATFORM_MISSING_PROJECT_IDENTITY";
pub const E_PLATFORM_PATH_TRAVERSAL: &str = "E_PLATFORM_PATH_TRAVERSAL";
pub const E_PLATFORM_ADAPTER_NOT_FOUND: &str = "E_PLATFORM_ADAPTER_NOT_FOUND";
```

## Constants

```rust
// In platforms crate
pub const SUPPORTED_CONTRACT_MAJOR: u64 = 1;
pub const ADAPTER_CONTRACT_VERSION: &str = "1.0.0";
pub const DIAGNOSTIC_RETENTION_DAYS: u32 = 30;
pub const MAX_DIAGNOSTIC_FIELDS: usize = 20;
pub const DEFAULT_LEGACY_PATH: &str = ".agent-brain/mind.mv2";
pub const DEFAULT_PLATFORM: &str = "claude";
```
