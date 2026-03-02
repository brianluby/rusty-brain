//! Shared type definitions for the rusty-brain memory system.
//!
//! This crate provides the core domain types used across all rusty-brain crates.
//! It contains no business logic beyond input validation — only data structures,
//! error definitions, and serialization contracts.
//!
//! # Main types
//!
//! - [`Observation`] / [`ObservationMetadata`] / [`ObservationType`] — a single
//!   recorded memory entry with typed classification and extensible metadata.
//! - [`SessionSummary`] — aggregated summary of one agent coding session.
//! - [`InjectedContext`] — the context payload injected into an agent's prompt.
//! - [`MindConfig`] — configuration for the memory engine (paths, limits, flags).
//! - [`MindStats`] — read-only statistics snapshot of the memory store.
//! - [`HookInput`] / [`HookOutput`] — Claude Code hook protocol request/response.
//! - [`AgentBrainError`] — unified error type with stable, machine-parseable codes.
//!
//! All public types derive `Serialize` and `Deserialize` (serde) with camelCase
//! field naming, making them directly usable as JSON wire formats.

/// Memory engine configuration (paths, limits, feature flags).
pub mod config;
/// Context payload injected into an agent's system prompt.
pub mod context;
/// Contract version validation result for platform adapter events.
pub mod contract_version;
/// Diagnostic types for structured, redacted error and warning records.
pub mod diagnostic;
/// Unified error type and stable error code constants.
pub mod error;
/// Claude Code hook protocol request and response types.
pub mod hooks;
/// Observation types representing individual memory entries.
pub mod observation;
/// Platform event types for normalized agent session events.
pub mod platform_event;
/// Project context and identity types for platform adapters.
pub mod project_context;
/// Session summary representing an aggregated coding session.
pub mod session;
/// Read-only statistics snapshot of the memory store.
pub mod stats;

pub use config::MindConfig;
pub use context::InjectedContext;
pub use contract_version::ContractValidationResult;
pub use diagnostic::{DiagnosticRecord, DiagnosticSeverity};
pub use error::{AgentBrainError, RustyBrainError, StorageSource, error_codes};
pub use hooks::{HookInput, HookOutput};
pub use observation::{Observation, ObservationMetadata, ObservationType};
pub use platform_event::{EventKind, PlatformEvent};
pub use project_context::{IdentitySource, ProjectContext, ProjectIdentity};
pub use session::SessionSummary;
pub use stats::MindStats;
