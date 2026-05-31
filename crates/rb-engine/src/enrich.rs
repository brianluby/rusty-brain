#![allow(dead_code)]

/// Maximum characters retained in a heuristic summary.
const SUMMARY_MAX_CHARS: usize = 150;
/// Maximum number of derived keywords.
const MAX_KEYWORDS: usize = 5;
/// Minimum token length (in characters) kept as a keyword (drops stop-word-ish short tokens).
const MIN_KEYWORD_LEN: usize = 4;

/// Heuristic summary: trim, then keep the first `SUMMARY_MAX_CHARS` characters
/// on a char boundary (never splitting a multi-byte UTF-8 sequence).
pub(crate) fn default_summary(content: &str) -> String {
    let trimmed = content.trim();
    trimmed.chars().take(SUMMARY_MAX_CHARS).collect()
}

/// Heuristic keyword extraction: split on non-alphanumeric, lowercase, keep
/// tokens of length >= `MIN_KEYWORD_LEN` characters, dedupe preserving
/// first-seen order, and cap at `MAX_KEYWORDS`. Pure and deterministic.
pub(crate) fn derive_keywords(content: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in content.split(|c: char| !c.is_alphanumeric()) {
        if raw.chars().count() < MIN_KEYWORD_LEN {
            continue;
        }
        let token = raw.to_lowercase();
        if !out.iter().any(|existing| existing == &token) {
            out.push(token);
        }
        if out.len() == MAX_KEYWORDS {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn summary_of_short_content_is_unchanged_trimmed() {
        assert_eq!(default_summary("  short body  "), "short body");
    }

    #[test]
    fn summary_truncates_to_150_chars_on_char_boundary() {
        let content = "x".repeat(400);
        let s = default_summary(&content);
        assert_eq!(s.chars().count(), 150);
    }

    #[test]
    fn summary_truncation_never_splits_a_multibyte_char() {
        // 200 'é' (2 bytes each in UTF-8); truncation must stay on a char boundary.
        let content = "é".repeat(200);
        let s = default_summary(&content);
        assert_eq!(s.chars().count(), 150);
        // Round-trips as valid UTF-8 (would panic on a bad boundary while building).
        assert!(s.chars().all(|c| c == 'é'));
    }

    #[test]
    fn keywords_lowercase_dedupe_and_cap_at_five() {
        let kw =
            derive_keywords("SQLite WAL mode enables concurrent SQLITE readers and writers safely");
        // length >= 4 (by char count), lowercased, order-preserving, deduped, max 5.
        assert_eq!(
            kw,
            vec!["sqlite", "mode", "enables", "concurrent", "readers"]
        );
    }

    #[test]
    fn keywords_skips_short_tokens_and_punctuation() {
        let kw = derive_keywords("a an the to of, big-decision: keep!");
        assert_eq!(kw, vec!["decision", "keep"]);
    }

    #[test]
    fn keywords_empty_content_yields_empty() {
        assert!(derive_keywords("   ").is_empty());
    }

    #[test]
    fn keywords_length_guard_uses_char_count_not_bytes() {
        // "café" is 5 bytes but 4 chars; "über" is 5 bytes but 4 chars.
        // Both must be kept (>= 4 chars), proving the guard counts chars, not bytes.
        let kw = derive_keywords("café über");
        assert_eq!(kw, vec!["café", "über"]);
    }
}
