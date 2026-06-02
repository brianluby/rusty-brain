//! Report + error-code types for install/uninstall/status, JSON-serializable.
//!
//! Error codes mirror the legacy `[E_INSTALL_*]` scheme so consumers can parse
//! a stable code from the serialized `error` string.

use std::path::PathBuf;

use serde::Serialize;

/// Stable, code-prefixed install errors (the `Display` carries the `[E_*]` code).
#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("[E_INSTALL_AGENT_NOT_FOUND] agent '{agent}' not found on this system")]
    AgentNotFound { agent: String },
    #[error("[E_INSTALL_INVALID_AGENT] unknown agent '{agent}'. supported: claude-code, opencode, gemini, codex")]
    InvalidAgent { agent: String },
    #[error("[E_INSTALL_IO_ERROR] i/o error at '{path}': {message}")]
    IoError { path: PathBuf, message: String },
    #[error("[E_INSTALL_CONFIG_CORRUPTED] existing config at '{path}' is not valid json")]
    ConfigCorrupted { path: PathBuf },
}

/// Per-agent outcome status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Configured,
    Upgraded,
    Removed,
    Present,
    Absent,
    NotFound,
    WouldConfigure,
    WouldRemove,
    Failed,
}

/// Per-agent install/uninstall/status result.
#[derive(Debug, Clone, Serialize)]
pub struct AgentReport {
    pub agent: String,
    pub status: AgentStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Overall run status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportStatus {
    Success,
    Partial,
    Failed,
}

/// The full install/uninstall/status report (JSON root).
#[derive(Debug, Clone, Serialize)]
pub struct InstallReport {
    pub status: ReportStatus,
    pub scope: String,
    pub dry_run: bool,
    pub agents: Vec<AgentReport>,
}

impl InstallReport {
    /// Roll up per-agent statuses into the overall report status.
    #[must_use]
    pub fn roll_up(scope: &str, dry_run: bool, agents: Vec<AgentReport>) -> Self {
        let any_failed = agents.iter().any(|a| a.status == AgentStatus::Failed);
        let any_ok = agents.iter().any(|a| {
            matches!(
                a.status,
                AgentStatus::Configured
                    | AgentStatus::Upgraded
                    | AgentStatus::Removed
                    | AgentStatus::Present
                    | AgentStatus::WouldConfigure
                    | AgentStatus::WouldRemove
            )
        });
        let status = if any_failed && any_ok {
            ReportStatus::Partial
        } else if any_failed {
            ReportStatus::Failed
        } else {
            ReportStatus::Success
        };
        Self {
            status,
            scope: scope.to_string(),
            dry_run,
            agents,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn error_codes_are_stable() {
        assert!(InstallError::AgentNotFound { agent: "x".into() }
            .to_string()
            .contains("[E_INSTALL_AGENT_NOT_FOUND]"));
        assert!(InstallError::InvalidAgent { agent: "x".into() }
            .to_string()
            .contains("[E_INSTALL_INVALID_AGENT]"));
    }

    #[test]
    fn roll_up_success_when_all_ok() {
        let agents = vec![AgentReport {
            agent: "claude-code".into(),
            status: AgentStatus::Configured,
            config_path: Some("/tmp/.claude/settings.json".into()),
            version: Some("1.0.0".into()),
            error: None,
        }];
        let r = InstallReport::roll_up("project", false, agents);
        assert_eq!(r.status, ReportStatus::Success);
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains("\"status\":\"success\""));
        assert!(json.contains("\"agent\":\"claude-code\""));
    }

    #[test]
    fn roll_up_partial_when_mixed() {
        let agents = vec![
            AgentReport {
                agent: "claude-code".into(),
                status: AgentStatus::Configured,
                config_path: None,
                version: None,
                error: None,
            },
            AgentReport {
                agent: "codex".into(),
                status: AgentStatus::Failed,
                config_path: None,
                version: None,
                error: Some("boom".into()),
            },
        ];
        let r = InstallReport::roll_up("project", false, agents);
        assert_eq!(r.status, ReportStatus::Partial);
    }
}
