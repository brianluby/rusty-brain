//! Token estimation utilities.
//!
//! Provides a fast, allocation-free heuristic for estimating the number of
//! tokens in a text string (characters / 4).

/// Estimate the number of tokens in a text string.
///
/// Uses the simple heuristic of `text.len() / 4` (byte count divided by 4).
/// This is a fast approximation suitable for budget enforcement, not exact
/// tokenizer output.
///
/// **Design note:** We intentionally use byte count (`len()`) rather than
/// character count (`chars().count()`). For ASCII text the two are identical.
/// For multi-byte UTF-8 (CJK, emoji), byte count overestimates character
/// count, producing a *conservative* token budget (we'd rather under-fill
/// context than overflow it). This also avoids an O(n) char iteration and
/// is allocation-free.
#[must_use]
pub fn estimate_tokens(text: &str) -> usize {
    text.len() / 4
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_string_returns_zero() {
        assert_eq!(estimate_tokens(""), 0);
    }

    #[test]
    fn short_string_returns_expected() {
        // "Hello" = 5 bytes / 4 = 1
        assert_eq!(estimate_tokens("Hello"), 1);
    }

    #[test]
    fn longer_string_returns_expected() {
        // "Hello world" = 11 bytes / 4 = 2
        assert_eq!(estimate_tokens("Hello world"), 2);
    }

    #[test]
    fn exact_multiple_returns_expected() {
        // "abcd" = 4 bytes / 4 = 1
        assert_eq!(estimate_tokens("abcd"), 1);
        // "abcdefgh" = 8 bytes / 4 = 2
        assert_eq!(estimate_tokens("abcdefgh"), 2);
    }

    #[test]
    fn unicode_uses_byte_count() {
        // "🧠" is 4 UTF-8 bytes → 4/4 = 1
        assert_eq!(estimate_tokens("🧠"), 1);
        // "你好" is 6 UTF-8 bytes → 6/4 = 1
        assert_eq!(estimate_tokens("你好"), 1);
        // "مرحبا" is 10 UTF-8 bytes → 10/4 = 2
        assert_eq!(estimate_tokens("مرحبا"), 2);
    }

    #[test]
    fn long_string_returns_expected() {
        let text = "a".repeat(1000);
        assert_eq!(estimate_tokens(&text), 250);
    }
}
