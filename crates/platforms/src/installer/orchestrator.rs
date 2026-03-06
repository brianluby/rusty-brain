//! Install workflow coordinator.
//!
//! The [`InstallOrchestrator`] iterates over target agents, delegates to
//! [`AgentInstaller`] implementations, and uses [`ConfigWriter`] for all
//! filesystem operations. Fails per-agent, not per-command.

use std::path::{Path, PathBuf};

use types::install::{
    AgentInstallResult, InstallConfig, InstallError, InstallReport, InstallScope, InstallStatus,
    ReportStatus,
};

use super::is_valid_agent;
use super::registry::InstallerRegistry;
use super::writer::ConfigWriter;

/// Coordinates the full install workflow for [`InstallConfig`].
pub struct InstallOrchestrator {
    registry: InstallerRegistry,
}

impl InstallOrchestrator {
    /// Create a new orchestrator with the given registry.
    #[must_use]
    pub fn new(registry: InstallerRegistry) -> Self {
        Self { registry }
    }

    /// Create a new orchestrator with all built-in installers.
    #[must_use]
    pub fn with_builtins() -> Self {
        Self::new(InstallerRegistry::with_builtins())
    }

    /// Execute the full install workflow.
    ///
    /// 1. Determine target agents (from `config.agents` or auto-detect)
    /// 2. For each agent: detect -> `generate_config` -> write
    /// 3. Collect results into [`InstallReport`]
    ///
    /// Fails per-agent, not per-command.
    ///
    /// # Errors
    ///
    /// Returns [`InstallError::InvalidAgent`] if an explicitly requested agent name
    /// is not in the supported allowlist.
    pub fn run(&self, config: &InstallConfig) -> Result<InstallReport, InstallError> {
        let memory_store = config.scope.memory_store_path()?;
        let binary_path = resolve_binary_path();

        // Determine which agents to process.
        let target_agents = match &config.agents {
            Some(agents) => {
                // Validate agent names against allowlist (SEC-5).
                for agent in agents {
                    if !is_valid_agent(agent) {
                        return Err(InstallError::InvalidAgent {
                            agent: agent.clone(),
                        });
                    }
                }
                agents.clone()
            }
            None => {
                // Auto-detect: iterate all registered installers.
                self.registry.agents()
            }
        };

        let mut results = Vec::new();

        for agent_name in &target_agents {
            let result =
                self.install_agent(agent_name, &config.scope, &binary_path, config.reconfigure);
            results.push(result);
        }

        let status = if results.iter().all(|r| {
            matches!(
                r.status,
                InstallStatus::Configured | InstallStatus::Upgraded | InstallStatus::Skipped
            )
        }) {
            ReportStatus::Success
        } else if results.iter().any(|r| {
            matches!(
                r.status,
                InstallStatus::Configured | InstallStatus::Upgraded
            )
        }) {
            ReportStatus::Partial
        } else {
            ReportStatus::Failed
        };

        Ok(InstallReport {
            status,
            results,
            memory_store,
            scope: config.scope.label().to_string(),
        })
    }

    fn install_agent(
        &self,
        agent_name: &str,
        scope: &InstallScope,
        binary_path: &Path,
        reconfigure: bool,
    ) -> AgentInstallResult {
        let Some(installer) = self.registry.resolve(agent_name) else {
            return AgentInstallResult {
                agent_name: agent_name.to_string(),
                status: InstallStatus::NotFound,
                config_path: None,
                version_detected: None,
                error: Some(
                    InstallError::AgentNotFound {
                        agent: agent_name.to_string(),
                    }
                    .to_string(),
                ),
            };
        };

        // Detect agent.
        let Some(agent_info) = installer.detect() else {
            return AgentInstallResult {
                agent_name: agent_name.to_string(),
                status: InstallStatus::NotFound,
                config_path: None,
                version_detected: None,
                error: None,
            };
        };

        // Generate config files (pure, no I/O).
        let config_files = match installer.generate_config(scope, binary_path) {
            Ok(files) => files,
            Err(e) => {
                return AgentInstallResult {
                    agent_name: agent_name.to_string(),
                    status: InstallStatus::Failed,
                    config_path: None,
                    version_detected: agent_info.version.clone(),
                    error: Some(e.to_string()),
                };
            }
        };

        // Write config files via ConfigWriter.
        let first_config_path = config_files.first().map(|f| f.target_path.clone());
        let mut is_upgrade = false;

        for config_file in &config_files {
            if config_file.target_path.exists() {
                is_upgrade = true;
            }
            let backup = reconfigure || config_file.target_path.exists();
            if let Err(e) = ConfigWriter::write(config_file, backup) {
                return AgentInstallResult {
                    agent_name: agent_name.to_string(),
                    status: InstallStatus::Failed,
                    config_path: first_config_path,
                    version_detected: agent_info.version.clone(),
                    error: Some(e.to_string()),
                };
            }
        }

        let status = if is_upgrade {
            InstallStatus::Upgraded
        } else {
            InstallStatus::Configured
        };

        AgentInstallResult {
            agent_name: agent_name.to_string(),
            status,
            config_path: first_config_path,
            version_detected: agent_info.version,
            error: None,
        }
    }
}

/// Resolve the path to the rusty-brain binary.
fn resolve_binary_path() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("rusty-brain"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::installer::AgentInstaller;
    use std::path::Path;
    use types::install::{AgentInfo, ConfigFile, InstallScope};

    struct FakeInstaller {
        name: &'static str,
        detected: bool,
    }

    impl FakeInstaller {
        fn detected(name: &'static str) -> Box<dyn AgentInstaller> {
            Box::new(Self {
                name,
                detected: true,
            })
        }

        fn not_detected(name: &'static str) -> Box<dyn AgentInstaller> {
            Box::new(Self {
                name,
                detected: false,
            })
        }
    }

    impl AgentInstaller for FakeInstaller {
        fn agent_name(&self) -> &'static str {
            self.name
        }

        fn detect(&self) -> Option<AgentInfo> {
            if self.detected {
                Some(AgentInfo {
                    name: self.name.to_string(),
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
            _binary_path: &Path,
        ) -> Result<Vec<ConfigFile>, InstallError> {
            let dir = match scope {
                InstallScope::Project { root } => root.clone(),
                InstallScope::Global => PathBuf::from("/tmp/global"),
            };
            let name = self.name;
            Ok(vec![ConfigFile {
                target_path: dir.join(format!(".{name}/config.json")),
                content: format!(r#"{{"agent": "{name}"}}"#),
                description: format!("{name} config"),
            }])
        }

        fn validate(&self, _scope: &InstallScope) -> Result<(), InstallError> {
            Ok(())
        }
    }

    // T024: InstallOrchestrator tests

    #[test]
    fn auto_detect_flow() {
        let dir = tempfile::tempdir().unwrap();
        let mut registry = InstallerRegistry::new();
        registry.register(FakeInstaller::detected("opencode"));
        registry.register(FakeInstaller::not_detected("copilot"));

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
        // One configured, one not found
        let configured: Vec<_> = report
            .results
            .iter()
            .filter(|r| r.status == InstallStatus::Configured)
            .collect();
        let not_found: Vec<_> = report
            .results
            .iter()
            .filter(|r| r.status == InstallStatus::NotFound)
            .collect();

        assert_eq!(configured.len(), 1);
        assert_eq!(not_found.len(), 1);
    }

    #[test]
    fn explicit_agent_list() {
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

    #[test]
    fn invalid_agent_name_returns_error() {
        let registry = InstallerRegistry::new();
        let orch = InstallOrchestrator::new(registry);
        let config = InstallConfig {
            agents: Some(vec!["unknown_agent".to_string()]),
            scope: InstallScope::Project {
                root: PathBuf::from("/tmp"),
            },
            json: false,
            reconfigure: false,
        };

        let result = orch.run(&config);
        assert!(result.is_err());
        match result.unwrap_err() {
            InstallError::InvalidAgent { agent } => {
                assert_eq!(agent, "unknown_agent");
            }
            other => panic!("Expected InvalidAgent, got: {other:?}"),
        }
    }

    #[test]
    fn agent_not_found_continues_to_next() {
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
        // Should have both results regardless of detection status
    }

    #[test]
    fn report_contains_memory_store_path() {
        let dir = tempfile::tempdir().unwrap();
        let registry = InstallerRegistry::new();
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
        assert_eq!(
            report.memory_store,
            dir.path().join(".rusty-brain").join("mind.mv2")
        );
        assert_eq!(report.scope, "project");
    }
}
