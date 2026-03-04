//! `OpenCode` editor adapter for rusty-brain.
//!
//! Provides hook handlers and a native mind tool for `OpenCode` integration.
//! All handlers are fail-open: errors and panics produce valid default output.
//! No stdin/stdout I/O — the CLI layer handles all I/O.

pub mod chat_hook;
pub mod mind_tool;
pub mod session_cleanup;
pub mod sidecar;
pub mod tool_hook;
pub mod types;

use std::panic::AssertUnwindSafe;

use crate::types::MindToolOutput;

/// Execute a handler function with fail-open error and panic recovery.
///
/// Catches both `Result::Err` and panics, returning a valid default `HookOutput`.
/// Emits `tracing::warn!` for all caught errors and panics (SEC-10).
pub fn handle_with_failopen<F>(handler: F) -> ::types::HookOutput
where
    F: FnOnce() -> Result<::types::HookOutput, ::types::RustyBrainError>,
{
    match std::panic::catch_unwind(AssertUnwindSafe(handler)) {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "handler failed, fail-open");
            ::types::HookOutput::default()
        }
        Err(_panic) => {
            tracing::warn!("handler panicked, fail-open");
            ::types::HookOutput::default()
        }
    }
}

/// Fail-open wrapper for `MindToolOutput` (mind tool handlers).
pub fn mind_tool_with_failopen<F>(handler: F) -> MindToolOutput
where
    F: FnOnce() -> Result<MindToolOutput, ::types::RustyBrainError>,
{
    match std::panic::catch_unwind(AssertUnwindSafe(handler)) {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "mind tool failed, fail-open");
            MindToolOutput::error("internal error")
        }
        Err(_panic) => {
            tracing::warn!("mind tool panicked, fail-open");
            MindToolOutput::error("internal error")
        }
    }
}
