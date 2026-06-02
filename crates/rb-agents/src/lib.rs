//! `rb-agents` — CLI-agnostic spine for the agent hook + install surface.
//!
//! Defines the canonical hook event model, the per-CLI `AgentCli` JSON adapter
//! trait + registry, a strictly fail-open `DaemonClient` over `rb_proto`, a
//! self-contained namespace detector, and the install-side `AgentInstaller`
//! contract. NEVER referenced by any core crate: kept out of the default build
//! closure so the `rusty-brain` binary never links it.
#![forbid(unsafe_code)]

mod event;

pub use event::{HookContext, HookEvent, HookResult};
