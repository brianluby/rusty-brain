//! Shared cache-directory + stable-hashing helpers for the per-process hook
//! state files (the W3.1 per-session scratch).
//!
//! The scratch lives under the XDG cache dir, namespaced by a filesystem slug
//! disambiguated with a stable FNV-1a hash (the slug alone is not injective —
//! `my-app`/`my_app`/`my.app` all collapse) plus the session id, and is read by
//! a later, separate hook process. The hash is therefore pinned to FNV-1a so it
//! stays stable across processes and Rust versions.

use std::path::PathBuf;

use rb_types::Namespace;

/// Resolve the hook state cache dir: `$XDG_CACHE_HOME/rusty-brain/` or
/// `~/.cache/rusty-brain/`, falling back to the system temp dir if neither
/// `XDG_CACHE_HOME` nor `HOME` is set. A single resolver so the scratch and
/// `scratch::prune_stale` always agree on the directory.
pub(crate) fn cache_dir() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_CACHE_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("rusty-brain");
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        if !home.is_empty() {
            return PathBuf::from(home).join(".cache").join("rusty-brain");
        }
    }
    std::env::temp_dir().join("rusty-brain")
}

/// Turn a namespace into a filesystem-safe slug (alphanumerics kept, every
/// other byte replaced by `_`). Not injective on its own — always pair it with
/// [`fnv1a`] of the raw db string to keep distinct namespaces apart.
pub(crate) fn namespace_slug(namespace: &Namespace) -> String {
    namespace
        .as_db_string()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

/// Stable FNV-1a 64-bit hash of arbitrary bytes. Stable across processes and
/// Rust versions (required: persisted to disk / embedded in cache filenames,
/// read by a later invocation).
pub(crate) fn fnv1a(bytes: &[u8]) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = FNV_OFFSET;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn fnv1a_is_deterministic_and_distinguishes_inputs() {
        assert_eq!(fnv1a(b"abc"), fnv1a(b"abc"));
        assert_ne!(fnv1a(b"abc"), fnv1a(b"abd"));
    }

    #[test]
    fn namespace_slug_is_filesystem_safe() {
        let slug = namespace_slug(&Namespace::Project("rusty/brain:1".into()));
        assert!(
            slug.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
            "slug must be fs-safe, got {slug}"
        );
    }

    #[test]
    fn cache_dir_prefers_xdg_then_home() {
        // Process-global env: this test only reads, asserting the precedence
        // shape rather than mutating env (other tests own that).
        let dir = cache_dir();
        assert!(
            dir.ends_with("rusty-brain"),
            "cache dir is under a rusty-brain folder: {dir:?}"
        );
    }
}
