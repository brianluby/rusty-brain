//! `rb-install` — the merge/uninstall/status engine for `rusty-brain-install`.
//!
//! Wires the JSON-protocol CLIs (Claude Code, Gemini, Codex) to the
//! `rusty-brain-hooks` binary by deep-merging a sentinel-marked hook block into
//! each CLI's config, and installs OMP's standalone project extension. OpenCode
//! remains deferred. NEVER referenced by any core crate, so the default
//! `rusty-brain` build never compiles it.

pub mod cli;
pub mod detect;
pub mod engine;
pub mod installers;
pub mod report;
pub mod uninstall;
pub mod writer;

pub use detect::{find_binary_on_path, parse_version, version_of};
pub use installers::{
    builtins, ClaudeCodeInstaller, CodexInstaller, GeminiInstaller, OmpInstaller,
};
pub use report::{AgentReport, AgentStatus, InstallError, InstallReport, ReportStatus};
pub use uninstall::{strip_sentinel, uninstall_file};
pub use writer::{backup_path, merge_into_file, merge_value, read_config, write};
