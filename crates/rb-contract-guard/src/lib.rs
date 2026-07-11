//! `rb-contract-guard`: the CI contract-drift guard (Road-to-Tens W5a.4).
//!
//! The W5a.4 protocol-evolution policy — additive serde-default fields never
//! bump `CONTRACT_VERSION`; breaking changes bump it — used to be enforced by
//! reviewer diligence only. This crate operationalizes it: the shape-defining
//! items of the wire surface (rb-proto messages + the rb-types payload types
//! they embed) and the rb-store on-disk migrations are digested and compared
//! against a checked-in snapshot (`contract-snapshot.toml`). Any drift fails
//! CI until the author records a deliberate decision:
//!
//! - additive, serde-default change (no bump):
//!   `cargo run -p rb-contract-guard -- update --intent additive --note "..."`
//! - breaking change (bump `CONTRACT_VERSION` first):
//!   `cargo run -p rb-contract-guard -- update --intent breaking --note "..."`
//!
//! Detection parses the sources with `syn` and digests normalized item tokens
//! (doc comments stripped, `cfg(test)` items skipped), so comment- and
//! formatting-only edits never trip the guard.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod check;
pub mod extract;
pub mod snapshot;
pub mod update;

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::Path;

/// Which files define the contract surface, relative to the repo root.
#[derive(Debug, Clone)]
pub struct SurfaceConfig {
    /// Rust sources whose type definitions shape the wire contract.
    pub wire_files: Vec<String>,
    /// The file declaring `CONTRACT_VERSION` (also listed in `wire_files`).
    pub version_file: String,
    /// Directory of on-disk schema migrations (`NNN_*.sql`).
    pub migrations_dir: String,
    /// The checked-in snapshot the observed surface is compared against.
    pub snapshot_file: String,
}

impl SurfaceConfig {
    /// The rusty-brain contract surface: rb-proto wire messages, the rb-types
    /// payload shapes they embed (the v2 bump — `MemoryNote.contested` — lived
    /// in rb-types, so rb-proto alone is not enough), and rb-store migrations.
    pub fn default_paths() -> Self {
        Self {
            wire_files: vec![
                "crates/rb-proto/src/messages.rs".to_string(),
                "crates/rb-types/src/change.rs".to_string(),
                "crates/rb-types/src/feedback_kind.rs".to_string(),
                "crates/rb-types/src/job.rs".to_string(),
                "crates/rb-types/src/link.rs".to_string(),
                "crates/rb-types/src/link_type.rs".to_string(),
                "crates/rb-types/src/memory.rs".to_string(),
                "crates/rb-types/src/memory_id.rs".to_string(),
                "crates/rb-types/src/memory_type.rs".to_string(),
                "crates/rb-types/src/namespace.rs".to_string(),
                "crates/rb-types/src/query.rs".to_string(),
            ],
            version_file: "crates/rb-proto/src/messages.rs".to_string(),
            migrations_dir: "crates/rb-store/migrations".to_string(),
            snapshot_file: "contract-snapshot.toml".to_string(),
        }
    }
}

/// The contract surface as observed from the working tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observed {
    /// Value of `rb_proto::CONTRACT_VERSION` parsed from the version file.
    pub contract_version: u32,
    /// `"<file>::<item>"` -> sha256 of the item's normalized tokens.
    pub wire: BTreeMap<String, String>,
    /// Migration file name -> sha256 of its bytes.
    pub migrations: BTreeMap<String, String>,
}

/// Read and digest the configured contract surface under `root`.
///
/// Fails closed: a missing wire file or migrations directory is an error, not
/// an empty observation — a silently unobserved surface would defeat the guard.
pub fn observe(root: &Path, cfg: &SurfaceConfig) -> Result<Observed> {
    let mut wire = BTreeMap::new();
    for rel in &cfg.wire_files {
        let path = root.join(rel);
        let source = std::fs::read_to_string(&path)
            .with_context(|| format!("reading wire file {}", path.display()))?;
        wire.extend(extract::extract_wire_items(rel, &source)?);
    }

    let version_path = root.join(&cfg.version_file);
    let version_source = std::fs::read_to_string(&version_path)
        .with_context(|| format!("reading version file {}", version_path.display()))?;
    let contract_version = extract::parse_contract_version(&version_source)
        .with_context(|| format!("parsing CONTRACT_VERSION from {}", cfg.version_file))?;

    let mig_dir = root.join(&cfg.migrations_dir);
    let mut migrations = BTreeMap::new();
    let entries = std::fs::read_dir(&mig_dir)
        .with_context(|| format!("reading migrations dir {}", mig_dir.display()))?;
    for entry in entries {
        let entry = entry.with_context(|| format!("listing {}", mig_dir.display()))?;
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.ends_with(".sql") {
            continue;
        }
        let bytes = std::fs::read(entry.path())
            .with_context(|| format!("reading migration {}", entry.path().display()))?;
        migrations.insert(name, extract::sha256_hex(&bytes));
    }

    Ok(Observed {
        contract_version,
        wire,
        migrations,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(root: &Path, rel: &str, content: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn tiny_config() -> SurfaceConfig {
        SurfaceConfig {
            wire_files: vec!["src/messages.rs".to_string()],
            version_file: "src/messages.rs".to_string(),
            migrations_dir: "migrations".to_string(),
            snapshot_file: "contract-snapshot.toml".to_string(),
        }
    }

    const MESSAGES: &str = "pub const CONTRACT_VERSION: u32 = 2;\n\
                            pub struct Handshake { pub contract_version: u32 }\n";

    #[test]
    fn observe_reads_wire_items_version_and_migrations() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "src/messages.rs", MESSAGES);
        write(
            dir.path(),
            "migrations/001_init.sql",
            "CREATE TABLE t (id);",
        );
        write(dir.path(), "migrations/README.md", "not sql");

        let obs = observe(dir.path(), &tiny_config()).unwrap();
        assert_eq!(obs.contract_version, 2);
        assert!(obs.wire.contains_key("src/messages.rs::Handshake"));
        assert!(obs.wire.contains_key("src/messages.rs::CONTRACT_VERSION"));
        assert_eq!(obs.migrations.len(), 1, "non-.sql files are ignored");
        assert!(obs.migrations.contains_key("001_init.sql"));
    }

    #[test]
    fn observe_fails_closed_on_missing_wire_file() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "migrations/001_init.sql",
            "CREATE TABLE t (id);",
        );
        let err = observe(dir.path(), &tiny_config()).unwrap_err();
        assert!(
            err.to_string().contains("messages.rs"),
            "error names the missing file, got: {err:#}"
        );
    }

    #[test]
    fn observe_fails_closed_on_missing_migrations_dir() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "src/messages.rs", MESSAGES);
        let err = observe(dir.path(), &tiny_config()).unwrap_err();
        assert!(
            err.to_string().contains("migrations"),
            "error names the missing dir, got: {err:#}"
        );
    }
}
