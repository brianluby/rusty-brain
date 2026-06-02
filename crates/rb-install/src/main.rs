//! Entry point for the `rusty-brain-install` binary.

use std::process::ExitCode;

use clap::Parser as _;

use rb_install::cli::{execute, exit_code, render, Cli};

fn main() -> ExitCode {
    let cli = Cli::parse();
    let json = cli.json || !std::io::IsTerminal::is_terminal(&std::io::stdout());
    match execute(&cli) {
        Ok((report, json_out)) => {
            print!("{}", render(&report, json_out));
            ExitCode::from(exit_code(&report) as u8)
        }
        Err(message) => {
            if json {
                println!(
                    "{{\"status\":\"failed\",\"error\":{}}}",
                    json_string(&message)
                );
            } else {
                eprintln!("error: {message}");
            }
            // Fail-open ethos: report the error, but never block with non-zero.
            ExitCode::SUCCESS
        }
    }
}

/// Encode `s` as a JSON string literal (quotes + escapes) without `unwrap`.
fn json_string(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}
