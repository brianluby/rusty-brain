//! Platform adapter system for rusty-brain.
//!
//! This crate provides the multi-platform abstraction layer: adapter trait,
//! contract validation, event pipeline, platform detection, identity resolution,
//! path policy, and diagnostics.

/// Platform adapter trait and built-in adapter factory.
pub mod adapter;
/// Built-in platform adapters (Claude Code, OpenCode).
pub mod adapters;
/// Contract version validation.
pub mod contract;
/// Platform detection from environment and hook input.
pub mod detection;
/// Project identity resolution from project context.
pub mod identity;
/// Memory file path policy resolution.
pub mod path_policy;
/// Event processing pipeline composing validation and identity.
pub mod pipeline;
/// Adapter registry for registration, lookup, and listing.
pub mod registry;

// Public re-exports for ergonomic imports.
pub use adapter::{ADAPTER_CONTRACT_VERSION, PlatformAdapter, create_builtin_adapter};
pub use adapters::{claude_adapter, opencode_adapter};
pub use contract::{SUPPORTED_CONTRACT_MAJOR, validate_contract};
pub use detection::detect_platform;
pub use identity::resolve_project_identity;
pub use path_policy::{
    LEGACY_CLAUDE_MEMORY_PATH, PathMode, ResolvedMemoryPath, format_legacy_path_warning,
    resolve_memory_path,
};
pub use pipeline::{EventPipeline, PipelineResult};
pub use registry::AdapterRegistry;
pub mod bootstrap;
/// Agent installer subsystem for configuring external AI agents.
pub mod installer;
pub use installer::orchestrator::InstallOrchestrator;
pub use installer::registry::InstallerRegistry;
pub use installer::writer::ConfigWriter;
pub use installer::{AgentInstaller, SUPPORTED_AGENTS, find_binary_on_path, is_valid_agent};
