//! Opt-in background "evolution" jobs: bounded, idempotent maintenance passes
//! that read via the read pool and mutate ONLY through the single writer.

mod config;

pub use config::{ConsolidationConfig, ImportanceConfig, JobsConfig, LinkDecayConfig};
