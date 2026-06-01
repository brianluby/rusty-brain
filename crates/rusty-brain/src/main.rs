//! `rusty-brain` binary entry point. Parses the CLI; dispatch is wired in Task 34.

use clap::Parser;
use rusty_brain::cli::Cli;
use std::process::ExitCode;

fn main() -> ExitCode {
    let _cli = Cli::parse();
    ExitCode::SUCCESS
}
