//! `OpenCode` subcommand handlers.
//!
//! Reads JSON from stdin, deserializes to typed input, dispatches to library
//! handlers wrapped in fail-open, and serializes output to stdout JSON.
//! Tracing goes to stderr.

use std::path::Path;
use std::time::Duration;

use crate::CliError;
use crate::args::OpenCodeCommand;
use types::{RustyBrainError, error_codes};

/// Dispatch an `OpenCode` subcommand.
///
/// Reads input from stdin, delegates to the appropriate library handler
/// (wrapped in fail-open), and writes JSON output to stdout.
pub fn dispatch(subcmd: &OpenCodeCommand) -> Result<(), CliError> {
    match subcmd {
        OpenCodeCommand::ChatHook => run_chat_hook(),
        OpenCodeCommand::ToolHook => run_tool_hook(),
        OpenCodeCommand::Mind => run_mind(),
        OpenCodeCommand::SessionCleanup => run_session_cleanup(),
        OpenCodeCommand::SessionStart => run_session_start(),
    }
}

fn io_to_rusty(err: std::io::Error) -> RustyBrainError {
    RustyBrainError::FileSystem {
        code: error_codes::E_FS_IO_ERROR,
        message: "failed to read OpenCode command input".to_string(),
        source: Some(err),
    }
}

fn json_to_rusty(err: serde_json::Error) -> RustyBrainError {
    RustyBrainError::Serialization {
        code: error_codes::E_SER_DESERIALIZE_FAILED,
        message: "invalid OpenCode JSON input".to_string(),
        source: Some(err),
    }
}

/// Read stdin into a string.
fn read_stdin() -> Result<String, std::io::Error> {
    use std::io::Read;
    let mut buf = String::new();
    std::io::stdin().read_to_string(&mut buf)?;
    Ok(buf)
}

/// Write JSON value to stdout.
fn write_json_stdout(value: &impl serde::Serialize) -> Result<(), CliError> {
    let json = serde_json::to_string(value).map_err(|e| CliError::Io(std::io::Error::other(e)))?;
    println!("{json}");
    Ok(())
}

fn run_chat_hook() -> Result<(), CliError> {
    let output = opencode::handle_with_failopen(|| {
        let raw = read_stdin().map_err(io_to_rusty)?;
        let input: types::HookInput = serde_json::from_str(&raw).map_err(json_to_rusty)?;

        let cwd = Path::new(&input.cwd);
        opencode::chat_hook::handle_chat_hook(&input, cwd)
    });

    write_json_stdout(&output)
}

fn run_tool_hook() -> Result<(), CliError> {
    let output = opencode::handle_with_failopen(|| {
        let raw = read_stdin().map_err(io_to_rusty)?;
        let input: types::HookInput = serde_json::from_str(&raw).map_err(json_to_rusty)?;

        let cwd = Path::new(&input.cwd);
        opencode::tool_hook::handle_tool_hook(&input, cwd)
    });

    write_json_stdout(&output)
}

fn run_mind() -> Result<(), CliError> {
    let output = opencode::mind_tool_with_failopen(|| {
        let raw = read_stdin().map_err(io_to_rusty)?;
        let input: opencode::types::MindToolInput =
            serde_json::from_str(&raw).map_err(json_to_rusty)?;

        // MindToolInput doesn't carry cwd; use the process working directory
        let cwd = std::env::current_dir().map_err(io_to_rusty)?;
        opencode::mind_tool::handle_mind_tool(&input, &cwd)
    });

    write_json_stdout(&output)
}

fn run_session_cleanup() -> Result<(), CliError> {
    let output = opencode::handle_with_failopen(|| {
        let raw = read_stdin().map_err(io_to_rusty)?;
        let input: types::HookInput = serde_json::from_str(&raw).map_err(json_to_rusty)?;

        let cwd = Path::new(&input.cwd);
        opencode::session_cleanup::handle_session_cleanup(&input.session_id, cwd)
    });

    write_json_stdout(&output)
}

fn run_session_start() -> Result<(), CliError> {
    let output = opencode::handle_with_failopen(|| {
        let raw = read_stdin().map_err(io_to_rusty)?;
        let input: types::HookInput = serde_json::from_str(&raw).map_err(json_to_rusty)?;

        let cwd = Path::new(&input.cwd);
        let sidecar_dir = cwd.join(".opencode");

        // Cleanup stale sidecar files (>24h old)
        opencode::sidecar::cleanup_stale(&sidecar_dir, Duration::from_secs(24 * 3600));

        Ok(types::HookOutput::default())
    });

    write_json_stdout(&output)
}
