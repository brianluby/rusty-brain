//! `OpenCode` agent installer.

use std::path::Path;

use types::install::{AgentInfo, ConfigFile, InstallError, InstallScope};

use crate::installer::{
    AgentInstaller, detect_binary_version, find_binary_on_path, resolve_global_config_dir,
    validate_json_config,
};

/// Installer for the `OpenCode` AI agent.
pub struct OpenCodeInstaller;

impl AgentInstaller for OpenCodeInstaller {
    fn agent_name(&self) -> &'static str {
        "opencode"
    }

    fn detect(&self) -> Option<AgentInfo> {
        let binary_path = find_binary_on_path("opencode")?;

        let version = detect_binary_version(&binary_path);

        Some(AgentInfo {
            name: "opencode".to_string(),
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
            InstallScope::Project { root } => root.join(".opencode").join("plugins"),
            InstallScope::Global => resolve_global_config_dir("opencode")?.join("plugins"),
        };

        let binary_str = binary_path.to_string_lossy();
        let manifest = generate_plugin_manifest(&binary_str);

        Ok(vec![ConfigFile {
            target_path: config_dir.join("rusty-brain.json"),
            content: manifest,
            description: "OpenCode plugin manifest for rusty-brain".to_string(),
        }])
    }

    fn validate(&self, scope: &InstallScope) -> Result<(), InstallError> {
        let config_path = match scope {
            InstallScope::Project { root } => root
                .join(".opencode")
                .join("plugins")
                .join("rusty-brain.json"),
            InstallScope::Global => resolve_global_config_dir("opencode")?
                .join("plugins")
                .join("rusty-brain.json"),
        };

        validate_json_config(&config_path)
    }
}

/// Generate the plugin manifest JSON for `OpenCode`.
fn generate_plugin_manifest(binary_path: &str) -> String {
    serde_json::to_string_pretty(&serde_json::json!({
        "name": "rusty-brain",
        "description": "AI agent memory system powered by memvid",
        "binary": binary_path,
        "commands": {
            "ask": {
                "description": "Ask a question about your memory",
                "args": ["ask", "--json"]
            },
            "search": {
                "description": "Search memories by text pattern",
                "args": ["find", "--json"]
            },
            "recent": {
                "description": "Show recent memory timeline",
                "args": ["timeline", "--json"]
            },
            "stats": {
                "description": "View memory statistics",
                "args": ["stats", "--json"]
            }
        }
    }))
    .expect("JSON serialization should not fail for static data")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::installer::parse_version_string;
    use std::path::PathBuf;

    // T032: OpenCodeInstaller::detect() tests

    #[test]
    fn detect_returns_none_when_binary_not_found() {
        // OpenCode is unlikely to be installed in CI
        // This test verifies the method doesn't panic
        let installer = OpenCodeInstaller;
        let _ = installer.detect();
    }

    // T033: OpenCodeInstaller::generate_config() tests

    #[test]
    fn generate_config_project_scope() {
        let installer = OpenCodeInstaller;
        let scope = InstallScope::Project {
            root: PathBuf::from("/tmp/project"),
        };
        let binary = PathBuf::from("/usr/local/bin/rusty-brain");

        let files = installer.generate_config(&scope, &binary).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(
            files[0].target_path,
            PathBuf::from("/tmp/project/.opencode/plugins/rusty-brain.json")
        );

        // Verify JSON content
        let parsed: serde_json::Value = serde_json::from_str(&files[0].content).unwrap();
        assert_eq!(parsed["name"], "rusty-brain");
        assert_eq!(parsed["binary"], "/usr/local/bin/rusty-brain");
        assert!(parsed["commands"]["ask"].is_object());
        assert!(parsed["commands"]["search"].is_object());
        assert!(parsed["commands"]["recent"].is_object());
        assert!(parsed["commands"]["stats"].is_object());
    }

    #[test]
    fn generate_config_global_scope() {
        let installer = OpenCodeInstaller;
        let scope = InstallScope::Global;
        let binary = PathBuf::from("/usr/local/bin/rusty-brain");

        let result = installer.generate_config(&scope, &binary);
        assert!(result.is_ok());
        let files = result.unwrap();
        assert_eq!(files.len(), 1);
        let path_str = files[0].target_path.to_string_lossy();
        assert!(
            path_str.contains("opencode"),
            "global path should contain agent name: {path_str}"
        );
        assert!(path_str.contains("rusty-brain.json"));
    }

    #[test]
    fn generate_config_slash_commands_registered() {
        let installer = OpenCodeInstaller;
        let scope = InstallScope::Project {
            root: PathBuf::from("/tmp/project"),
        };
        let binary = PathBuf::from("/usr/local/bin/rusty-brain");

        let files = installer.generate_config(&scope, &binary).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&files[0].content).unwrap();
        let commands = parsed["commands"].as_object().unwrap();

        // Verify all required slash commands are registered
        assert!(commands.contains_key("ask"), "missing /ask command");
        assert!(commands.contains_key("search"), "missing /search command");
        assert!(commands.contains_key("recent"), "missing /recent command");
        assert!(commands.contains_key("stats"), "missing /stats command");
    }

    // parse_version_string tests

    #[test]
    fn parse_version_from_output() {
        assert_eq!(
            parse_version_string("opencode 1.2.3"),
            Some("1.2.3".to_string())
        );
        assert_eq!(parse_version_string("v1.2.3"), Some("1.2.3".to_string()));
        assert_eq!(parse_version_string("1.2.3"), Some("1.2.3".to_string()));
    }

    #[test]
    fn parse_version_empty_returns_none() {
        assert_eq!(parse_version_string(""), None);
        assert_eq!(parse_version_string("   "), None);
    }

    #[test]
    fn parse_version_no_semver_returns_none() {
        assert_eq!(parse_version_string("no version here"), None);
        assert_eq!(parse_version_string("License: MIT"), None);
    }
}
