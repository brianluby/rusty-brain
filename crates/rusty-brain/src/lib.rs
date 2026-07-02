//! `rusty_brain` binary library: clap CLI, namespace detection, daemon/client glue.
//!
//! Logic lives here (testable directly); `main.rs` is a thin shell that parses
//! args and dispatches through the modules below.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod cli;
pub mod client;
pub mod import;
pub mod logging;
pub mod mcp;
pub mod namespace_detect;
pub mod output;
pub mod paths;
pub mod run;
pub mod serve;
