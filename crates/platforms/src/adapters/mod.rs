//! Built-in platform adapters (Claude Code, OpenCode).

/// Claude Code platform adapter.
pub mod claude;
/// OpenCode platform adapter.
pub mod opencode;

pub use claude::claude_adapter;
pub use opencode::opencode_adapter;
