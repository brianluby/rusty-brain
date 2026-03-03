// Integration tests for session summary and context assembly round-trip.
//
// Uses real `MemvidStore` backend against temp `.mv2` files.

mod common {
    include!("../common/mod.rs");
}

use rusty_brain_core::mind::Mind;
use types::ObservationType;

// =========================================================================
// T044: Session summary round-trip (SC-002)
// =========================================================================

#[test]
fn save_session_summary_appears_in_context() {
    let (_dir, config) = common::temp_mind_config();
    let mind = Mind::open(config).unwrap();

    mind.save_session_summary(
        vec!["Chose async".to_string()],
        vec!["src/main.rs".to_string()],
        "Productive session implementing types crate",
    )
    .unwrap();

    let ctx = mind.get_context(None).unwrap();
    assert!(
        !ctx.session_summaries.is_empty(),
        "session summary should appear in context"
    );
    assert_eq!(
        ctx.session_summaries[0].summary,
        "Productive session implementing types crate"
    );
    assert_eq!(
        ctx.session_summaries[0].key_decisions,
        vec!["Chose async".to_string()]
    );
}

#[test]
fn multiple_session_summaries_in_context() {
    let (_dir, config) = common::temp_mind_config();
    let mind = Mind::open(config).unwrap();

    mind.save_session_summary(vec![], vec![], "First session")
        .unwrap();
    mind.save_session_summary(vec![], vec![], "Second session")
        .unwrap();

    let ctx = mind.get_context(None).unwrap();
    assert!(
        ctx.session_summaries.len() >= 2,
        "both session summaries should appear in context, got {}",
        ctx.session_summaries.len()
    );
}

#[test]
fn context_includes_both_observations_and_summaries() {
    let (_dir, config) = common::temp_mind_config();
    let mind = Mind::open(config).unwrap();

    mind.remember(
        ObservationType::Discovery,
        "Read",
        "Found a caching pattern",
        Some("LRU cache details"),
        None,
    )
    .unwrap();

    mind.save_session_summary(
        vec!["decision1".to_string()],
        vec![],
        "Session with observation and summary",
    )
    .unwrap();

    let ctx = mind.get_context(None).unwrap();
    assert!(
        !ctx.recent_observations.is_empty(),
        "should include observations"
    );
    assert!(
        !ctx.session_summaries.is_empty(),
        "should include session summaries"
    );
    assert!(ctx.token_count > 0);
}

#[test]
fn context_with_query_finds_relevant_memories() {
    let (_dir, config) = common::temp_mind_config();
    let mind = Mind::open(config).unwrap();

    mind.remember(
        ObservationType::Discovery,
        "Read",
        "caching pattern in service layer",
        Some("Uses an LRU cache with 5-minute TTL"),
        None,
    )
    .unwrap();

    mind.remember(
        ObservationType::Decision,
        "Write",
        "database connection pooling",
        None,
        None,
    )
    .unwrap();

    let ctx = mind.get_context(Some("caching")).unwrap();
    assert!(ctx.token_count > 0);
    // At minimum, caching-related content should be in recent observations.
    let has_caching = ctx.recent_observations.iter().any(|o| {
        o.summary.contains("caching") || o.content.as_deref().unwrap_or("").contains("caching")
    });
    assert!(
        has_caching,
        "context should contain caching-related content"
    );
}
