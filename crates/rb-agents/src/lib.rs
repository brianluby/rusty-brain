//! `rb-agents` — CLI-agnostic spine for the agent hook + install surface.
//!
//! Defines the canonical hook event model, the per-CLI `AgentCli` JSON adapter
//! trait + registry, a strictly fail-open `DaemonClient` over `rb_proto`, a
//! self-contained namespace detector, and the install-side `AgentInstaller`
//! contract. NEVER referenced by any core crate: kept out of the default build
//! closure so the `rusty-brain` binary never links it.
#![forbid(unsafe_code)]

mod claude_code;
mod cli;
mod codex;
mod daemon;
mod event;
mod gemini;
mod install;
mod namespace;
mod opencode;

pub use claude_code::ClaudeCodeCli;
pub use cli::{agent_for, AgentCli, AgentId, PassthroughCli};
pub use codex::CodexCli;
pub use daemon::{AutoStart, DaemonClient};
pub use event::{HookContext, HookEvent, HookResult};
pub use gemini::GeminiCli;
pub use install::{AgentInstaller, HookFragment, InstallScope, SENTINEL};
pub use namespace::detect_namespace;
pub use opencode::OpenCodeCli;
