//! Types for the agent install subsystem.
//!
//! Defines error codes, configuration, and result types used by the installer
//! infrastructure in the `platforms` crate and the `install` CLI subcommand.

use std::path::{Path, PathBuf};

use serde::Serialize;

/// Stable error types for install operations.
///
/// Each variant carries an `[E_INSTALL_*]` prefixed code in its `Display` output
/// so consumers can parse the error code from serialized strings.
#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("[E_INSTALL_AGENT_NOT_FOUND] Agent '{agent}' not found on this system")]
    AgentNotFound { agent: String },

    #[error("[E_INSTALL_PERMISSION_DENIED] Cannot write to '{path}': {suggestion}")]
    PermissionDenied { path: PathBuf, suggestion: String },

    #[error(
        "[E_INSTALL_UNSUPPORTED_VERSION] Agent '{agent}' version {version} is below minimum {min_version}"
    )]
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

    #[error(
        "[E_INSTALL_INVALID_AGENT] Unknown agent '{agent}'. Supported: opencode, copilot, codex, gemini"
    )]
    InvalidAgent { agent: String },

    #[error("[E_INSTALL_PATH_TRAVERSAL] Path '{path}' contains traversal sequences")]
    PathTraversal { path: PathBuf },
}

/// Scope for installation.
#[derive(Debug, Clone)]
pub enum InstallScope {
    /// Config placed relative to project root.
    Project { root: PathBuf },
    /// Config placed in user-level directories.
    Global,
}

/// Information about a detected agent installation.
#[derive(Debug, Clone)]
pub struct AgentInfo {
    /// Canonical agent name (lowercase).
    pub name: String,
    /// Absolute path to agent binary on the system.
    pub binary_path: PathBuf,
    /// Detected version string, if available.
    pub version: Option<String>,
}

/// A configuration file to be written for an agent.
#[derive(Debug, Clone)]
pub struct ConfigFile {
    /// Absolute path where this file should be written.
    pub target_path: PathBuf,
    /// File content.
    pub content: String,
    /// Human-readable description (for logging and JSON output).
    pub description: String,
}

/// Input configuration parsed from CLI args.
#[derive(Debug)]
pub struct InstallConfig {
    /// Explicit agent list; None = auto-detect all.
    pub agents: Option<Vec<String>>,
    /// Installation scope (required).
    pub scope: InstallScope,
    /// Force JSON output.
    pub json: bool,
    /// Regenerate config files (backup existing).
    pub reconfigure: bool,
}

/// Installation outcome for a single agent.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InstallStatus {
    Configured,
    Upgraded,
    Skipped,
    Failed,
    NotFound,
}

/// Per-agent installation result.
#[derive(Debug, Clone, Serialize)]
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

/// Overall report status.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReportStatus {
    Success,
    Partial,
    Failed,
}

/// Overall installation report.
#[derive(Debug, Clone, Serialize)]
pub struct InstallReport {
    pub status: ReportStatus,
    pub results: Vec<AgentInstallResult>,
    pub memory_store: PathBuf,
    pub scope: String,
}

impl InstallScope {
    /// Human-readable scope label for reports.
    #[must_use]
    pub fn label(&self) -> &str {
        match self {
            Self::Project { .. } => "project",
            Self::Global => "global",
        }
    }

    /// Resolve the memory store path for this scope.
    ///
    /// # Errors
    ///
    /// Returns [`InstallError::IoError`] when the global scope is used and
    /// neither `HOME` nor `USERPROFILE` environment variables are set.
    pub fn memory_store_path(&self) -> Result<PathBuf, InstallError> {
        match self {
            Self::Project { root } => Ok(root.join(".rusty-brain").join("mind.mv2")),
            Self::Global => {
                let home = std::env::var("HOME")
                    .or_else(|_| std::env::var("USERPROFILE"))
                    .map_err(|_| InstallError::IoError {
                        path: PathBuf::from("~"),
                        source: std::io::Error::new(
                            std::io::ErrorKind::NotFound,
                            "HOME environment variable not set",
                        ),
                    })?;
                Ok(Path::new(&home).join(".rusty-brain").join("mind.mv2"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_error_agent_not_found_display() {
        let err = InstallError::AgentNotFound {
            agent: "opencode".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("[E_INSTALL_AGENT_NOT_FOUND]"));
        assert!(msg.contains("opencode"));
    }

    #[test]
    fn install_error_scope_required_display() {
        let err = InstallError::ScopeRequired;
        let msg = err.to_string();
        assert!(msg.contains("[E_INSTALL_SCOPE_REQUIRED]"));
        assert!(msg.contains("--project"));
        assert!(msg.contains("--global"));
    }

    #[test]
    fn install_error_invalid_agent_display() {
        let err = InstallError::InvalidAgent {
            agent: "vscode".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("[E_INSTALL_INVALID_AGENT]"));
        assert!(msg.contains("vscode"));
    }

    #[test]
    fn install_error_path_traversal_display() {
        let err = InstallError::PathTraversal {
            path: PathBuf::from("/etc/../shadow"),
        };
        let msg = err.to_string();
        assert!(msg.contains("[E_INSTALL_PATH_TRAVERSAL]"));
    }

    #[test]
    fn install_error_permission_denied_display() {
        let err = InstallError::PermissionDenied {
            path: PathBuf::from("/root/.config"),
            suggestion: "Run with appropriate permissions".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("[E_INSTALL_PERMISSION_DENIED]"));
        assert!(msg.contains("/root/.config"));
    }

    #[test]
    fn install_error_io_error_display() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let err = InstallError::IoError {
            path: PathBuf::from("/tmp/test"),
            source: io_err,
        };
        let msg = err.to_string();
        assert!(msg.contains("[E_INSTALL_IO_ERROR]"));
        assert!(msg.contains("/tmp/test"));
    }

    #[test]
    fn install_scope_label() {
        let project = InstallScope::Project {
            root: PathBuf::from("/tmp/project"),
        };
        assert_eq!(project.label(), "project");
        assert_eq!(InstallScope::Global.label(), "global");
    }

    #[test]
    fn install_scope_memory_store_path_project() {
        let scope = InstallScope::Project {
            root: PathBuf::from("/tmp/project"),
        };
        assert_eq!(
            scope.memory_store_path().unwrap(),
            PathBuf::from("/tmp/project/.rusty-brain/mind.mv2")
        );
    }

    #[test]
    fn install_status_serialization() {
        let status = InstallStatus::Configured;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"configured\"");

        let status = InstallStatus::NotFound;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"not_found\"");
    }

    #[test]
    fn agent_install_result_serialization() {
        let result = AgentInstallResult {
            agent_name: "opencode".to_string(),
            status: InstallStatus::Configured,
            config_path: Some(PathBuf::from("/tmp/.opencode/plugins/rusty-brain.json")),
            version_detected: Some("1.2.3".to_string()),
            error: None,
        };
        let json = serde_json::to_string_pretty(&result).unwrap();
        assert!(json.contains("\"agent_name\": \"opencode\""));
        assert!(json.contains("\"status\": \"configured\""));
        assert!(json.contains("\"version_detected\": \"1.2.3\""));
        assert!(!json.contains("\"error\""));
    }

    #[test]
    fn install_report_serialization() {
        let report = InstallReport {
            status: ReportStatus::Success,
            results: vec![],
            memory_store: PathBuf::from("/tmp/.rusty-brain/mind.mv2"),
            scope: "project".to_string(),
        };
        let json = serde_json::to_string_pretty(&report).unwrap();
        assert!(json.contains("\"status\": \"success\""));
        assert!(json.contains("\"scope\": \"project\""));
        assert!(json.contains("\"memory_store\""));
    }
}
