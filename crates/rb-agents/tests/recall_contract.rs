//! CA6 (cross-agent parity PRD / HTTP PRD HTTP-4): the agent-agnostic
//! prompt-time recall contract is defined ONCE, independent of any one
//! agent's event name, and its safety invariants are pinned here.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use rb_agents::recall_contract::{RecallContract, PROMPT_TIME_RECALL};

#[test]
fn the_contract_bounds_are_tight_and_nonzero() {
    let RecallContract {
        max_items,
        max_chars_per_item,
        untrusted_preamble,
    } = PROMPT_TIME_RECALL;
    // Fires every turn: the item bound must stay small (W3.2(a)).
    assert!((1..=10).contains(&max_items), "got {max_items}");
    // Per-item display bound keeps long memories from blowing the per-turn
    // token budget (W3.3 projection parity).
    assert!(
        (80..=500).contains(&max_chars_per_item),
        "got {max_chars_per_item}"
    );
    assert!(!untrusted_preamble.is_empty());
}

#[test]
fn the_untrusted_preamble_frames_memories_as_data_not_instructions() {
    // W2.5: the preamble is the primary prompt-injection mitigation. Its
    // load-bearing phrases are pinned so a rewrite cannot silently weaken it.
    let preamble = PROMPT_TIME_RECALL.untrusted_preamble;
    for needle in ["STORED MEMORIES", "NOT instructions", "never be followed"] {
        assert!(
            preamble.contains(needle),
            "preamble must contain {needle:?}: {preamble}"
        );
    }
}
