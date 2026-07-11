//! CLI entry point for the contract-drift guard.
//!
//! Exit codes: 0 = surface matches the snapshot; 1 = contract drift without a
//! recorded decision; 2 = usage/configuration error (missing snapshot, rejected
//! intent, unreadable surface).

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use rb_contract_guard::check::{check, Outcome};
use rb_contract_guard::snapshot::{self, Intent};
use rb_contract_guard::{observe, update, SurfaceConfig};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "rb-contract-guard",
    about = "Contract-drift guard for the rusty-brain wire protocol and rb-store migrations (W5a.4)"
)]
struct Cli {
    /// Repo root containing crates/ and the snapshot file.
    #[arg(long, global = true, default_value = ".")]
    root: PathBuf,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Compare the observed contract surface against the checked-in snapshot.
    Check,
    /// Write the baseline snapshot (first-time bootstrap only).
    Init {
        /// Short description recorded in the snapshot log.
        #[arg(long)]
        note: String,
    },
    /// Regenerate the snapshot, recording a deliberate contract decision.
    Update {
        /// additive = serde-default-compatible, no CONTRACT_VERSION bump;
        /// breaking = CONTRACT_VERSION was bumped.
        #[arg(long)]
        intent: CliIntent,
        /// Short description recorded in the snapshot log.
        #[arg(long)]
        note: String,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum CliIntent {
    Additive,
    Breaking,
}

impl From<CliIntent> for Intent {
    fn from(value: CliIntent) -> Self {
        match value {
            CliIntent::Additive => Intent::Additive,
            CliIntent::Breaking => Intent::Breaking,
        }
    }
}

fn drift_report(lines: &[String], cfg: &SurfaceConfig) -> String {
    let mut out = String::from(
        "contract drift detected — the wire/persistence contract surface changed \
         without a recorded decision (W5a.4 protocol-evolution policy):\n",
    );
    for line in lines {
        out.push_str("  - ");
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(&format!(
        "\nDecide and record the change:\n\
         * additive, serde-default-compatible change (old frames still decode; \
         NO CONTRACT_VERSION bump):\n\
         \x20   cargo run -p rb-contract-guard -- update --intent additive --note \"<what changed>\"\n\
         * breaking change: bump CONTRACT_VERSION in {version_file} first, then:\n\
         \x20   cargo run -p rb-contract-guard -- update --intent breaking --note \"<what changed>\"\n\
         Then commit the regenerated {snapshot}.",
        version_file = cfg.version_file,
        snapshot = cfg.snapshot_file,
    ));
    out
}

fn run(cli: &Cli) -> Result<ExitCode> {
    let cfg = SurfaceConfig::default_paths();
    let snap_path = cli.root.join(&cfg.snapshot_file);
    let observed = observe(&cli.root, &cfg)?;

    match &cli.cmd {
        Cmd::Check => {
            if !snap_path.is_file() {
                bail!(
                    "no {} found under {} — bootstrap it once with \
                     `cargo run -p rb-contract-guard -- init --note \"baseline\"`",
                    cfg.snapshot_file,
                    cli.root.display()
                );
            }
            let snap = snapshot::load(&snap_path)?;
            match check(&observed, &snap) {
                Outcome::Clean => {
                    println!(
                        "contract surface matches {} (CONTRACT_VERSION {}, {} wire items, {} migrations)",
                        cfg.snapshot_file,
                        observed.contract_version,
                        observed.wire.len(),
                        observed.migrations.len()
                    );
                    Ok(ExitCode::SUCCESS)
                }
                Outcome::Drift(lines) => {
                    eprintln!("{}", drift_report(&lines, &cfg));
                    Ok(ExitCode::from(1))
                }
            }
        }
        Cmd::Init { note } => {
            if snap_path.is_file() {
                bail!(
                    "{} already exists — record changes with `update`, not `init`",
                    cfg.snapshot_file
                );
            }
            let snap = update::init_snapshot(&observed, note)?;
            snapshot::save(&snap_path, &snap)?;
            println!(
                "wrote baseline {} (CONTRACT_VERSION {})",
                cfg.snapshot_file, snap.contract_version
            );
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Update { intent, note } => {
            let prev = snapshot::load(&snap_path).with_context(|| {
                format!(
                    "no usable {} — bootstrap it once with `init --note ...`",
                    cfg.snapshot_file
                )
            })?;
            let snap = update::apply_update(&observed, &prev, (*intent).into(), note)?;
            snapshot::save(&snap_path, &snap)?;
            println!(
                "recorded {} contract change in {} (CONTRACT_VERSION {})",
                match intent {
                    CliIntent::Additive => "additive",
                    CliIntent::Breaking => "breaking",
                },
                cfg.snapshot_file,
                snap.contract_version
            );
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("rb-contract-guard error: {e:#}");
            ExitCode::from(2)
        }
    }
}
