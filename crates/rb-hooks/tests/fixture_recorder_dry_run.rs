//! Offline CI gate for the cross-agent fixture-recording harness: runs the
//! script's pure `--self-test` and an `--dry-run --agent all`. The live
//! recording path needs CLI auth and is exercised manually by the operator.
use std::path::PathBuf;
use std::process::Command;

fn script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../scripts/record-agent-fixtures.sh")
}

#[test]
fn self_test_passes() {
    let out = Command::new("bash")
        .arg(script())
        .arg("--self-test")
        .output()
        .expect("run self-test");
    assert!(
        out.status.success(),
        "self-test failed:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn dry_run_all_succeeds() {
    let out = Command::new("bash")
        .arg(script())
        .args(["--dry-run", "--agent", "all"])
        .output()
        .expect("run dry-run");
    assert!(
        out.status.success(),
        "dry-run failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
