//! `rusty-brain` binary entry point: init logging, parse, dispatch, map exit code.

use clap::Parser;
use rusty_brain::cli::Cli;
use rusty_brain::logging::init_logging;
use rusty_brain::run::run;
use std::process::ExitCode;

fn main() -> ExitCode {
    init_logging();
    let cli = Cli::parse();

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("error: failed to start tokio runtime: {e}");
            return ExitCode::FAILURE;
        }
    };

    match runtime.block_on(run(cli)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}
