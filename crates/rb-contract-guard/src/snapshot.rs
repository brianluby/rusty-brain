//! The checked-in snapshot (`contract-snapshot.toml`): recorded contract
//! version, per-item wire digests, migration digests, and an append-only log
//! of deliberate contract decisions (the reviewable "intent markers").

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// The deliberate decision recorded with a snapshot update.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Intent {
    /// Baseline snapshot (only ever written by `init`).
    Initial,
    /// Additive, serde-default-compatible change: NO `CONTRACT_VERSION` bump.
    Additive,
    /// Breaking wire change: `CONTRACT_VERSION` was bumped.
    Breaking,
}

/// One append-only log entry: the marker reviewers see in the PR diff.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    pub contract_version: u32,
    pub intent: Intent,
    pub note: String,
}

/// The full checked-in snapshot.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub contract_version: u32,
    pub wire: BTreeMap<String, String>,
    pub migrations: BTreeMap<String, String>,
    #[serde(default)]
    pub log: Vec<LogEntry>,
}

/// Header prepended to the generated file so nobody edits it by hand.
const HEADER: &str = "\
# Contract-drift guard snapshot (rb-contract-guard). DO NOT EDIT BY HAND.\n\
# Regenerate after a deliberate contract decision (W5a.4):\n\
#   cargo run -p rb-contract-guard -- update --intent additive --note \"...\"   # serde-default addition, no bump\n\
#   cargo run -p rb-contract-guard -- update --intent breaking --note \"...\"   # after bumping CONTRACT_VERSION\n";

/// Load a snapshot from `path`.
pub fn load(path: &Path) -> Result<Snapshot> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading snapshot {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parsing snapshot {}", path.display()))
}

/// Write `snap` to `path` with the generated-file header.
pub fn save(path: &Path, snap: &Snapshot) -> Result<()> {
    let body = toml::to_string_pretty(snap)
        .with_context(|| format!("serializing snapshot {}", path.display()))?;
    std::fs::write(path, format!("{HEADER}\n{body}"))
        .with_context(|| format!("writing snapshot {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Snapshot {
        let mut wire = BTreeMap::new();
        wire.insert(
            "crates/rb-proto/src/messages.rs::Request".to_string(),
            "ab".repeat(32),
        );
        let mut migrations = BTreeMap::new();
        migrations.insert("001_init.sql".to_string(), "cd".repeat(32));
        Snapshot {
            contract_version: 2,
            wire,
            migrations,
            log: vec![LogEntry {
                contract_version: 2,
                intent: Intent::Initial,
                note: "baseline".to_string(),
            }],
        }
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("contract-snapshot.toml");
        let snap = sample();
        save(&path, &snap).unwrap();
        assert_eq!(load(&path).unwrap(), snap);
    }

    #[test]
    fn saved_file_carries_do_not_edit_header_and_update_hint() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("contract-snapshot.toml");
        save(&path, &sample()).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("# Contract-drift guard snapshot"));
        assert!(text.contains("--intent additive"));
        assert!(text.contains("--intent breaking"));
    }

    #[test]
    fn load_missing_file_errors_with_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nope.toml");
        let err = load(&path).unwrap_err();
        assert!(
            err.to_string().contains("nope.toml"),
            "error names the file, got: {err:#}"
        );
    }

    #[test]
    fn load_rejects_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.toml");
        std::fs::write(&path, "contract_version = \"not a number\"").unwrap();
        assert!(load(&path).is_err());
    }
}
