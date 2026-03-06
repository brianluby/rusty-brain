//! GitHub `Copilot` CLI agent installer.
//!
//! NOTE: `Copilot` CLI extension mechanism requires Spike-1 research (PRD).
//! This is a stub installer that reports the agent as detected but generates
//! a placeholder config until the extension format is confirmed.

use std::path::Path;
use std::process::{Command, Stdio};

use types::install::{AgentInfo, ConfigFile, InstallError, InstallScope};

use crate::installer::{
    AgentInstaller, detect_binary_version, find_binary_on_path, resolve_global_config_dir,
    validate_json_config,
};

/// Installer for the GitHub `Copilot` CLI agent.
pub struct CopilotInstaller;

impl AgentInstaller for CopilotInstaller {
    fn agent_name(&self) -> &'static str {
        "copilot"
    }

    fn detect(&self) -> Option<AgentInfo> {
        // Copilot CLI is accessed via `gh copilot` — detect `gh` binary.
        let binary_path = find_binary_on_path("gh")?;

        // Verify `gh copilot` subcommand is available (gh may exist without copilot).
        let copilot_check = Command::new(&binary_path)
            .args(["help", "copilot"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if !copilot_check.is_ok_and(|s| s.success()) {
            return None;
        }

        let version = detect_binary_version(&binary_path);

        Some(AgentInfo {
            name: "copilot".to_string(),
            binary_path,
            version,
        })
    }

    fn generate_config(
        &self,
        scope: &InstallScope,
        binary_path: &Path,
    ) -> Result<Vec<ConfigFile>, InstallError> {
        let config_dir = match scope {
            InstallScope::Project { root } => root.join(".copilot"),
            InstallScope::Global => resolve_global_config_dir("copilot")?,
        };

        let binary_str = binary_path.to_string_lossy();
        let manifest = generate_copilot_config(&binary_str);

        Ok(vec![ConfigFile {
            target_path: config_dir.join("rusty-brain.json"),
            content: manifest,
            description:
                "Copilot CLI extension config for rusty-brain (stub — awaiting Spike-1 research)"
                    .to_string(),
        }])
    }

    fn validate(&self, scope: &InstallScope) -> Result<(), InstallError> {
        let config_path = match scope {
            InstallScope::Project { root } => root.join(".copilot").join("rusty-brain.json"),
            InstallScope::Global => resolve_global_config_dir("copilot")?.join("rusty-brain.json"),
        };

        validate_json_config(&config_path)
    }
}

fn generate_copilot_config(binary_path: &str) -> String {
    serde_json::to_string_pretty(&serde_json::json!({
        "name": "rusty-brain",
        "description": "AI agent memory system powered by memvid (stub — awaiting Spike-1 research)",
        "binary": binary_path,
        "status": "stub",
        "note": "Copilot CLI extension mechanism not yet confirmed. This config will be updated after Spike-1 research."
    }))
    .expect("JSON serialization should not fail for static data")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // T040: CopilotInstaller::detect() tests

    #[test]
    fn detect_does_not_panic() {
        let installer = CopilotInstaller;
        let _ = installer.detect();
    }

    // T041: CopilotInstaller::generate_config() tests

    #[test]
    fn generate_config_project_scope() {
        let installer = CopilotInstaller;
        let scope = InstallScope::Project {
            root: PathBuf::from("/tmp/project"),
        };
        let binary = PathBuf::from("/usr/local/bin/rusty-brain");

        let files = installer.generate_config(&scope, &binary).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(
            files[0].target_path,
            PathBuf::from("/tmp/project/.copilot/rusty-brain.json")
        );

        let parsed: serde_json::Value = serde_json::from_str(&files[0].content).unwrap();
        assert_eq!(parsed["name"], "rusty-brain");
        assert_eq!(parsed["status"], "stub");
    }
}
