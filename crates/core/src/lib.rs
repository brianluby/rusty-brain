//! Core memory engine (Mind) for rusty-brain.
//!
//! This crate provides the [`Mind`] struct — the central API for storing,
//! searching, and retrieving observations from a memvid-backed `.mv2` memory
//! file. It also exposes [`estimate_tokens`] for token budget estimation and
//! [`get_mind`] / [`reset_mind`] for singleton access patterns.

mod backend;
mod context_builder;
mod file_guard;
mod memvid_store;
pub mod mind;
pub mod token;

use std::sync::{Arc, Mutex};

use mind::Mind;
use types::{MindConfig, RustyBrainError, error_codes};

/// Global singleton holding the shared `Mind` instance.
static MIND_INSTANCE: Mutex<Option<Arc<Mind>>> = Mutex::new(None);

/// Get or create the shared `Mind` singleton.
///
/// First call opens the mind with the given config. Subsequent calls return
/// the same `Arc<Mind>` regardless of config (the config is ignored if an
/// instance already exists). Use [`reset_mind`] to clear the instance.
///
/// # Errors
///
/// Returns `RustyBrainError` if the first call fails to open the mind, or
/// if the internal mutex is poisoned.
pub fn get_mind(config: MindConfig) -> Result<Arc<Mind>, RustyBrainError> {
    let mut guard = MIND_INSTANCE.lock().map_err(|_| RustyBrainError::Lock {
        code: error_codes::E_LOCK_ACQUISITION_FAILED,
        message: "mind singleton mutex poisoned".to_string(),
    })?;

    if let Some(ref existing) = *guard {
        tracing::debug!("mind singleton already initialized, ignoring config");
        return Ok(Arc::clone(existing));
    }

    let mind = Arc::new(Mind::open(config)?);
    *guard = Some(Arc::clone(&mind));
    Ok(mind)
}

/// Clear the global `Mind` singleton, allowing a fresh instance on next
/// [`get_mind`] call. Primarily used in tests.
pub fn reset_mind() {
    if let Ok(mut guard) = MIND_INSTANCE.lock() {
        *guard = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // T055: get_mind / reset_mind singleton
    //
    // Combined into a single test to avoid races — both tests mutate the
    // global MIND_INSTANCE and cargo runs tests in parallel by default.
    // =========================================================================

    #[test]
    fn singleton_get_and_reset() {
        reset_mind();

        // Part 1: get_mind creates instance and returns same Arc.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("singleton.mv2");
        let config = MindConfig {
            memory_path: path.clone(),
            ..MindConfig::default()
        };

        let mind1 = get_mind(config.clone()).unwrap();
        assert!(mind1.is_initialized());

        let mind2 = get_mind(config).unwrap();
        assert!(Arc::ptr_eq(&mind1, &mind2), "should return same Arc");

        // Part 2: reset_mind clears instance.
        reset_mind();

        let dir2 = tempfile::tempdir().unwrap();
        let path2 = dir2.path().join("reset_test.mv2");
        let config2 = MindConfig {
            memory_path: path2,
            ..MindConfig::default()
        };

        let mind3 = get_mind(config2).unwrap();
        assert!(
            !Arc::ptr_eq(&mind1, &mind3),
            "should be new instance after reset"
        );

        reset_mind();
    }
}
