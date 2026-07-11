//! Snapshot regeneration with an enforced, deliberate intent (the W5a.4
//! decision): `additive` must NOT bump `CONTRACT_VERSION`, `breaking` MUST.

use crate::check::{check, Outcome};
use crate::snapshot::{Intent, LogEntry, Snapshot};
use crate::Observed;
use anyhow::{bail, Result};

fn require_note(note: &str) -> Result<String> {
    let trimmed = note.trim();
    if trimmed.is_empty() {
        bail!("a non-empty --note describing the contract change is required");
    }
    Ok(trimmed.to_string())
}

/// Build the baseline snapshot (first ever generation).
pub fn init_snapshot(observed: &Observed, note: &str) -> Result<Snapshot> {
    let note = require_note(note)?;
    Ok(Snapshot {
        contract_version: observed.contract_version,
        wire: observed.wire.clone(),
        migrations: observed.migrations.clone(),
        log: vec![LogEntry {
            contract_version: observed.contract_version,
            intent: Intent::Initial,
            note,
        }],
    })
}

/// Regenerate the snapshot from `observed`, recording `intent` + `note` in the
/// append-only log. Enforces the version rule for the declared intent.
pub fn apply_update(
    observed: &Observed,
    prev: &Snapshot,
    intent: Intent,
    note: &str,
) -> Result<Snapshot> {
    let note = require_note(note)?;
    if check(observed, prev) == Outcome::Clean {
        bail!("snapshot is already current; nothing to record");
    }
    match intent {
        Intent::Initial => bail!("intent `initial` is reserved for `init`"),
        Intent::Additive => {
            if observed.contract_version != prev.contract_version {
                bail!(
                    "--intent additive requires an unchanged CONTRACT_VERSION \
                     (snapshot {}, code {}): a bumped version is a breaking \
                     change — use --intent breaking",
                    prev.contract_version,
                    observed.contract_version
                );
            }
        }
        Intent::Breaking => {
            if observed.contract_version <= prev.contract_version {
                bail!(
                    "--intent breaking requires a CONTRACT_VERSION bump \
                     (snapshot {}, code {}): bump the const in rb-proto \
                     messages.rs first",
                    prev.contract_version,
                    observed.contract_version
                );
            }
        }
    }
    let mut log = prev.log.clone();
    log.push(LogEntry {
        contract_version: observed.contract_version,
        intent,
        note,
    });
    Ok(Snapshot {
        contract_version: observed.contract_version,
        wire: observed.wire.clone(),
        migrations: observed.migrations.clone(),
        log,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn observed(version: u32) -> Observed {
        Observed {
            contract_version: version,
            wire: map(&[("m.rs::Request", "aaa")]),
            migrations: map(&[("001_init.sql", "bbb")]),
        }
    }

    fn baseline() -> Snapshot {
        init_snapshot(&observed(2), "baseline").unwrap()
    }

    #[test]
    fn init_records_baseline_with_initial_intent() {
        let snap = baseline();
        assert_eq!(snap.contract_version, 2);
        assert_eq!(snap.wire, observed(2).wire);
        assert_eq!(snap.migrations, observed(2).migrations);
        assert_eq!(snap.log.len(), 1);
        assert_eq!(snap.log[0].intent, Intent::Initial);
        assert_eq!(snap.log[0].note, "baseline");
    }

    #[test]
    fn init_rejects_empty_note() {
        assert!(init_snapshot(&observed(2), "  ").is_err());
    }

    #[test]
    fn additive_update_with_same_version_appends_log() {
        let prev = baseline();
        let mut obs = observed(2);
        obs.wire
            .insert("m.rs::Request".to_string(), "NEW".to_string());
        let snap = apply_update(&obs, &prev, Intent::Additive, "add Stats variant").unwrap();
        assert_eq!(snap.contract_version, 2);
        assert_eq!(
            snap.wire.get("m.rs::Request").map(String::as_str),
            Some("NEW")
        );
        assert_eq!(snap.log.len(), 2, "log is append-only");
        assert_eq!(snap.log[1].intent, Intent::Additive);
        assert_eq!(snap.log[1].contract_version, 2);
    }

    #[test]
    fn additive_update_rejects_a_version_bump() {
        let prev = baseline();
        let mut obs = observed(3);
        obs.wire
            .insert("m.rs::Request".to_string(), "NEW".to_string());
        let err = apply_update(&obs, &prev, Intent::Additive, "oops").unwrap_err();
        assert!(
            err.to_string().contains("breaking"),
            "error points at --intent breaking, got: {err:#}"
        );
    }

    #[test]
    fn breaking_update_requires_a_version_bump() {
        let prev = baseline();
        let mut obs = observed(2);
        obs.wire
            .insert("m.rs::Request".to_string(), "NEW".to_string());
        let err = apply_update(&obs, &prev, Intent::Breaking, "shape change").unwrap_err();
        assert!(
            err.to_string().contains("bump"),
            "error demands a CONTRACT_VERSION bump, got: {err:#}"
        );
    }

    #[test]
    fn breaking_update_with_bump_succeeds() {
        let prev = baseline();
        let mut obs = observed(3);
        obs.wire
            .insert("m.rs::Request".to_string(), "NEW".to_string());
        let snap = apply_update(&obs, &prev, Intent::Breaking, "retag Request").unwrap();
        assert_eq!(snap.contract_version, 3);
        assert_eq!(snap.log.len(), 2);
        assert_eq!(snap.log[1].intent, Intent::Breaking);
        assert_eq!(snap.log[1].contract_version, 3);
    }

    #[test]
    fn breaking_update_rejects_a_version_decrease() {
        let prev = baseline();
        let mut obs = observed(1);
        obs.wire
            .insert("m.rs::Request".to_string(), "NEW".to_string());
        assert!(apply_update(&obs, &prev, Intent::Breaking, "downgrade").is_err());
    }

    #[test]
    fn update_rejects_empty_note() {
        let prev = baseline();
        let mut obs = observed(2);
        obs.wire
            .insert("m.rs::Request".to_string(), "NEW".to_string());
        assert!(apply_update(&obs, &prev, Intent::Additive, "").is_err());
    }

    #[test]
    fn update_rejects_initial_intent() {
        let prev = baseline();
        let mut obs = observed(2);
        obs.wire
            .insert("m.rs::Request".to_string(), "NEW".to_string());
        assert!(apply_update(&obs, &prev, Intent::Initial, "nope").is_err());
    }

    #[test]
    fn update_when_already_current_is_rejected() {
        let prev = baseline();
        let err = apply_update(&observed(2), &prev, Intent::Additive, "noise").unwrap_err();
        assert!(
            err.to_string().contains("current"),
            "error says the snapshot is already current, got: {err:#}"
        );
    }
}
