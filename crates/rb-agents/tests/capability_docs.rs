//! Guards the README agent capability matrix against drift from the code's
//! source of truth (`rb_agents::capability`).
//!
//! The README table is user-facing documentation; `capability.rs` is what the
//! adapters and scorecard actually honor. Editing one without the other is a
//! recurring failure mode (the opencode row went stale when capture graduated
//! Partial -> Supported), so this test parses the README table and asserts
//! every cell matches the matrix.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;

use rb_agents::{agent_capabilities, AdapterStatus, SupportLevel};

const MATRIX_MARKER: &str = "Current agent capability matrix:";

fn readme_text() -> String {
    // CARGO_MANIFEST_DIR = <workspace>/crates/rb-agents; README sits at the root.
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../README.md");
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
}

/// Extract the capability table rows (agent rows only) that follow the marker.
fn readme_matrix_rows(readme: &str) -> Vec<Vec<String>> {
    let after_marker = readme
        .split_once(MATRIX_MARKER)
        .unwrap_or_else(|| panic!("README is missing the marker line {MATRIX_MARKER:?}"))
        .1;
    after_marker
        .lines()
        .skip_while(|line| !line.trim_start().starts_with('|'))
        .take_while(|line| line.trim_start().starts_with('|'))
        .filter(|line| {
            // Drop the header row and the |---| separator row.
            let body = line.trim().trim_matches('|');
            !body.contains("Agent")
                && !body.trim_matches('-').trim().starts_with("---")
                && !body.replace(['-', '|', ' '], "").is_empty()
        })
        .map(|line| {
            line.trim()
                .trim_start_matches('|')
                .trim_end_matches('|')
                .split('|')
                .map(|cell| cell.trim().to_string())
                .collect()
        })
        .collect()
}

fn adapter_status_token(status: AdapterStatus) -> &'static str {
    match status {
        AdapterStatus::Stable => "stable",
        AdapterStatus::Experimental => "experimental",
        AdapterStatus::Discovery => "discovery",
        AdapterStatus::Unsupported => "unsupported",
    }
}

fn support_level_token(level: SupportLevel) -> &'static str {
    match level {
        SupportLevel::Supported => "supported",
        SupportLevel::Partial => "partial",
        SupportLevel::Unsupported => "unsupported",
        SupportLevel::Unknown => "unknown",
    }
}

#[test]
fn readme_matrix_matches_capability_source_of_truth() {
    let readme = readme_text();
    let rows = readme_matrix_rows(&readme);
    let matrix = agent_capabilities();

    assert_eq!(
        rows.len(),
        matrix.len(),
        "README capability table has {} agent rows, capability.rs has {}",
        rows.len(),
        matrix.len()
    );

    for capability in matrix {
        let want_id = format!("`{}`", capability.agent);
        let row = rows
            .iter()
            .find(|cells| cells.first().map(String::as_str) == Some(want_id.as_str()))
            .unwrap_or_else(|| panic!("README matrix is missing a row for {want_id}"));
        assert!(
            row.len() >= 7,
            "README row for {want_id} has {} cells, expected at least 7",
            row.len()
        );

        let expect = [
            adapter_status_token(capability.adapter_status),
            support_level_token(capability.capture),
            support_level_token(capability.retrieval),
            support_level_token(capability.config),
            support_level_token(capability.scorecard),
        ];
        let columns = ["adapter", "capture", "retrieval", "config", "scorecard"];
        for (idx, (column, want)) in columns.iter().zip(expect).enumerate() {
            assert_eq!(
                row[idx + 1],
                want,
                "README {want_id} `{column}` cell drifted from capability.rs \
                 (README says {:?}, code says {want:?})",
                row[idx + 1]
            );
        }
        assert!(
            !row[6].is_empty(),
            "README {want_id} lifecycle/limitation cell must not be empty"
        );
    }
}

#[test]
fn readme_lists_scorecard_agent_targets() {
    // CA5/CA7: the README must document per-agent scorecard invocation,
    // including the aggregate target.
    let readme = readme_text();
    for target in ["claude-code", "codex", "opencode", "all"] {
        let needle = format!("--agent {target}");
        assert!(
            readme.contains(&needle),
            "README must document `scripts/memory-scorecard.sh {needle}`"
        );
    }
}
