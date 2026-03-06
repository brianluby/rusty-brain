//! Google `Gemini` CLI agent installer.
//!
//! NOTE: `Gemini` CLI extension mechanism requires Spike-3 research (PRD).
//! This is a stub installer until the extension format is confirmed.

use std::path::Path;

use types::install::{AgentInfo, ConfigFile, InstallError, InstallScope};

use crate::installer::{
    AgentInstaller, detect_binary_version, find_binary_on_path, resolve_global_config_dir,
    validate_json_config,
};

/// Installer for the Google `Gemini` CLI agent.
pub struct GeminiInstaller;

impl AgentInstaller for GeminiInstaller {
    fn agent_name(&self) -> &'static str {
        "gemini"
    }

    fn detect(&self) -> Option<AgentInfo> {
        let binary_path = find_binary_on_path("gemini")?;

        let version = detect_binary_version(&binary_path);

        Some(AgentInfo {
            name: "gemini".to_string(),
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
            InstallScope::Project { root } => root.join(".gemini"),
            InstallScope::Global => resolve_global_config_dir("gemini")?,
        };

        let binary_str = binary_path.to_string_lossy();
        let manifest = generate_gemini_config(&binary_str);

        Ok(vec![ConfigFile {
            target_path: config_dir.join("rusty-brain.json"),
            content: manifest,
            description:
                "Gemini CLI extension config for rusty-brain (stub — awaiting Spike-3 research)"
                    .to_string(),
        }])
    }

    fn validate(&self, scope: &InstallScope) -> Result<(), InstallError> {
        let config_path = match scope {
            InstallScope::Project { root } => root.join(".gemini").join("rusty-brain.json"),
            InstallScope::Global => resolve_global_config_dir("gemini")?.join("rusty-brain.json"),
        };

        validate_json_config(&config_path)
    }
}

fn generate_gemini_config(binary_path: &str) -> String {
    serde_json::to_string_pretty(&serde_json::json!({
        "name": "rusty-brain",
        "description": "AI agent memory system powered by memvid (stub — awaiting Spike-3 research)",
        "binary": binary_path,
        "status": "stub",
        "note": "Gemini CLI extension mechanism not yet confirmed. This config will be updated after Spike-3 research."
    }))
    .expect("JSON serialization should not fail for static data")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // T058: GeminiInstaller::detect() tests

    #[test]
    fn detect_does_not_panic() {
        let installer = GeminiInstaller;
        let _ = installer.detect();
    }

    // T059: GeminiInstaller::generate_config() tests

    #[test]
    fn generate_config_project_scope() {
        let installer = GeminiInstaller;
        let scope = InstallScope::Project {
            root: PathBuf::from("/tmp/project"),
        };
        let binary = PathBuf::from("/usr/local/bin/rusty-brain");

        let files = installer.generate_config(&scope, &binary).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(
            files[0].target_path,
            PathBuf::from("/tmp/project/.gemini/rusty-brain.json")
        );

        let parsed: serde_json::Value = serde_json::from_str(&files[0].content).unwrap();
        assert_eq!(parsed["name"], "rusty-brain");
        assert_eq!(parsed["status"], "stub");
    }
}
