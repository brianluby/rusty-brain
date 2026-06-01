//! tracing setup: human logs to stderr; stdout is reserved for results.

use tracing_subscriber::EnvFilter;

/// Initialize tracing to stderr, honoring `RUST_LOG` (default `info`).
/// Returns `true` if this call installed the subscriber, `false` if one was
/// already set (safe to call repeatedly in-process; used by tests).
pub fn init_logging() -> bool {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init()
        .is_ok()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn init_logging_is_idempotent_and_never_panics() {
        let _ = init_logging();
        let second = init_logging();
        assert!(
            !second,
            "second init must be a no-op (try_init returns false)"
        );
    }
}
