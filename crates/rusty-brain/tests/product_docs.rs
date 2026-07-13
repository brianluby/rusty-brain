//! Guards load-bearing product-documentation claims against the code and
//! workspace manifests that define them.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use clap::CommandFactory as _;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

fn workspace_members() -> Vec<String> {
    let manifest: toml::Value = toml::from_str(&read("Cargo.toml")).expect("workspace TOML parses");
    manifest["workspace"]["members"]
        .as_array()
        .expect("workspace.members is an array")
        .iter()
        .map(|member| {
            member
                .as_str()
                .expect("workspace member is a string")
                .to_string()
        })
        .collect()
}

#[test]
fn workspace_inventory_matches_readme_and_architecture() {
    let members = workspace_members();
    let readme = read("README.md");
    let architecture = read("docs/ARCHITECTURE.md");
    let count_claim = format!("**{} workspace crates**", members.len());

    for (name, document) in [("README", &readme), ("architecture", &architecture)] {
        assert!(
            document.contains(&count_claim),
            "{name} must derive its workspace count from Cargo.toml; expected {count_claim:?}"
        );
        for member in &members {
            let crate_name = Path::new(member)
                .file_name()
                .and_then(|name| name.to_str())
                .expect("workspace member has a final path component");
            assert!(
                document.contains(crate_name),
                "{name} workspace inventory is missing manifest member {crate_name:?}"
            );
        }
    }
}

#[test]
fn readme_mcp_count_matches_tool_definitions() {
    let readme = read("README.md");
    let count = rb_mcp::tool_definitions().len();
    let claim = format!("**{count} MCP tools**");
    assert!(
        readme.contains(&claim),
        "README must carry the code-derived MCP count marker {claim:?}"
    );
    assert!(
        !readme.contains("ten tools") && !readme.contains("the ten tools"),
        "README still contains the obsolete ten-tool claim"
    );
}

fn status_section(document: &str) -> &str {
    let remainder = document
        .split_once("## Status")
        .expect("PRD has a Status section")
        .1;
    remainder
        .split_once("\n## ")
        .map_or(remainder, |(status, _)| status)
}

fn checklist_section(document: &str) -> &str {
    let remainder = document
        .split_once("## Implementation Checklist")
        .expect("delivered PRD has an Implementation Checklist")
        .1;
    remainder
        .split_once("\n## ")
        .map_or(remainder, |(checklist, _)| checklist)
}

#[test]
fn delivered_prd_statuses_are_backed_by_cli_surfaces_and_closed_checklists() {
    let command = rusty_brain::cli::Cli::command();
    let index = read("docs/prds/README.md");
    let claims: [(&str, &[&str]); 3] = [
        (
            "docs/prds/2026-07-02-doctor-and-stats-observability.md",
            &["status", "stats", "doctor"],
        ),
        (
            "docs/prds/2026-07-02-init-and-project-import.md",
            &["init", "import"],
        ),
        (
            "docs/prds/2026-07-02-portable-export-and-backup.md",
            &["export", "backup", "restore"],
        ),
    ];

    for (prd, commands) in claims {
        let document = read(prd);
        assert!(
            status_section(&document)
                .trim_start()
                .starts_with("Delivered "),
            "{prd} must identify its delivered status"
        );
        assert!(
            !checklist_section(&document).contains("- [ ]"),
            "{prd} is marked delivered but still has an unchecked implementation item"
        );
        let file_name = Path::new(prd)
            .file_name()
            .and_then(|name| name.to_str())
            .expect("PRD path has a file name");
        let index_row = index
            .lines()
            .find(|line| line.contains(file_name))
            .unwrap_or_else(|| panic!("PRD index is missing {file_name}"));
        assert!(
            index_row.contains("Delivered"),
            "PRD index status drifted from delivered file {file_name}: {index_row}"
        );
        for subcommand in commands {
            assert!(
                command.find_subcommand(subcommand).is_some(),
                "{prd} claims delivery but CLI subcommand {subcommand:?} is absent"
            );
        }
    }

    let http_prd = "docs/prds/2026-07-02-http-surface-and-agent-agnostic-recall.md";
    let document = read(http_prd);
    assert!(
        status_section(&document)
            .trim_start()
            .starts_with("Delivered "),
        "{http_prd} must identify its delivered status"
    );
    assert!(
        !checklist_section(&document).contains("- [ ]"),
        "{http_prd} is marked delivered but still has an unchecked implementation item"
    );
    let http_index_row = index
        .lines()
        .find(|line| line.contains("2026-07-02-http-surface-and-agent-agnostic-recall.md"))
        .expect("PRD index includes the delivered HTTP PRD");
    assert!(
        http_index_row.contains("Delivered"),
        "PRD index status drifted from the delivered HTTP PRD: {http_index_row}"
    );
    let serve = command
        .find_subcommand("serve")
        .expect("delivered HTTP PRD requires serve");
    assert!(
        serve
            .get_arguments()
            .any(|argument| argument.get_id().as_str() == "http"),
        "delivered HTTP PRD requires the public `serve --http` flag"
    );
}
