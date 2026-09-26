//! Adapters that run third-party memory benchmarks against rusty-brain
//! (Vikunja #60). Pins, checksums and commands live in
//! `crates/rb-eval/external/manifest.json`; datasets stay outside git.

pub mod longmemeval;
pub mod metrics;
