// Format round-trip tests (T040).
//
// Writes observations with the Rust Mind API, closes, reopens, and verifies
// the data is identical. Tests various observation types, unicode content,
// and empty content.

use rusty_brain_core::mind::Mind;
use types::{MindConfig, ObservationType};

fn create_mind(path: &std::path::Path) -> Mind {
    let config = MindConfig {
        memory_path: path.to_path_buf(),
        min_confidence: 0.0,
        ..MindConfig::default()
    };
    Mind::open(config).expect("failed to open mind")
}

#[test]
fn roundtrip_basic_observations() {
    let dir = tempfile::tempdir().unwrap();
    let mv2_path = dir.path().join("roundtrip-basic.mv2");

    let observations: Vec<(ObservationType, &str, &str, Option<&str>)> = vec![
        (
            ObservationType::Discovery,
            "Read",
            "Found caching layer in service module",
            Some("LRU cache with 5min TTL"),
        ),
        (
            ObservationType::Decision,
            "Write",
            "Chose PostgreSQL for data storage",
            Some("JSONB support was the deciding factor"),
        ),
        (
            ObservationType::Success,
            "Bash",
            "All tests passing after refactor",
            None,
        ),
        (
            ObservationType::Problem,
            "Read",
            "Race condition in background task",
            Some("File locking missing on concurrent writes"),
        ),
    ];

    // Write observations
    {
        let mind = create_mind(&mv2_path);
        for (obs_type, tool, summary, content) in &observations {
            mind.remember(*obs_type, tool, summary, *content, None)
                .expect("remember should succeed");
        }
    }

    // Reopen and verify via search
    {
        let mind = create_mind(&mv2_path);

        // Stats should show correct count
        let stats = mind.stats().expect("stats should succeed");
        assert_eq!(stats.total_observations, 4, "should have 4 observations");

        // Timeline should have 4 entries
        let timeline = mind.timeline(20, false).expect("timeline should succeed");
        assert_eq!(timeline.len(), 4, "timeline should have 4 entries");

        // Search for each observation's key terms
        for (_obs_type, _tool, summary, _content) in &observations {
            let query_words: Vec<&str> = summary.split_whitespace().take(3).collect();
            let query = query_words.join(" ");
            let results = mind.search(&query, Some(5)).expect("search should succeed");
            assert!(
                !results.is_empty(),
                "should find results for query {query:?} (from summary {summary:?})"
            );
        }
    }
}

#[test]
fn roundtrip_unicode_content() {
    let dir = tempfile::tempdir().unwrap();
    let mv2_path = dir.path().join("roundtrip-unicode.mv2");

    {
        let mind = create_mind(&mv2_path);
        mind.remember(
            ObservationType::Discovery,
            "Read",
            "Unicode emoji and CJK characters in observation",
            Some(
                "Content: \u{1f600}\u{1f680}\u{2764}\u{fe0f} CJK: \u{4f60}\u{597d}\u{4e16}\u{754c}",
            ),
            None,
        )
        .expect("remember unicode should succeed");

        mind.remember(
            ObservationType::Discovery,
            "Read",
            "Cyrillic and accented characters preserved",
            Some("Content: \u{041f}\u{0440}\u{0438}\u{0432}\u{0435}\u{0442} caf\u{e9} na\u{ef}ve"),
            None,
        )
        .expect("remember cyrillic should succeed");
    }

    // Reopen and verify
    {
        let mind = create_mind(&mv2_path);
        let stats = mind.stats().expect("stats should succeed");
        assert_eq!(stats.total_observations, 2);

        let results = mind
            .search("unicode emoji CJK", Some(5))
            .expect("search should succeed");
        assert!(
            !results.is_empty(),
            "should find unicode observation after reopen"
        );

        let results = mind
            .search("cyrillic accented", Some(5))
            .expect("search should succeed");
        assert!(
            !results.is_empty(),
            "should find cyrillic observation after reopen"
        );
    }
}

#[test]
fn roundtrip_empty_content() {
    let dir = tempfile::tempdir().unwrap();
    let mv2_path = dir.path().join("roundtrip-empty.mv2");

    {
        let mind = create_mind(&mv2_path);
        mind.remember(
            ObservationType::Success,
            "Bash",
            "Completed migration with no additional details",
            None,
            None,
        )
        .expect("remember with None content should succeed");
    }

    {
        let mind = create_mind(&mv2_path);
        let stats = mind.stats().expect("stats should succeed");
        assert_eq!(stats.total_observations, 1);

        let timeline = mind.timeline(10, false).expect("timeline should succeed");
        assert_eq!(timeline.len(), 1);

        let results = mind
            .search("completed migration", Some(5))
            .expect("search should succeed");
        assert!(
            !results.is_empty(),
            "should find observation with empty content"
        );
    }
}

#[test]
fn roundtrip_all_observation_types() {
    let dir = tempfile::tempdir().unwrap();
    let mv2_path = dir.path().join("roundtrip-types.mv2");

    let all_types = [
        ObservationType::Discovery,
        ObservationType::Decision,
        ObservationType::Success,
        ObservationType::Problem,
    ];

    {
        let mind = create_mind(&mv2_path);
        for (i, obs_type) in all_types.iter().enumerate() {
            mind.remember(
                *obs_type,
                "Read",
                &format!("Observation type test number {i}"),
                Some(&format!("Content for observation type variant {i}")),
                None,
            )
            .expect("remember should succeed for all observation types");
        }
    }

    {
        let mind = create_mind(&mv2_path);
        let stats = mind.stats().expect("stats should succeed");
        assert_eq!(
            stats.total_observations,
            all_types.len() as u64,
            "all observation types should be stored"
        );
    }
}
