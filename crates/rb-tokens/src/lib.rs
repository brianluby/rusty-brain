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
}
