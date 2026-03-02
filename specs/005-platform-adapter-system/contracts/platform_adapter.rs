// Contract: PlatformAdapter trait
//
// This file defines the interface contract for platform adapters.
// It is a design artifact — NOT compilable source code.
// Implementation will mirror this contract in crates/platforms/src/adapter.rs.

use types::hooks::HookInput;
use types::platform_event::PlatformEvent;

/// A platform adapter normalizes raw hook input into typed platform events.
///
/// Each adapter is associated with a specific platform (e.g., "claude", "opencode")
/// and declares its own contract version. The `normalize()` method converts
/// platform-specific hook input into one of three event kinds.
///
/// # Contract
///
/// - `platform_name()` MUST return a lowercase, trimmed string.
/// - `contract_version()` MUST return a valid semver string.
/// - `normalize()` MUST return `None` when:
///   - The session ID is missing or empty (FR-005a)
///   - The event kind is ToolObservation but tool_name is missing (FR-005)
/// - `normalize()` MUST auto-generate event_id (UUID v4) and timestamp (FR-002)
/// - `normalize()` MUST extract ProjectContext from the hook input (FR-003)
/// - `normalize()` MUST NOT panic or return Err; it returns Option<PlatformEvent>.
///
/// # Extensibility (SC-007)
///
/// New platforms are supported by:
/// 1. Implementing this trait
/// 2. Registering the implementation via AdapterRegistry::register()
/// No existing code needs modification.
pub trait PlatformAdapter: Send + Sync {
    /// Returns the lowercase platform name (e.g., "claude", "opencode").
    fn platform_name(&self) -> &str;

    /// Returns the adapter's contract version as a semver string (e.g., "1.0.0").
    fn contract_version(&self) -> &str;

    /// Normalize raw hook input into a typed platform event.
    ///
    /// Returns `None` if the input cannot be normalized (missing session ID,
    /// missing tool name for ToolObservation, etc.).
    fn normalize(&self, input: &HookInput, event_kind_hint: &str) -> Option<PlatformEvent>;
}

/// Factory function for creating built-in adapters.
///
/// Both Claude and OpenCode adapters share the same normalization logic
/// (since their hook input schemas are identical). This factory creates
/// an adapter with the given platform name.
///
/// # Arguments
///
/// * `platform_name` - Must be a non-empty string; will be lowercased.
///
/// # Returns
///
/// A boxed adapter implementing PlatformAdapter.
pub fn create_builtin_adapter(platform_name: &str) -> Box<dyn PlatformAdapter>;
