// Contract: AgentInstaller Trait
// Feature: 011-agent-installs
// Date: 2026-03-05
//
// This file defines the interface contracts for the install subsystem.
// Implementation MUST conform to these trait signatures.

use std::path::{Path, PathBuf};

// --- Core Types (types crate: src/install.rs) ---

/// Information about a detected agent installation.
#[derive(Debug, Clone)]
pub struct AgentInfo {
    /// Canonical agent name (lowercase): "opencode", "copilot", "codex", "gemini"
    pub name: String,
    /// Absolute path to agent binary on the system
    pub binary_path: PathBuf,
    /// Detected version string, if available
    pub version: Option<String>,
}

/// A configuration file to be written for an agent.
#[derive(Debug, Clone)]
pub struct ConfigFile {
    /// Absolute path where this file should be written
    pub target_path: PathBuf,
    /// File content
    pub content: String,
    /// Human-readable description (for logging and JSON output)
    pub description: String,
}

/// Scope for installation.
#[derive(Debug, Clone)]
pub enum InstallScope {
    /// Config placed relative to project root
    Project { root: PathBuf },
    /// Config placed in user-level directories
    Global,
}

/// Input configuration parsed from CLI args.
#[derive(Debug)]
pub struct InstallConfig {
    /// Explicit agent list; None = auto-detect all
    pub agents: Option<Vec<String>>,
    /// Installation scope (required)
    pub scope: InstallScope,
    /// Force JSON output
    pub json: bool,
    /// Regenerate config files (backup existing)
    pub reconfigure: bool,
    /// Override config directory for the agent
    pub config_dir: Option<PathBuf>,
}

/// Per-agent installation result.
///
/// NOTE: `error` is the Display output of `InstallError` (serialized as string).
/// Error codes are embedded via `[E_INSTALL_*]` prefixes in the message, so
/// consumers can parse the code from the string for programmatic handling.
/// Mapping: `result.error = Some(format!("{install_error}"))`
#[derive(Debug, Clone, serde::Serialize)]
pub struct AgentInstallResult {
    pub agent_name: String,
    pub status: InstallStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version_detected: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Overall installation report.
#[derive(Debug, Clone, serde::Serialize)]
pub struct InstallReport {
    pub status: String,
    pub results: Vec<AgentInstallResult>,
    pub memory_store: PathBuf,
    pub scope: String,
}

/// Installation outcome for a single agent.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallStatus {
    Configured,
    Upgraded,
    Skipped,
    Failed,
    NotFound,
}

/// Stable error types for install operations.
#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("[E_INSTALL_AGENT_NOT_FOUND] Agent '{agent}' not found on this system")]
    AgentNotFound { agent: String },

    #[error("[E_INSTALL_PERMISSION_DENIED] Cannot write to '{path}': {suggestion}")]
    PermissionDenied { path: PathBuf, suggestion: String },

    #[error("[E_INSTALL_UNSUPPORTED_VERSION] Agent '{agent}' version {version} is below minimum {min_version}")]
    UnsupportedVersion {
        agent: String,
        version: String,
        min_version: String,
    },

    #[error("[E_INSTALL_CONFIG_CORRUPTED] Existing config at '{path}' is corrupted")]
    ConfigCorrupted { path: PathBuf },

    #[error("[E_INSTALL_IO_ERROR] I/O error at '{path}': {source}")]
    IoError {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("[E_INSTALL_SCOPE_REQUIRED] Installation scope required: use --project or --global")]
    ScopeRequired,

    #[error("[E_INSTALL_INVALID_AGENT] Unknown agent '{agent}'. Supported: opencode, copilot, codex, gemini")]
    InvalidAgent { agent: String },

    #[error("[E_INSTALL_PATH_TRAVERSAL] Path '{path}' contains traversal sequences")]
    PathTraversal { path: PathBuf },
}

// --- AgentInstaller Trait (platforms crate: src/installer/mod.rs) ---

/// Trait that each agent installer must implement.
///
/// IMPORTANT: `generate_config` MUST be a pure function — no filesystem I/O.
/// All filesystem operations go through `ConfigWriter`.
pub trait AgentInstaller: Send + Sync {
    /// Canonical lowercase agent name: "opencode", "copilot", "codex", "gemini"
    fn agent_name(&self) -> &str;

    /// Detect if this agent is installed on the system.
    ///
    /// Checks PATH for the agent binary and optionally runs `--version`
    /// with a 2-second timeout.
    ///
    /// Returns `None` if the agent is not found.
    fn detect(&self) -> Option<AgentInfo>;

    /// Generate configuration files for this agent.
    ///
    /// This MUST be a pure function: given scope and binary path, return
    /// a list of files to write. No filesystem I/O.
    ///
    /// `binary_path` is the absolute path to the rusty-brain binary.
    fn generate_config(
        &self,
        scope: &InstallScope,
        binary_path: &Path,
    ) -> Result<Vec<ConfigFile>, InstallError>;

    /// Validate that the installation is working (post-install check).
    ///
    /// Checks that config files exist and are valid.
    fn validate(&self, scope: &InstallScope) -> Result<(), InstallError>;
}

// --- InstallerRegistry (platforms crate: src/installer/registry.rs) ---

/// Registry for AgentInstaller implementations.
///
/// Mirrors the pattern of AdapterRegistry in platforms/src/registry.rs.
pub trait InstallerRegistryContract {
    /// Register an installer.
    fn register(&mut self, installer: Box<dyn AgentInstaller>);

    /// Look up an installer by agent name (case-insensitive).
    fn resolve(&self, agent_name: &str) -> Option<&dyn AgentInstaller>;

    /// Return sorted list of all registered agent names.
    fn agents(&self) -> Vec<String>;
}

// --- ConfigWriter (platforms crate: src/installer/writer.rs) ---

/// Contract for atomic config file writing with backup support.
pub trait ConfigWriterContract {
    /// Write a config file atomically (temp file + rename).
    ///
    /// If the target file exists and `backup` is true, creates a `.bak` copy first.
    /// Creates parent directories if they don't exist.
    /// Sets file permissions to 0o644 on Unix.
    fn write(&self, config: &ConfigFile, backup: bool) -> Result<(), InstallError>;

    /// Create a `.bak` backup of an existing file.
    fn backup(&self, path: &Path) -> Result<(), InstallError>;
}

// --- InstallOrchestrator (platforms crate: src/installer/orchestrator.rs) ---

/// Contract for the install workflow coordinator.
pub trait InstallOrchestratorContract {
    /// Execute the full install workflow.
    ///
    /// 1. Determine target agents (from config.agents or auto-detect)
    /// 2. For each agent: detect -> generate_config -> write
    /// 3. Collect results into InstallReport
    ///
    /// Fails per-agent, not per-command: if one agent fails, continues to next.
    fn run(&self, config: InstallConfig) -> Result<InstallReport, InstallError>;
}

// --- CLI Subcommand (cli crate: src/args.rs) ---

// The Install variant added to the Command enum:
//
// Install {
//     /// Comma-separated list of agents to configure
//     #[arg(long, value_delimiter = ',')]
//     agents: Option<Vec<String>>,
//
//     /// Install config relative to current working directory
//     #[arg(long, group = "scope")]
//     project: bool,
//
//     /// Install config in user-level directories
//     #[arg(long, group = "scope")]
//     global: bool,
//
//     /// Force JSON output
//     #[arg(long)]
//     json: bool,
//
//     /// Regenerate config files (backup existing)
//     #[arg(long)]
//     reconfigure: bool,
//
//     /// Override config directory for the agent
//     #[arg(long)]
//     config_dir: Option<PathBuf>,
// }
