//! `rusty_brain` binary library: clap CLI, namespace detection, daemon/client glue.
//!
//! Logic lives here (testable directly); `main.rs` is a thin shell that parses
//! args and dispatches. Later tasks add `paths`, `namespace_detect`, `logging`,
//! `output`, `serve`, `client`, and `run`.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod cli;
pub mod client;
pub mod logging;
pub mod namespace_detect;
pub mod output;
pub mod paths;
pub mod serve;
