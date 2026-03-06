//! Integration tests for multi-agent install flow (T052-T054, T057).
//!
//! Tests auto-detection, explicit filtering, and shared memory store path.

use std::path::PathBuf;

use platforms::installer::AgentInstaller;
use platforms::installer::orchestrator::InstallOrchestrator;
use platforms::installer::registry::InstallerRegistry;
use types::install::{
    AgentInfo, ConfigFile, InstallConfig, InstallError, InstallScope, InstallStatus,
};

struct FakeInstaller {
    name: String,
    detected: bool,
}

impl FakeInstaller {
    fn detected(name: &str) -> Box<dyn AgentInstaller> {
        Box::new(Self {
            name: name.to_string(),
            detected: true,
        })
    }

    fn not_detected(name: &str) -> Box<dyn AgentInstaller> {
        Box::new(Self {
            name: name.to_string(),
            detected: false,
        })
    }
}

impl AgentInstaller for FakeInstaller {
    #[allow(clippy::unnecessary_literal_bound)]
    fn agent_name(&self) -> &'static str {
        Box::leak(self.name.clone().into_boxed_str())
    }

    fn detect(&self) -> Option<AgentInfo> {
        if self.detected {
            Some(AgentInfo {
                name: self.name.clone(),
                binary_path: PathBuf::from(format!("/usr/bin/{}", self.name)),
                version: Some("1.0.0".to_string()),
            })
        } else {
            None
        }
    }

    fn generate_config(
        &self,
        scope: &InstallScope,
        _binary_path: &std::path::Path,
    ) -> Result<Vec<ConfigFile>, InstallError> {
        let dir = match scope {
            InstallScope::Project { root } => root.clone(),
            InstallScope::Global => PathBuf::from("/tmp/global"),
        };
        Ok(vec![ConfigFile {
            target_path: dir.join(format!(".{}/config.json", self.name)),
            content: format!(r#"{{"agent": "{}"}}"#, self.name),
            description: format!("{} config", self.name),
        }])
    }

    fn validate(&self, _scope: &InstallScope) -> Result<(), InstallError> {
        Ok(())
    }
}

// T052 / AC-5: Multiple agents detected and configured, single memory store path shared.
#[test]
fn auto_detect_configures_multiple_agents() {
    let dir = tempfile::tempdir().unwrap();
    let mut registry = InstallerRegistry::new();
    registry.register(FakeInstaller::detected("opencode"));
    registry.register(FakeInstaller::detected("copilot"));

    let orch = InstallOrchestrator::new(registry);
    let config = InstallConfig {
        agents: None,
        scope: InstallScope::Project {
            root: dir.path().to_path_buf(),
        },
        json: false,
        reconfigure: false,
    };

    let report = orch.run(&config).unwrap();

    let configured: Vec<_> = report
        .results
        .iter()
        .filter(|r| r.status == InstallStatus::Configured)
        .collect();
    assert_eq!(configured.len(), 2, "both agents should be configured");

    // AC-7: All agents share the same memory store path.
    assert_eq!(
        report.memory_store,
        dir.path().join(".rusty-brain").join("mind.mv2")
    );
}

// T053 / AC-6: Explicit --agents filtering configures only specified agents.
#[test]
fn explicit_agents_filters_to_specified_only() {
    let dir = tempfile::tempdir().unwrap();
    let mut registry = InstallerRegistry::new();
    registry.register(FakeInstaller::detected("opencode"));
    registry.register(FakeInstaller::detected("copilot"));

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
    assert_eq!(report.results[0].agent_name, "opencode");
    assert_eq!(report.results[0].status, InstallStatus::Configured);
}

// T054 / AC-10: Missing agent reports not-found, continues for others.
#[test]
fn missing_agent_continues_for_others() {
    let dir = tempfile::tempdir().unwrap();
    let mut registry = InstallerRegistry::new();
    registry.register(FakeInstaller::not_detected("opencode"));
    registry.register(FakeInstaller::detected("copilot"));

    let orch = InstallOrchestrator::new(registry);
    let config = InstallConfig {
        agents: None,
        scope: InstallScope::Project {
            root: dir.path().to_path_buf(),
        },
        json: false,
        reconfigure: false,
    };

    let report = orch.run(&config).unwrap();
    assert_eq!(report.results.len(), 2);

    let not_found: Vec<_> = report
        .results
        .iter()
        .filter(|r| r.status == InstallStatus::NotFound)
        .collect();
    let configured: Vec<_> = report
        .results
        .iter()
        .filter(|r| r.status == InstallStatus::Configured)
        .collect();

    assert_eq!(not_found.len(), 1);
    assert_eq!(configured.len(), 1);
}

// T057 / AC-7: All agent configs reference the same shared memory store path.
#[test]
fn all_agents_share_same_memory_store_path() {
    let dir = tempfile::tempdir().unwrap();
    let mut registry = InstallerRegistry::new();
    registry.register(FakeInstaller::detected("opencode"));
    registry.register(FakeInstaller::detected("copilot"));
    registry.register(FakeInstaller::detected("codex"));

    let orch = InstallOrchestrator::new(registry);
    let config = InstallConfig {
        agents: None,
        scope: InstallScope::Project {
            root: dir.path().to_path_buf(),
        },
        json: false,
        reconfigure: false,
    };

    let report = orch.run(&config).unwrap();
    let expected_store = dir.path().join(".rusty-brain").join("mind.mv2");
    assert_eq!(report.memory_store, expected_store);
    assert_eq!(report.scope, "project");
}
