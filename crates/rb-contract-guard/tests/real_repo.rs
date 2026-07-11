//! Binds the guard to THIS repository: `cargo test --workspace` fails locally
//! (not just in the dedicated CI job) whenever the wire surface or migrations
//! drift from `contract-snapshot.toml` without a recorded decision.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use rb_contract_guard::check::{check, Outcome};
use rb_contract_guard::{observe, snapshot, SurfaceConfig};
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

#[test]
fn snapshot_is_current_for_this_repo() {
    let root = repo_root();
    let cfg = SurfaceConfig::default_paths();
    let observed = observe(&root, &cfg).expect("contract surface must be observable");
    let snap = snapshot::load(&root.join(&cfg.snapshot_file))
        .expect("contract-snapshot.toml must exist; bootstrap with `rb-contract-guard init`");
    match check(&observed, &snap) {
        Outcome::Clean => {}
        Outcome::Drift(lines) => panic!(
            "contract drift without a recorded decision (W5a.4). Run\n  \
             cargo run -p rb-contract-guard -- update --intent additive|breaking --note \"...\"\n\
             after deciding whether the change bumps CONTRACT_VERSION.\nDrift:\n  {}",
            lines.join("\n  ")
        ),
    }
}

#[test]
fn snapshot_version_matches_rb_proto() {
    // The snapshot's recorded version must be the one rb-proto actually
    // exports (guards against a hand-edited snapshot file).
    let root = repo_root();
    let cfg = SurfaceConfig::default_paths();
    let snap = snapshot::load(&root.join(&cfg.snapshot_file)).unwrap();
    assert_eq!(snap.contract_version, rb_proto::CONTRACT_VERSION);
}
