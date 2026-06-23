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
    // Use an explicit --out-dir so the layout is at a known path we can inspect
    // (the bare run scatters each agent under its own mktemp dir).
    let base = std::env::temp_dir().join(format!("rb-dryrun-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let out = Command::new("bash")
        .arg(script())
        .args(["--dry-run", "--agent", "all", "--out-dir"])
        .arg(&base)
        .output()
        .expect("run dry-run");
    assert!(
        out.status.success(),
        "dry-run failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Each agent must land in its OWN subfolder (no clobbering) with a valid
    // hooks.json/plugin, a parseable terminus.json, and a README.
    for agent in ["codex", "opencode"] {
        let dir = base.join(agent);
        let readme = dir.join("README.md");
        assert!(readme.is_file(), "missing {agent} README.md");
        let readme_txt = std::fs::read_to_string(&readme).expect("read README");
        assert!(
            readme_txt.contains(&format!("# {agent} Fixture Status")),
            "{agent} README was clobbered or mis-titled:\n{readme_txt}"
        );
        assert!(
            readme_txt.contains("## Provenance") && readme_txt.contains("## Files"),
            "{agent} README missing required sections"
        );

        let terminus = dir.join("terminus.json");
        assert!(terminus.is_file(), "missing {agent} terminus.json");
        let terminus_txt = std::fs::read_to_string(&terminus).expect("read terminus.json");
        let parsed: serde_json::Value =
            serde_json::from_str(&terminus_txt).expect("terminus.json is valid JSON");
        assert!(
            parsed.get("verdict").is_some(),
            "{agent} terminus.json missing verdict: {terminus_txt}"
        );
    }

    // codex hooks.json must be valid JSON registering the four lifecycle events.
    let hooks_txt =
        std::fs::read_to_string(base.join("codex/.codex/hooks.json")).expect("read hooks.json");
    let hooks: serde_json::Value =
        serde_json::from_str(&hooks_txt).expect("codex hooks.json is valid JSON");
    for ev in ["SessionStart", "PostToolUse", "Stop", "PreCompact"] {
        assert!(
            hooks["hooks"].get(ev).is_some(),
            "codex hooks.json missing event {ev}"
        );
    }

    let _ = std::fs::remove_dir_all(&base);
}
