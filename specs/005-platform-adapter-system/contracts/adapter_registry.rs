// Contract: AdapterRegistry
//
// This file defines the interface contract for the adapter registry.
// It is a design artifact — NOT compilable source code.
// Implementation will be in crates/platforms/src/registry.rs.

use crate::adapter::PlatformAdapter;

/// A registry of platform adapters, supporting registration, lookup, and listing.
///
/// # Contract
///
/// - `register()` stores adapters by lowercase platform name (FR-017)
/// - `register()` silently overwrites on duplicate names (last-registered wins)
/// - `resolve()` returns the adapter for a platform name, or None (FR-017)
/// - `resolve()` is case-insensitive (normalizes to lowercase before lookup)
/// - `platforms()` returns all registered platform names in sorted order (FR-017)
///
/// # Thread Safety
///
/// AdapterRegistry is NOT thread-safe. Registration happens at startup;
/// lookup is read-only during event processing. No interior mutability.
pub struct AdapterRegistry {
    // HashMap<String, Box<dyn PlatformAdapter>>
}

impl AdapterRegistry {
    /// Create an empty registry.
    pub fn new() -> Self;

    /// Register an adapter. If an adapter already exists for this platform name,
    /// it is silently replaced (last-registered wins).
    pub fn register(&mut self, adapter: Box<dyn PlatformAdapter>);

    /// Look up an adapter by platform name (case-insensitive).
    /// Returns None if no adapter is registered for the given name.
    pub fn resolve(&self, platform_name: &str) -> Option<&dyn PlatformAdapter>;

    /// Return all registered platform names in sorted (alphabetical) order.
    pub fn platforms(&self) -> Vec<String>;

    /// Create a registry pre-loaded with the built-in Claude and OpenCode adapters.
    pub fn with_builtins() -> Self;
}
