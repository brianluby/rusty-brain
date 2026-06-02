//! `rusty-brain` binary entry point: init logging, parse, resolve namespace OFF
//! the async runtime, then dispatch on the runtime, and map the exit code.

use clap::Parser;
use rusty_brain::cli::Cli;
use rusty_brain::logging::init_logging;
use rusty_brain::namespace_detect::detect_namespace;
use rusty_brain::run::run;
use std::process::ExitCode;

fn main() -> ExitCode {
    init_logging();
    let cli = Cli::parse();

    // Resolve the namespace synchronously BEFORE the runtime exists: detection
    // shells out to git and reads `CLAUDE.md`, which must not run on a tokio
    // worker thread (P1 should-fix). It never fails (degrades to Global).
    let namespace = detect_namespace();

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("error: failed to start tokio runtime: {e}");
            return ExitCode::FAILURE;
        }
    };

    match runtime.block_on(run(cli, namespace)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use rusty_brain::namespace_detect::detect_namespace;

    #[test]
    fn detect_namespace_runs_without_a_tokio_runtime() {
        // This test has NO #[tokio::test] and no runtime: detection must be a
        // plain synchronous call. It does spawn a `git` subprocess (no network)
        // and read files, which is exactly why it must run BEFORE block_on and
        // never on a tokio worker thread. A clean return here (it never fails;
        // it degrades to Global) proves it is runtime-free.
        let _ns = detect_namespace();
    }
}
