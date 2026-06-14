//! rusty-brain — W3.3 token economy: a faithful BPE token counter plus the
//! token-context budgets.
//!
//! [`count_tokens`] uses the `o200k_base` BPE as a proxy for Claude's non-public
//! tokenizer — the budgets below carry margin for the proxy gap. The vocab is
//! embedded in the `tiktoken-rs` crate, so counting needs no network at runtime.
//! `count_tokens` is fail-safe: a caller inside the strictly fail-open hook
//! binary can never panic on a tokenizer error (it degrades to a char estimate).
#![forbid(unsafe_code)]

use std::sync::OnceLock;

use tiktoken_rs::CoreBPE;

/// Budget (in tokens) for the MCP `tools/list` payload — the largest static
/// surface the model pays for on every turn (W3.3).
pub const TOOLS_LIST_BUDGET: usize = 900;

/// Budget for the MCP `initialize` `instructions` string (W3.3).
pub const INSTRUCTIONS_BUDGET: usize = 150;

/// Budget for one context injection (the SessionStart digest or a
/// UserPromptSubmit recall) fed back into the model (W3.3).
pub const INJECTION_BUDGET: usize = 600;

/// The lazily-initialized BPE, or `None` if it failed to load (then
/// [`count_tokens`] falls back to a char estimate). `OnceLock` so the load
/// happens at most once per process.
fn bpe() -> Option<&'static CoreBPE> {
    static BPE: OnceLock<Option<CoreBPE>> = OnceLock::new();
    BPE.get_or_init(|| tiktoken_rs::o200k_base().ok()).as_ref()
}

/// Count the tokens in `text` with the `o200k_base` BPE (a proxy for Claude's
/// non-public tokenizer). Fail-safe: if the tokenizer cannot initialize, falls
/// back to a conservative `bytes / 4` estimate (rounded up) so the budget still
/// bounds the output and a fail-open caller never panics.
#[must_use]
pub fn count_tokens(text: &str) -> usize {
    match bpe() {
        Some(bpe) => bpe.encode_ordinary(text).len(),
        None => text.len().div_ceil(4),
    }
}

/// The longest char-boundary prefix of `text` whose token count is ≤
/// `max_tokens`. Used as a hard last-resort guard so a single pathological line
/// (dense CJK/emoji, where 1 char can be several tokens) cannot blow a budget.
/// Returns the whole string when it already fits; `""` when even one char
/// exceeds `max_tokens`. Binary search → O(log n) `count_tokens` calls.
#[must_use]
pub fn truncate_to_tokens(text: &str, max_tokens: usize) -> &str {
    if count_tokens(text) <= max_tokens {
        return text;
    }
    // Char-boundary byte offsets (including the end), so every slice is valid UTF-8.
    let bounds: Vec<usize> = text
        .char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(text.len()))
        .collect();
    let (mut lo, mut hi) = (0usize, bounds.len() - 1);
    while lo < hi {
        let mid = lo + (hi - lo).div_ceil(2);
        if count_tokens(&text[..bounds[mid]]) <= max_tokens {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    &text[..bounds[lo]]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_bpe_loads_offline() {
        // The embedded o200k_base vocab must initialize with no network — else
        // the token-accounting guards silently degrade to the char estimate.
        assert!(bpe().is_some(), "o200k_base must initialize offline");
    }

    #[test]
    fn count_is_zero_empty_positive_nonempty_and_monotonic() {
        assert_eq!(count_tokens(""), 0, "empty text is 0 tokens");
        let short = count_tokens("hello world");
        let long = count_tokens("hello world — a noticeably longer sentence with more tokens.");
        assert!(short > 0, "non-empty text has > 0 tokens");
        assert!(long > short, "more text => more tokens");
    }

    #[test]
    fn truncate_to_tokens_bounds_and_is_char_safe() {
        let s = "the quick brown fox jumps over the lazy dog ".repeat(50);
        let cut = truncate_to_tokens(&s, 20);
        assert!(count_tokens(cut) <= 20, "truncated to <= 20 tokens");
        assert!(cut.len() < s.len(), "actually truncated");
        // A string that already fits is returned whole.
        assert_eq!(truncate_to_tokens("hello world", 100), "hello world");
        // Multibyte input is cut on a char boundary (never panics).
        let cjk = "日本語のテキストを切り詰める".repeat(10);
        let cut = truncate_to_tokens(&cjk, 5);
        assert!(count_tokens(cut) <= 5);
        assert!(cjk.is_char_boundary(cut.len()), "cut on a char boundary");
    }
}
