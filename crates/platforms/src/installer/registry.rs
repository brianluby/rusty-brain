//! Installer registry for registration, lookup, and listing.
//!
//! Mirrors the [`AdapterRegistry`](crate::registry::AdapterRegistry) pattern.

use std::collections::HashMap;

use super::AgentInstaller;
use super::agents;

/// A registry of agent installers, keyed by lowercase agent name.
pub struct InstallerRegistry {
    installers: HashMap<String, Box<dyn AgentInstaller>>,
}

impl InstallerRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            installers: HashMap::new(),
        }
    }

    /// Create a registry pre-loaded with all built-in agent installers.
    #[must_use]
    pub fn with_builtins() -> Self {
        let mut registry = Self::new();
        registry.register(agents::opencode_installer());
        registry.register(agents::copilot_installer());
        registry.register(agents::codex_installer());
        registry.register(agents::gemini_installer());
        registry
    }

    /// Register an installer.
    pub fn register(&mut self, installer: Box<dyn AgentInstaller>) {
        let key = installer.agent_name().to_lowercase();
        self.installers.insert(key, installer);
    }

    /// Look up an installer by agent name (case-insensitive).
    #[must_use]
    pub fn resolve(&self, agent_name: &str) -> Option<&dyn AgentInstaller> {
        let key = agent_name.to_lowercase();
        self.installers.get(&key).map(AsRef::as_ref)
    }

    /// Return sorted list of all registered agent names.
    #[must_use]
    pub fn agents(&self) -> Vec<String> {
        let mut names: Vec<String> = self.installers.keys().cloned().collect();
        names.sort();
        names
    }

    /// Return an iterator over all registered installers.
    pub fn iter(&self) -> impl Iterator<Item = &dyn AgentInstaller> {
        self.installers.values().map(AsRef::as_ref)
    }
}

impl Default for InstallerRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use types::install::{AgentInfo, ConfigFile, InstallError, InstallScope};

    struct MockInstaller {
        name: &'static str,
    }

    impl MockInstaller {
        fn new(name: &'static str) -> Box<dyn AgentInstaller> {
            Box::new(Self { name })
        }
    }

    impl AgentInstaller for MockInstaller {
        fn agent_name(&self) -> &'static str {
            self.name
        }
        fn detect(&self) -> Option<AgentInfo> {
            None
        }
        fn generate_config(
            &self,
            _scope: &InstallScope,
            _binary_path: &Path,
        ) -> Result<Vec<ConfigFile>, InstallError> {
            Ok(vec![])
        }
        fn validate(&self, _scope: &InstallScope) -> Result<(), InstallError> {
            Ok(())
        }
    }

    // T022: InstallerRegistry tests

    #[test]
    fn register_and_resolve() {
        let mut registry = InstallerRegistry::new();
        registry.register(MockInstaller::new("opencode"));

        let installer = registry.resolve("opencode");
        assert!(installer.is_some());
        assert_eq!(installer.unwrap().agent_name(), "opencode");
    }

    #[test]
    fn resolve_case_insensitive() {
        let mut registry = InstallerRegistry::new();
        registry.register(MockInstaller::new("opencode"));

        assert!(registry.resolve("OpenCode").is_some());
        assert!(registry.resolve("OPENCODE").is_some());
        assert!(registry.resolve("opencode").is_some());
    }

    #[test]
    fn resolve_unknown_returns_none() {
        let registry = InstallerRegistry::new();
        assert!(registry.resolve("unknown").is_none());
    }

    #[test]
    fn agents_returns_sorted_list() {
        let mut registry = InstallerRegistry::new();
        registry.register(MockInstaller::new("codex"));
        registry.register(MockInstaller::new("opencode"));
        registry.register(MockInstaller::new("copilot"));

        let agents = registry.agents();
        assert_eq!(agents, vec!["codex", "copilot", "opencode"]);
    }

    #[test]
    fn new_creates_empty_registry() {
        let registry = InstallerRegistry::new();
        assert!(registry.agents().is_empty());
    }

    #[test]
    fn with_builtins_registers_all_agents() {
        let registry = InstallerRegistry::with_builtins();
        let agents = registry.agents();
        assert!(agents.contains(&"opencode".to_string()));
        assert!(agents.contains(&"copilot".to_string()));
        assert!(agents.contains(&"codex".to_string()));
        assert!(agents.contains(&"gemini".to_string()));
    }
}
