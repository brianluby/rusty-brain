//! `OpenCode` editor adapter for rusty-brain.
//!
//! Provides hook handlers and a native mind tool for `OpenCode` integration.
//! All handlers are fail-open: errors and panics produce valid default output.
//! No stdin/stdout I/O — the CLI layer handles all I/O.

pub mod bootstrap;

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

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // handle_with_failopen
    // -----------------------------------------------------------------------

    #[test]
    fn handle_with_failopen_returns_ok_output_on_success() {
        let output = handle_with_failopen(|| {
            Ok(::types::HookOutput {
                system_message: Some("hello".to_string()),
                ..Default::default()
            })
        });
        assert_eq!(output.system_message, Some("hello".to_string()));
    }

    #[test]
    fn handle_with_failopen_returns_default_on_error() {
        let output = handle_with_failopen(|| {
            Err(::types::RustyBrainError::FileSystem {
                code: ::types::error_codes::E_FS_NOT_FOUND,
                message: "test error".to_string(),
                source: None,
            })
        });
        assert_eq!(output, ::types::HookOutput::default());
    }

    #[test]
    fn handle_with_failopen_returns_default_on_panic() {
        let output = handle_with_failopen(|| {
            panic!("intentional test panic");
        });
        assert_eq!(output, ::types::HookOutput::default());
    }

    // -----------------------------------------------------------------------
    // mind_tool_with_failopen
    // -----------------------------------------------------------------------

    #[test]
    fn mind_tool_with_failopen_returns_ok_output_on_success() {
        let data = serde_json::json!({"result": "ok"});
        let output = mind_tool_with_failopen(|| Ok(MindToolOutput::success(data.clone())));
        assert!(output.success);
        assert_eq!(output.data, Some(data));
    }

    #[test]
    fn mind_tool_with_failopen_returns_error_output_on_error() {
        let output = mind_tool_with_failopen(|| {
            Err(::types::RustyBrainError::FileSystem {
                code: ::types::error_codes::E_FS_IO_ERROR,
                message: "disk full".to_string(),
                source: None,
            })
        });
        assert!(!output.success);
        assert_eq!(output.error.as_deref(), Some("internal error"));
    }

    #[test]
    fn mind_tool_with_failopen_returns_error_output_on_panic() {
        let output = mind_tool_with_failopen(|| {
            panic!("intentional test panic");
        });
        assert!(!output.success);
        assert_eq!(output.error.as_deref(), Some("internal error"));
    }
}
