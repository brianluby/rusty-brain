//! Integration tests for OpenCode agent install flow (T034).
//!
//! Tests the full install pipeline: detect -> generate_config -> write -> validate.

use std::path::PathBuf;

use platforms::installer::AgentInstaller;
use platforms::installer::orchestrator::InstallOrchestrator;
use platforms::installer::registry::InstallerRegistry;
use types::install::{InstallConfig, InstallScope, InstallStatus};

/// Helper: create a fake installer that always detects the agent.
struct FakeOpenCodeInstaller;

impl AgentInstaller for FakeOpenCodeInstaller {
    fn agent_name(&self) -> &'static str {
        "opencode"
    }

    fn detect(&self) -> Option<types::install::AgentInfo> {
        Some(types::install::AgentInfo {
            name: "opencode".to_string(),
            binary_path: PathBuf::from("/usr/local/bin/rusty-brain"),
            version: Some("1.2.3".to_string()),
        })
    }

    fn generate_config(
        &self,
        scope: &InstallScope,
        binary_path: &std::path::Path,
    ) -> Result<Vec<types::install::ConfigFile>, types::install::InstallError> {
        let config_dir = match scope {
            InstallScope::Project { root } => root.join(".opencode").join("plugins"),
            InstallScope::Global => PathBuf::from("/tmp/global/opencode/plugins"),
        };

        let binary_str = binary_path.to_string_lossy();
        let content = serde_json::to_string_pretty(&serde_json::json!({
            "name": "rusty-brain",
            "binary": binary_str.as_ref(),
            "commands": {
                "ask": { "args": ["ask", "--json"] },
                "search": { "args": ["find", "--json"] },
                "recent": { "args": ["timeline", "--json"] },
                "stats": { "args": ["stats", "--json"] }
            }
        }))
        .unwrap();

        Ok(vec![types::install::ConfigFile {
            target_path: config_dir.join("rusty-brain.json"),
            content,
            description: "OpenCode plugin manifest".to_string(),
        }])
    }

    fn validate(&self, scope: &InstallScope) -> Result<(), types::install::InstallError> {
        let config_path = match scope {
            InstallScope::Project { root } => root
                .join(".opencode")
                .join("plugins")
                .join("rusty-brain.json"),
            InstallScope::Global => PathBuf::from("/tmp/global/opencode/plugins/rusty-brain.json"),
        };

        if !config_path.exists() {
            return Err(types::install::InstallError::ConfigCorrupted { path: config_path });
        }
        Ok(())
    }
}

// AC-1: Install creates valid config files.
#[test]
fn install_creates_config_files_in_tempdir() {
    let dir = tempfile::tempdir().unwrap();
    let mut registry = InstallerRegistry::new();
    registry.register(Box::new(FakeOpenCodeInstaller));

    let orch = InstallOrchestrator::new(registry);
    let config = InstallConfig {
        agents: Some(vec!["opencode".to_string()]),
        scope: InstallScope::Project {
            root: dir.path().to_path_buf(),
        },
        json: false,
        reconfigure: false,
    };

    let report = orch.run(&config).unwrap();
    assert_eq!(report.results.len(), 1);
    assert_eq!(report.results[0].status, InstallStatus::Configured);

    // Verify file exists with correct content.
    let config_path = dir
        .path()
        .join(".opencode")
        .join("plugins")
        .join("rusty-brain.json");
    assert!(config_path.exists(), "config file should exist");

    let content = std::fs::read_to_string(&config_path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert_eq!(parsed["name"], "rusty-brain");
}

// AC-2: Slash commands are registered in config.
#[test]
fn install_registers_slash_commands() {
    let dir = tempfile::tempdir().unwrap();
    let mut registry = InstallerRegistry::new();
    registry.register(Box::new(FakeOpenCodeInstaller));

    let orch = InstallOrchestrator::new(registry);
    let config = InstallConfig {
        agents: Some(vec!["opencode".to_string()]),
        scope: InstallScope::Project {
            root: dir.path().to_path_buf(),
        },
        json: false,
        reconfigure: false,
    };

    let report = orch.run(&config).unwrap();
    assert_eq!(report.results[0].status, InstallStatus::Configured);

    let config_path = dir
        .path()
        .join(".opencode")
        .join("plugins")
        .join("rusty-brain.json");
    let content = std::fs::read_to_string(&config_path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
    let commands = parsed["commands"].as_object().unwrap();

    assert!(commands.contains_key("ask"));
    assert!(commands.contains_key("search"));
    assert!(commands.contains_key("recent"));
    assert!(commands.contains_key("stats"));
}

// AC-9: Upgrade preserves data (backup created on re-install).
#[test]
fn upgrade_creates_backup_of_existing_config() {
    let dir = tempfile::tempdir().unwrap();
    let mut registry = InstallerRegistry::new();
    registry.register(Box::new(FakeOpenCodeInstaller));

    let orch = InstallOrchestrator::new(registry);
    let config = InstallConfig {
        agents: Some(vec!["opencode".to_string()]),
        scope: InstallScope::Project {
            root: dir.path().to_path_buf(),
        },
        json: false,
        reconfigure: false,
    };

    // First install.
    let report1 = orch.run(&config).unwrap();
    assert_eq!(report1.results[0].status, InstallStatus::Configured);

    // Second install (should be upgrade with backup).
    let mut registry2 = InstallerRegistry::new();
    registry2.register(Box::new(FakeOpenCodeInstaller));
    let orch2 = InstallOrchestrator::new(registry2);

    let report2 = orch2.run(&config).unwrap();
    assert_eq!(report2.results[0].status, InstallStatus::Upgraded);

    // Verify backup exists.
    let backup_path = dir
        .path()
        .join(".opencode")
        .join("plugins")
        .join("rusty-brain.json.bak");
    assert!(
        backup_path.exists(),
        "backup file should exist after upgrade"
    );
}
