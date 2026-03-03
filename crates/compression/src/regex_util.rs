//! Regex compilation helper that preserves the crate's infallible API contract.

use regex::Regex;

/// Compile a regex pattern, returning a never-matching fallback on failure.
///
/// All regex patterns in this crate are string literals that are validated by
/// the test suite. This helper exists as a defense-in-depth measure: if a
/// pattern is ever edited incorrectly, compression degrades gracefully (the
/// broken pattern silently matches nothing) instead of panicking at runtime.
///
/// On failure, a `WARN` is emitted via `tracing` and a no-match regex is returned.
pub(crate) fn compile(pattern: &str, name: &str) -> Regex {
    match Regex::new(pattern) {
        Ok(re) => re,
        Err(err) => {
            tracing::warn!(
                pattern_name = name,
                "BUG: invalid regex literal, using no-match fallback: {err}"
            );
            // `\z.` requires a character after absolute end-of-input — impossible.
            Regex::new(r"\z.").expect("fallback no-match regex must compile")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_pattern_compiles() {
        let re = compile(r"hello", "test");
        assert!(re.is_match("hello world"));
    }

    #[test]
    fn invalid_pattern_returns_no_match_fallback() {
        // Deliberately invalid pattern (unmatched group)
        let re = compile(r"(unclosed", "test_invalid");
        assert!(!re.is_match("anything"));
        assert!(!re.is_match("(unclosed"));
    }
}
