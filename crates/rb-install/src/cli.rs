//! clap CLI surface for `rusty-brain-install`.

use std::io::IsTerminal as _;
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use rb_agents::install::InstallScope;

use crate::engine::{resolve_hooks_bin, run_install, run_status, run_uninstall, select_installers};
use crate::report::{AgentStatus, InstallReport};

/// `rusty-brain-install` — wire JSON-protocol CLIs to `rusty-brain-hooks`.
#[derive(Debug, Parser)]
#[command(
    name = "rusty-brain-install",
    about = "Install/uninstall rusty-brain hooks for AI CLIs."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,

    /// Force JSON output (otherwise JSON is auto-selected when stdout is not a TTY).
    #[arg(long, global = true)]
    pub json: bool,
}

/// Top-level subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Merge our sentinel-marked hook block into each CLI's config.
    Install {
        /// Restrict to these agents (claude-code, gemini, codex; opencode is
        /// deferred — it needs a JS/TS plugin).
        #[arg(long, value_delimiter = ',')]
        agents: Option<Vec<String>>,
        /// Install into the per-user (global) config instead of the project.
        #[arg(long)]
        global: bool,
        /// Compute and print the report without writing any file.
        #[arg(long)]
        dry_run: bool,
    },
    /// Remove ONLY our sentinel-marked entries, leaving the user's hooks intact.
    Uninstall {
        #[arg(long, value_delimiter = ',')]
        agents: Option<Vec<String>>,
        #[arg(long)]
        global: bool,
        #[arg(long)]
        dry_run: bool,
    },
    /// Report per-CLI detection + whether our hook block is present.
    Status {
        #[arg(long, value_delimiter = ',')]
        agents: Option<Vec<String>>,
        #[arg(long)]
        global: bool,
    },
}

/// Resolve the install scope: `--global` → Global, else the current dir.
fn scope_for(global: bool) -> InstallScope {
    if global {
        InstallScope::Global
    } else {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        InstallScope::Project(cwd)
    }
}

/// Execute the parsed CLI, returning the report and the chosen JSON-ness.
///
/// Pure of process exit — `main` renders + exits. Returns `Err` only for a
/// fatal arg error (e.g. unknown agent), which `main` renders as JSON/text.
pub fn execute(cli: &Cli) -> Result<(InstallReport, bool), String> {
    let json = cli.json || !std::io::stdout().is_terminal();
    let hooks_bin = resolve_hooks_bin();
    let report = match &cli.command {
        Command::Install {
            agents,
            global,
            dry_run,
        } => {
            let installers = select_installers(agents.as_deref()).map_err(|e| e.to_string())?;
            run_install(&installers, &hooks_bin, &scope_for(*global), *dry_run)
        }
        Command::Uninstall {
            agents,
            global,
            dry_run,
        } => {
            let installers = select_installers(agents.as_deref()).map_err(|e| e.to_string())?;
            run_uninstall(&installers, &hooks_bin, &scope_for(*global), *dry_run)
        }
        Command::Status { agents, global } => {
            let installers = select_installers(agents.as_deref()).map_err(|e| e.to_string())?;
            run_status(&installers, &hooks_bin, &scope_for(*global))
        }
    };
    Ok((report, json))
}

/// Render a report as either JSON or a symbol-decorated human summary.
#[must_use]
pub fn render(report: &InstallReport, json: bool) -> String {
    if json {
        return serde_json::to_string_pretty(report)
            .unwrap_or_else(|_| "{\"status\":\"failed\"}".to_string());
    }
    let mut out = String::new();
    out.push_str(&format!(
        "rusty-brain-install ({} scope{})\n",
        report.scope,
        if report.dry_run { ", dry-run" } else { "" }
    ));
    for a in &report.agents {
        let symbol = match a.status {
            AgentStatus::Configured
            | AgentStatus::Upgraded
            | AgentStatus::Removed
            | AgentStatus::Present
            | AgentStatus::WouldConfigure
            | AgentStatus::WouldRemove => "[ok]",
            AgentStatus::Absent | AgentStatus::NotFound => "[--]",
            AgentStatus::Failed => "[xx]",
        };
        let path = a
            .config_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        out.push_str(&format!(
            "  {symbol} {:<12} {:?}  {}\n",
            a.agent, a.status, path
        ));
        if let Some(err) = &a.error {
            out.push_str(&format!("        error: {err}\n"));
        }
    }
    out.push_str(&format!("overall: {:?}\n", report.status));
    out
}

/// Map a report's overall status to a process exit code (always 0 — installer
/// never blocks; failures are reported, not fatal, mirroring the capture
/// fail-open ethos).
#[must_use]
pub fn exit_code(_report: &InstallReport) -> i32 {
    0
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use crate::report::ReportStatus;
    use clap::Parser;

    #[test]
    fn parses_install_with_agents_and_flags() {
        let cli = Cli::try_parse_from([
            "rusty-brain-install",
            "install",
            "--agents",
            "claude-code,codex",
            "--global",
            "--dry-run",
        ])
        .unwrap();
        match cli.command {
            Command::Install {
                agents,
                global,
                dry_run,
            } => {
                assert_eq!(
                    agents,
                    Some(vec!["claude-code".to_string(), "codex".to_string()])
                );
                assert!(global);
                assert!(dry_run);
            }
            _ => panic!("expected install"),
        }
    }

    #[test]
    fn parses_uninstall_and_status() {
        assert!(matches!(
            Cli::try_parse_from(["rusty-brain-install", "uninstall"])
                .unwrap()
                .command,
            Command::Uninstall { .. }
        ));
        assert!(matches!(
            Cli::try_parse_from(["rusty-brain-install", "status"])
                .unwrap()
                .command,
            Command::Status { .. }
        ));
    }

    #[test]
    fn render_json_contains_status_and_agents() {
        let report = InstallReport::roll_up(
            "project",
            true,
            vec![crate::report::AgentReport {
                agent: "claude-code".into(),
                status: AgentStatus::WouldConfigure,
                config_path: Some("/tmp/.claude/settings.json".into()),
                version: Some("1.0.0".into()),
                error: None,
            }],
        );
        let json = render(&report, true);
        assert!(json.contains("\"status\": \"success\""));
        assert!(json.contains("\"would_configure\""));
        assert!(json.contains("claude-code"));
    }

    #[test]
    fn render_human_uses_symbols() {
        let report = InstallReport::roll_up(
            "project",
            false,
            vec![crate::report::AgentReport {
                agent: "codex".into(),
                status: AgentStatus::Failed,
                config_path: None,
                version: None,
                error: Some("boom".into()),
            }],
        );
        let text = render(&report, false);
        assert!(text.contains("[xx]"));
        assert!(text.contains("codex"));
        assert!(text.contains("error: boom"));
        assert_eq!(ReportStatus::Failed, report.status);
    }

    #[test]
    fn exit_code_is_always_zero() {
        let report = InstallReport::roll_up("project", false, vec![]);
        assert_eq!(exit_code(&report), 0);
    }
}
