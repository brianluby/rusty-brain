//! Install subcommand handler.

use std::io::IsTerminal;

use platforms::installer::orchestrator::InstallOrchestrator;
use types::install::{InstallConfig, InstallError, InstallScope};

use crate::CliError;
use crate::output;

/// Run the install subcommand.
///
/// Builds an [`InstallConfig`] from CLI args, delegates to
/// [`InstallOrchestrator::run()`], and formats output.
///
/// # Errors
///
/// Returns [`CliError::Install`] if scope is not specified or the orchestrator
/// fails, or [`CliError::Io`] if the current directory cannot be determined.
#[allow(clippy::fn_params_excessive_bools)]
pub fn run_install(
    agents: Option<Vec<String>>,
    project: bool,
    global: bool,
    json: bool,
    reconfigure: bool,
) -> Result<(), CliError> {
    // Determine scope (M-13: require --project or --global).
    let scope = if project {
        let root = std::env::current_dir().map_err(CliError::Io)?;
        InstallScope::Project { root }
    } else if global {
        InstallScope::Global
    } else {
        return Err(CliError::Install(InstallError::ScopeRequired));
    };

    // Auto-enable JSON when stdin is not a TTY (M-7, AC-12).
    let json = json || !std::io::stdin().is_terminal();

    let config = InstallConfig {
        agents,
        scope,
        json,
        reconfigure,
    };

    let orchestrator = InstallOrchestrator::with_builtins();
    let report = orchestrator.run(&config).map_err(CliError::Install)?;

    if json {
        output::print_json(&report)
    } else {
        print_install_human(&report);
        Ok(())
    }
}

/// Print a human-readable install report.
fn print_install_human(report: &types::install::InstallReport) {
    println!("Install Report (scope: {})", report.scope);
    println!("Memory store: {}", report.memory_store.display());
    println!();

    for result in &report.results {
        let status_str = match &result.status {
            types::install::InstallStatus::Configured => "✓ Configured",
            types::install::InstallStatus::Upgraded => "↑ Upgraded",
            types::install::InstallStatus::Skipped => "- Skipped",
            types::install::InstallStatus::Failed => "✗ Failed",
            types::install::InstallStatus::NotFound => "? Not found",
        };

        print!("  {}: {status_str}", result.agent_name);

        if let Some(ref version) = result.version_detected {
            print!(" (v{version})");
        }
        if let Some(ref path) = result.config_path {
            print!(" -> {}", path.display());
        }
        if let Some(ref err) = result.error {
            print!(" [{err}]");
        }
        println!();
    }

    println!();
    let status_label = match report.status {
        types::install::ReportStatus::Success => "success",
        types::install::ReportStatus::Partial => "partial",
        types::install::ReportStatus::Failed => "failed",
    };
    println!("Status: {status_label}");
}
