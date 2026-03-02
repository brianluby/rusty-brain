//! Adapter registry for registration, lookup, and listing.
//!
//! [`AdapterRegistry`] provides a central store for [`PlatformAdapter`] instances,
//! supporting case-insensitive lookup by platform name, sorted platform listing,
//! and a convenience constructor that pre-registers all built-in adapters.

use std::collections::HashMap;

use crate::adapter::PlatformAdapter;
use crate::adapters::{claude_adapter, opencode_adapter};

/// A registry of platform adapters, keyed by lowercase platform name.
///
/// Adapters are stored in a `HashMap` and looked up case-insensitively.
/// Duplicate registrations overwrite the previous adapter (last-registered wins).
pub struct AdapterRegistry {
    adapters: HashMap<String, Box<dyn PlatformAdapter>>,
}

impl AdapterRegistry {
    /// Create an empty registry with no registered adapters.
    #[must_use]
    pub fn new() -> Self {
        Self {
            adapters: HashMap::new(),
        }
    }

    /// Create a registry pre-loaded with all built-in adapters (Claude Code, `OpenCode`).
    #[must_use]
    pub fn with_builtins() -> Self {
        let mut registry = Self::new();
        registry.register(claude_adapter());
        registry.register(opencode_adapter());
        registry
    }

    /// Register a platform adapter.
    ///
    /// The adapter's platform name is normalized to lowercase for storage.
    /// If an adapter with the same platform name is already registered,
    /// it is replaced (last-registered wins).
    pub fn register(&mut self, adapter: Box<dyn PlatformAdapter>) {
        let key = adapter.platform_name().to_lowercase();
        self.adapters.insert(key, adapter);
    }

    /// Look up a registered adapter by platform name (case-insensitive).
    ///
    /// Returns `None` if no adapter is registered for the given platform name.
    #[must_use]
    pub fn resolve(&self, platform_name: &str) -> Option<&dyn PlatformAdapter> {
        let key = platform_name.to_lowercase();
        self.adapters.get(&key).map(AsRef::as_ref)
    }

    /// Return a sorted list of all registered platform names.
    #[must_use]
    pub fn platforms(&self) -> Vec<String> {
        let mut names: Vec<String> = self.adapters.keys().cloned().collect();
        names.sort();
        names
    }
}

impl Default for AdapterRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::create_builtin_adapter;

    // -------------------------------------------------------------------------
    // T021: AdapterRegistry tests
    // -------------------------------------------------------------------------

    #[test]
    fn resolve_returns_registered_adapter_for_claude() {
        let mut registry = AdapterRegistry::new();
        registry.register(create_builtin_adapter("claude"));

        let adapter = registry
            .resolve("claude")
            .expect("resolve must return Some for registered 'claude'");
        assert_eq!(adapter.platform_name(), "claude");
    }

    #[test]
    fn resolve_returns_none_for_unregistered_platform() {
        let registry = AdapterRegistry::new();

        assert!(
            registry.resolve("unknown").is_none(),
            "resolve must return None for unregistered platform"
        );
    }

    #[test]
    fn platforms_returns_sorted_list() {
        let mut registry = AdapterRegistry::new();
        // Register in reverse alphabetical order to verify sorting.
        registry.register(create_builtin_adapter("opencode"));
        registry.register(create_builtin_adapter("claude"));

        assert_eq!(
            registry.platforms(),
            vec!["claude".to_string(), "opencode".to_string()],
            "platforms() must return a sorted list"
        );
    }

    #[test]
    fn duplicate_registration_overwrites() {
        let mut registry = AdapterRegistry::new();
        registry.register(create_builtin_adapter("claude"));
        registry.register(create_builtin_adapter("claude"));

        // After two registrations of "claude", resolve still works
        // and platforms() contains exactly one entry.
        let adapter = registry
            .resolve("claude")
            .expect("resolve must return Some after duplicate registration");
        assert_eq!(adapter.platform_name(), "claude");
        assert_eq!(
            registry.platforms().len(),
            1,
            "duplicate registration must overwrite, not accumulate"
        );
    }

    #[test]
    fn resolve_is_case_insensitive() {
        let mut registry = AdapterRegistry::new();
        registry.register(create_builtin_adapter("claude"));

        // Look up with various casings.
        assert!(
            registry.resolve("Claude").is_some(),
            "resolve must be case-insensitive (Title case)"
        );
        assert!(
            registry.resolve("CLAUDE").is_some(),
            "resolve must be case-insensitive (UPPER case)"
        );
        assert!(
            registry.resolve("cLaUdE").is_some(),
            "resolve must be case-insensitive (mixed case)"
        );
    }

    #[test]
    fn with_builtins_pre_registers_claude_and_opencode() {
        let registry = AdapterRegistry::with_builtins();

        assert!(
            registry.resolve("claude").is_some(),
            "with_builtins must pre-register claude"
        );
        assert!(
            registry.resolve("opencode").is_some(),
            "with_builtins must pre-register opencode"
        );
        assert_eq!(
            registry.platforms(),
            vec!["claude".to_string(), "opencode".to_string()],
            "with_builtins must register exactly claude and opencode"
        );
    }

    #[test]
    fn new_creates_empty_registry() {
        let registry = AdapterRegistry::new();

        assert!(
            registry.platforms().is_empty(),
            "new() must create an empty registry"
        );
        assert!(
            registry.resolve("claude").is_none(),
            "empty registry must resolve nothing"
        );
    }
}
