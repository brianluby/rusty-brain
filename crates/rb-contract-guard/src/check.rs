//! Compare the observed contract surface against the checked-in snapshot.

use crate::snapshot::Snapshot;
use crate::Observed;
use std::collections::BTreeMap;

/// Result of a drift check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Observed surface matches the snapshot exactly.
    Clean,
    /// One human-readable line per divergence, each naming the drifted item.
    Drift(Vec<String>),
}

fn diff_maps(
    kind: &str,
    observed: &BTreeMap<String, String>,
    recorded: &BTreeMap<String, String>,
    drift: &mut Vec<String>,
) {
    for (key, digest) in observed {
        match recorded.get(key) {
            None => drift.push(format!("{kind} added: {key}")),
            Some(rec) if rec != digest => drift.push(format!("{kind} changed: {key}")),
            Some(_) => {}
        }
    }
    for key in recorded.keys() {
        if !observed.contains_key(key) {
            drift.push(format!("{kind} removed: {key}"));
        }
    }
}

/// Compare `observed` against `snapshot`.
pub fn check(observed: &Observed, snapshot: &Snapshot) -> Outcome {
    let mut drift = Vec::new();
    if observed.contract_version != snapshot.contract_version {
        drift.push(format!(
            "CONTRACT_VERSION changed: snapshot records {}, code declares {}",
            snapshot.contract_version, observed.contract_version
        ));
    }
    diff_maps("wire item", &observed.wire, &snapshot.wire, &mut drift);
    diff_maps(
        "migration",
        &observed.migrations,
        &snapshot.migrations,
        &mut drift,
    );
    if drift.is_empty() {
        Outcome::Clean
    } else {
        Outcome::Drift(drift)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::{Intent, LogEntry};

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn observed() -> Observed {
        Observed {
            contract_version: 2,
            wire: map(&[("m.rs::Request", "aaa"), ("m.rs::Response", "bbb")]),
            migrations: map(&[("001_init.sql", "ccc")]),
        }
    }

    fn snapshot_matching(obs: &Observed) -> Snapshot {
        Snapshot {
            contract_version: obs.contract_version,
            wire: obs.wire.clone(),
            migrations: obs.migrations.clone(),
            log: vec![LogEntry {
                contract_version: obs.contract_version,
                intent: Intent::Initial,
                note: "baseline".to_string(),
            }],
        }
    }

    fn drift_lines(outcome: Outcome) -> Vec<String> {
        match outcome {
            Outcome::Drift(lines) => lines,
            Outcome::Clean => panic!("expected drift"),
        }
    }

    #[test]
    fn matching_surface_is_clean() {
        let obs = observed();
        let snap = snapshot_matching(&obs);
        assert_eq!(check(&obs, &snap), Outcome::Clean);
    }

    #[test]
    fn changed_wire_item_is_named() {
        let obs = observed();
        let mut snap = snapshot_matching(&obs);
        snap.wire
            .insert("m.rs::Request".to_string(), "OLD".to_string());
        let lines = drift_lines(check(&obs, &snap));
        assert_eq!(lines.len(), 1);
        assert!(
            lines[0].contains("m.rs::Request") && lines[0].contains("changed"),
            "line names the changed item: {lines:?}"
        );
    }

    #[test]
    fn added_and_removed_wire_items_are_named() {
        let obs = observed();
        let mut snap = snapshot_matching(&obs);
        snap.wire.remove("m.rs::Response"); // observed has it -> "added"
        snap.wire
            .insert("m.rs::Gone".to_string(), "ddd".to_string()); // only snapshot -> "removed"
        let lines = drift_lines(check(&obs, &snap));
        assert_eq!(lines.len(), 2);
        assert!(lines
            .iter()
            .any(|l| l.contains("m.rs::Response") && l.contains("added")));
        assert!(lines
            .iter()
            .any(|l| l.contains("m.rs::Gone") && l.contains("removed")));
    }

    #[test]
    fn version_change_alone_is_drift() {
        let obs = observed();
        let mut snap = snapshot_matching(&obs);
        snap.contract_version = 1;
        let lines = drift_lines(check(&obs, &snap));
        assert_eq!(lines.len(), 1);
        assert!(
            lines[0].contains("CONTRACT_VERSION")
                && lines[0].contains('1')
                && lines[0].contains('2'),
            "line names both versions: {lines:?}"
        );
    }

    #[test]
    fn migration_drift_is_named() {
        let mut obs = observed();
        let snap = snapshot_matching(&obs);
        obs.migrations
            .insert("009_new.sql".to_string(), "eee".to_string());
        obs.migrations
            .insert("001_init.sql".to_string(), "TAMPERED".to_string());
        let lines = drift_lines(check(&obs, &snap));
        assert_eq!(lines.len(), 2);
        assert!(lines
            .iter()
            .any(|l| l.contains("009_new.sql") && l.contains("added")));
        assert!(lines
            .iter()
            .any(|l| l.contains("001_init.sql") && l.contains("changed")));
    }

    #[test]
    fn multiple_drifts_all_reported() {
        let mut obs = observed();
        obs.contract_version = 3;
        obs.wire
            .insert("m.rs::Request".to_string(), "NEW".to_string());
        obs.migrations
            .insert("009_new.sql".to_string(), "eee".to_string());
        let snap = {
            let base = observed();
            snapshot_matching(&base)
        };
        let lines = drift_lines(check(&obs, &snap));
        assert_eq!(lines.len(), 3, "version + wire + migration: {lines:?}");
    }
}
