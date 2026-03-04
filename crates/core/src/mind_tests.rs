use super::*;
use crate::backend::MockBackend;

/// Create a Mind with `MockBackend` for unit testing.
///
/// Returns `(TempDir, Mind)` — caller must hold the `TempDir` for the
/// test's lifetime so the temp directory is not deleted prematurely.
fn test_mind() -> (tempfile::TempDir, Mind) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.mv2");
    let config = MindConfig {
        memory_path: path,
        ..MindConfig::default()
    };
    let mind = Mind::open_with_backend(config, Box::new(MockBackend::new())).unwrap();
    (dir, mind)
}

// =========================================================================
// T020: Mind::open (create new file)
// =========================================================================

#[test]
fn mind_open_creates_new_file_and_initializes() {
    let (_dir, mind) = test_mind();
    assert!(mind.is_initialized());
    assert!(!mind.session_id().is_empty());
    assert!(mind.memory_path().to_string_lossy().ends_with(".mv2"));
}

#[test]
fn mind_open_generates_valid_ulid_session_id() {
    let (_dir, mind) = test_mind();
    let sid = mind.session_id();
    // ULID is 26 chars, lowercase.
    assert_eq!(sid.len(), 26, "session_id should be 26-char ULID");
    assert!(
        sid.chars().all(|c| c.is_ascii_alphanumeric()),
        "session_id should be alphanumeric"
    );
}

// =========================================================================
// T021: Mind::open (open existing file)
// =========================================================================

#[test]
fn mind_open_existing_file_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("existing.mv2");
    std::fs::write(&path, b"existing data").unwrap();

    let config = MindConfig {
        memory_path: path,
        ..MindConfig::default()
    };
    let mind = Mind::open_with_backend(config, Box::new(MockBackend::new())).unwrap();
    assert!(mind.is_initialized());
}

#[test]
fn mind_open_read_only_missing_file_does_not_create_parent_dirs() {
    let dir = tempfile::tempdir().unwrap();
    let parent = dir.path().join("nested").join("deep");
    let path = parent.join("readonly.mv2");
    let config = MindConfig {
        memory_path: path,
        ..MindConfig::default()
    };

    let result = Mind::open_read_only(config);
    assert!(result.is_err(), "missing read-only file should fail");
    assert!(
        !parent.exists(),
        "open_read_only must not create parent directories"
    );
}

// =========================================================================
// T022: Mind::remember
// =========================================================================

#[test]
fn mind_remember_returns_ulid() {
    let (_dir, mind) = test_mind();
    let id = mind
        .remember(
            ObservationType::Discovery,
            "Read",
            "Found a caching pattern",
            Some("LRU cache in service layer"),
            None,
        )
        .unwrap();
    assert_eq!(id.len(), 26, "returned ID should be a 26-char ULID");
    assert!(id.chars().all(|c| c.is_ascii_alphanumeric()));
}

#[test]
fn mind_remember_rejects_empty_summary() {
    let (_dir, mind) = test_mind();
    let result = mind.remember(ObservationType::Discovery, "Read", "", None, None);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.code(), error_codes::E_INPUT_EMPTY_FIELD);
}

#[test]
fn mind_remember_rejects_whitespace_only_summary() {
    let (_dir, mind) = test_mind();
    let result = mind.remember(ObservationType::Discovery, "Read", "   ", None, None);
    assert!(result.is_err());
}

#[test]
fn mind_remember_rejects_empty_tool_name() {
    let (_dir, mind) = test_mind();
    let result = mind.remember(
        ObservationType::Discovery,
        "",
        "A valid summary",
        None,
        None,
    );
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.code(), error_codes::E_INPUT_EMPTY_FIELD);
}

#[test]
fn mind_remember_content_is_optional() {
    let (_dir, mind) = test_mind();
    let id = mind
        .remember(
            ObservationType::Decision,
            "Write",
            "Chose async over sync",
            None,
            None,
        )
        .unwrap();
    assert_eq!(id.len(), 26);
}

// =========================================================================
// T023: Mind::search
// =========================================================================

#[test]
fn mind_search_returns_stored_observations() {
    let (_dir, mind) = test_mind();
    mind.remember(
        ObservationType::Discovery,
        "Read",
        "Found caching pattern in service layer",
        Some("LRU cache with 5-minute TTL"),
        None,
    )
    .unwrap();

    let results = mind.search("caching pattern", None).unwrap();
    assert!(
        !results.is_empty(),
        "search should find the stored observation"
    );

    let r = &results[0];
    assert_eq!(r.obs_type, ObservationType::Discovery);
    assert_eq!(r.summary, "Found caching pattern in service layer");
    assert!(r.score > 0.0);
    assert_eq!(r.tool_name, "Read");
    assert!(r.timestamp <= Utc::now());
}

#[test]
fn mind_search_empty_results_on_no_match() {
    let (_dir, mind) = test_mind();
    let results = mind.search("nonexistent query", None).unwrap();
    assert!(results.is_empty());
}

#[test]
fn mind_search_respects_limit() {
    let (_dir, mind) = test_mind();
    for i in 0..5 {
        mind.remember(
            ObservationType::Discovery,
            "Read",
            &format!("pattern discovery number {i}"),
            None,
            None,
        )
        .unwrap();
    }

    let results = mind.search("pattern discovery", Some(2)).unwrap();
    assert!(results.len() <= 2, "limit should cap results");
}

// =========================================================================
// T024: Mind::ask
// =========================================================================

#[test]
fn mind_ask_returns_relevant_content() {
    let (_dir, mind) = test_mind();
    mind.remember(
        ObservationType::Discovery,
        "Read",
        "caching is done via LRU in the service layer",
        None,
        None,
    )
    .unwrap();

    let answer = mind.ask("caching").unwrap();
    let text = answer.expect("ask should return Some for matching content");
    assert!(
        text.contains("caching") || text.contains("LRU"),
        "ask should return relevant content, got: {text}"
    );
}

#[test]
fn mind_ask_returns_none_when_no_matches() {
    let (_dir, mind) = test_mind();
    let answer = mind
        .ask("something that definitely does not exist")
        .unwrap();
    assert!(answer.is_none(), "ask should return None for no matches");
}

// =========================================================================
// T025: Error wrapping
// =========================================================================

#[test]
fn mind_remember_errors_are_rusty_brain_error() {
    let (_dir, mind) = test_mind();
    let err = mind
        .remember(ObservationType::Discovery, "Read", "", None, None)
        .unwrap_err();
    assert!(
        !err.code().is_empty(),
        "error should have a stable non-empty code"
    );
}

// =========================================================================
// T026: Accessors
// =========================================================================

#[test]
fn mind_session_id_is_valid_ulid() {
    let (_dir, mind) = test_mind();
    let sid = mind.session_id();
    assert_eq!(sid.len(), 26);
    assert!(
        ulid::Ulid::from_string(&sid.to_uppercase()).is_ok(),
        "session_id should parse as ULID"
    );
}

#[test]
fn mind_memory_path_matches_config() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("specific.mv2");
    let config = MindConfig {
        memory_path: path.clone(),
        ..MindConfig::default()
    };
    let mind = Mind::open_with_backend(config, Box::new(MockBackend::new())).unwrap();
    assert_eq!(mind.memory_path(), path);
}

#[test]
fn mind_is_initialized_returns_true_after_open() {
    let (_dir, mind) = test_mind();
    assert!(mind.is_initialized());
}

// =========================================================================
// Mind::timeline
// =========================================================================

#[test]
fn mind_timeline_empty_returns_empty_vec() {
    let (_dir, mind) = test_mind();
    let entries = mind.timeline(10, true).unwrap();
    assert!(entries.is_empty());
}

#[test]
fn mind_timeline_reverse_order_most_recent_first() {
    let (_dir, mind) = test_mind();
    mind.remember(ObservationType::Discovery, "Read", "first obs", None, None)
        .unwrap();
    mind.remember(ObservationType::Decision, "Write", "second obs", None, None)
        .unwrap();
    mind.remember(ObservationType::Bugfix, "Bash", "third obs", None, None)
        .unwrap();

    let entries = mind.timeline(10, true).unwrap();
    assert_eq!(entries.len(), 3);
    // Most recent first (reverse=true)
    assert_eq!(entries[0].summary, "third obs");
    assert_eq!(entries[0].obs_type, ObservationType::Bugfix);
    assert_eq!(entries[2].summary, "first obs");
    assert_eq!(entries[2].obs_type, ObservationType::Discovery);
}

#[test]
fn mind_timeline_chronological_order_oldest_first() {
    let (_dir, mind) = test_mind();
    mind.remember(ObservationType::Discovery, "Read", "first obs", None, None)
        .unwrap();
    mind.remember(ObservationType::Decision, "Write", "second obs", None, None)
        .unwrap();

    let entries = mind.timeline(10, false).unwrap();
    assert_eq!(entries.len(), 2);
    // Oldest first (reverse=false)
    assert_eq!(entries[0].summary, "first obs");
    assert_eq!(entries[1].summary, "second obs");
}

#[test]
fn mind_timeline_limit_respected() {
    let (_dir, mind) = test_mind();
    for i in 0..20 {
        mind.remember(
            ObservationType::Discovery,
            "Read",
            &format!("observation {i}"),
            None,
            None,
        )
        .unwrap();
    }

    let entries = mind.timeline(5, true).unwrap();
    assert_eq!(entries.len(), 5);
}

#[test]
fn mind_timeline_metadata_parsing() {
    let (_dir, mind) = test_mind();
    let _id = mind
        .remember(
            ObservationType::Solution,
            "Bash",
            "Fixed the caching issue",
            Some("Applied LRU eviction policy"),
            None,
        )
        .unwrap();

    let entries = mind.timeline(10, true).unwrap();
    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert_eq!(entry.obs_type, ObservationType::Solution);
    assert_eq!(entry.summary, "Fixed the caching issue");
    assert_eq!(entry.tool_name, "Bash");
    assert!(entry.timestamp <= Utc::now());
}

#[test]
fn mind_timeline_malformed_metadata_uses_fallbacks() {
    use crate::backend::MockBackend;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.mv2");
    let config = MindConfig {
        memory_path: path.clone(),
        ..MindConfig::default()
    };
    let backend = MockBackend::new();

    // Store a frame with no metadata fields (malformed)
    backend.create(&path).unwrap();
    backend
        .put(
            b"some preview text for fallback",
            &[],
            &[],
            &serde_json::json!({}),
        )
        .unwrap();

    let mind = Mind::open_with_backend(config, Box::new(backend)).unwrap();
    let entries = mind.timeline(10, true).unwrap();
    assert_eq!(entries.len(), 1);

    let entry = &entries[0];
    // Fallback values per contract
    assert_eq!(entry.obs_type, ObservationType::Discovery);
    assert_eq!(entry.tool_name, "unknown");
    // Summary falls back to preview text
    assert!(entry.summary.contains("some preview text"));
    // Timestamp falls back to now (approximately)
    assert!(entry.timestamp <= Utc::now());
}

#[test]
fn mind_timeline_remember_round_trip() {
    let (_dir, mind) = test_mind();
    mind.remember(
        ObservationType::Pattern,
        "read_file",
        "Repository pattern detected",
        Some("Found in service layer"),
        None,
    )
    .unwrap();

    let entries = mind.timeline(10, true).unwrap();
    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert_eq!(entry.obs_type, ObservationType::Pattern);
    assert_eq!(entry.summary, "Repository pattern detected");
    assert_eq!(entry.tool_name, "read_file");
}

// =========================================================================
// Mind::get_context
// =========================================================================

#[test]
fn mind_get_context_returns_context_with_observations() {
    let (_dir, mind) = test_mind();
    mind.remember(
        ObservationType::Discovery,
        "Read",
        "Found caching pattern",
        Some("LRU cache"),
        None,
    )
    .unwrap();

    let ctx = mind.get_context(None).unwrap();
    assert!(
        !ctx.recent_observations.is_empty(),
        "get_context should include recent observations"
    );
    assert!(ctx.token_count > 0);
}

#[test]
fn mind_get_context_with_query_populates_relevant() {
    let (_dir, mind) = test_mind();
    // Store observations: the first one matches "caching", the last is most recent.
    mind.remember(
        ObservationType::Discovery,
        "Read",
        "Found caching pattern",
        Some("LRU cache details"),
        None,
    )
    .unwrap();
    mind.remember(
        ObservationType::Decision,
        "Write",
        "Chose async approach",
        None,
        None,
    )
    .unwrap();
    mind.remember(
        ObservationType::Success,
        "Bash",
        "Completed setup tasks",
        None,
        None,
    )
    .unwrap();

    // All three are in recent (max_context_observations defaults to 20).
    // The "caching" one is also a relevant match but gets deduplicated.
    let ctx = mind.get_context(Some("caching")).unwrap();
    assert!(!ctx.recent_observations.is_empty());
    assert!(ctx.token_count > 0);
}

#[test]
fn mind_get_context_empty_returns_empty() {
    let (_dir, mind) = test_mind();
    let ctx = mind.get_context(None).unwrap();
    assert!(ctx.recent_observations.is_empty());
    assert!(ctx.relevant_memories.is_empty());
    assert!(ctx.session_summaries.is_empty());
    assert_eq!(ctx.token_count, 0);
}

// =========================================================================
// T043: Mind::save_session_summary
// =========================================================================

#[test]
fn mind_save_session_summary_returns_ulid() {
    let (_dir, mind) = test_mind();
    let id = mind
        .save_session_summary(
            vec!["Chose async".to_string()],
            vec!["src/main.rs".to_string()],
            "Productive session",
        )
        .unwrap();
    assert_eq!(id.len(), 26, "should return a 26-char ULID");
}

#[test]
fn mind_save_session_summary_stored_as_decision() {
    let (_dir, mind) = test_mind();
    mind.save_session_summary(
        vec!["decision1".to_string()],
        vec!["file1.rs".to_string()],
        "Test session",
    )
    .unwrap();

    let results = mind.search("session_summary", None).unwrap();
    assert!(!results.is_empty(), "should find the session summary");
    assert_eq!(results[0].obs_type, ObservationType::Decision);
}

#[test]
fn mind_save_session_summary_appears_in_context() {
    let (_dir, mind) = test_mind();
    mind.save_session_summary(
        vec!["decision1".to_string()],
        vec!["file1.rs".to_string()],
        "Productive session implementing types",
    )
    .unwrap();

    let ctx = mind.get_context(None).unwrap();
    assert!(
        !ctx.session_summaries.is_empty(),
        "session summary should appear in context"
    );
    assert_eq!(
        ctx.session_summaries[0].summary,
        "Productive session implementing types"
    );
}

#[test]
fn mind_save_session_summary_rejects_empty_summary() {
    let (_dir, mind) = test_mind();
    let result = mind.save_session_summary(vec![], vec![], "");
    assert!(result.is_err());
}

#[test]
fn mind_save_session_summary_preserves_decisions_and_files() {
    let (_dir, mind) = test_mind();
    mind.save_session_summary(
        vec!["use async".to_string(), "pin deps".to_string()],
        vec!["src/main.rs".to_string(), "Cargo.toml".to_string()],
        "Session with decisions",
    )
    .unwrap();

    let ctx = mind.get_context(None).unwrap();
    assert!(!ctx.session_summaries.is_empty());
    let s = &ctx.session_summaries[0];
    assert_eq!(s.key_decisions.len(), 2);
    assert_eq!(s.modified_files.len(), 2);
}

// =========================================================================
// T054: Mind::with_lock
// =========================================================================

#[test]
fn mind_with_lock_executes_closure() {
    let (_dir, mind) = test_mind();
    let result = mind.with_lock(|m| {
        m.remember(
            ObservationType::Discovery,
            "Read",
            "locked write",
            None,
            None,
        )
    });
    assert!(result.is_ok());
    assert_eq!(result.unwrap().len(), 26);
}

#[test]
fn mind_with_lock_returns_closure_result() {
    let (_dir, mind) = test_mind();
    let answer = mind.with_lock(|m| m.ask("nonexistent")).unwrap();
    assert!(answer.is_none(), "ask should return None for no matches");
}

#[cfg(unix)]
#[test]
fn mind_with_lock_creates_lock_file_with_0600_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let (_dir, mind) = test_mind();
    mind.with_lock(|m| m.remember(ObservationType::Discovery, "Read", "test", None, None))
        .unwrap();

    let mut lock_os = mind.memory_path().as_os_str().to_os_string();
    lock_os.push(".lock");
    let lock_path = std::path::PathBuf::from(lock_os);
    assert!(lock_path.exists(), "lock file should exist");

    let perms = std::fs::metadata(&lock_path).unwrap().permissions();
    assert_eq!(
        perms.mode() & 0o777,
        0o600,
        "lock file should have 0600 permissions"
    );
}

#[test]
fn mind_with_lock_propagates_closure_error() {
    let (_dir, mind) = test_mind();
    let result: Result<String, _> =
        mind.with_lock(|m| m.remember(ObservationType::Discovery, "Read", "", None, None));
    assert!(result.is_err(), "should propagate closure error");
}

// =========================================================================
// T050: Mind::open corruption detection and recovery
// =========================================================================

#[test]
fn mind_open_recovers_from_corrupted_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("corrupted.mv2");
    // Write garbage that is not a valid .mv2 file.
    std::fs::write(&path, b"this is not a valid mv2 file garbage data").unwrap();

    let config = MindConfig {
        memory_path: path.clone(),
        ..MindConfig::default()
    };

    // Mind::open should recover: backup corrupted file + create fresh.
    let mind = Mind::open(config).unwrap();
    assert!(mind.is_initialized());

    // Backup file should exist.
    let backups: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(std::result::Result::ok)
        .filter(|e| e.file_name().to_string_lossy().contains(".backup-"))
        .collect();
    assert_eq!(backups.len(), 1, "backup of corrupted file should exist");

    // Fresh store should be functional.
    let id = mind
        .remember(ObservationType::Discovery, "Read", "test obs", None, None)
        .unwrap();
    assert_eq!(id.len(), 26);
}

// =========================================================================
// T046: Mind::stats (computation)
// =========================================================================

#[test]
fn mind_stats_reports_total_observations() {
    let (_dir, mind) = test_mind();
    mind.remember(ObservationType::Discovery, "Read", "obs1", None, None)
        .unwrap();
    mind.remember(ObservationType::Decision, "Write", "obs2", None, None)
        .unwrap();
    mind.remember(ObservationType::Bugfix, "Bash", "obs3", None, None)
        .unwrap();

    let stats = mind.stats().unwrap();
    assert_eq!(stats.total_observations, 3);
}

#[test]
fn mind_stats_counts_session_summaries() {
    let (_dir, mind) = test_mind();
    mind.remember(ObservationType::Discovery, "Read", "obs1", None, None)
        .unwrap();
    mind.save_session_summary(vec![], vec![], "Session 1")
        .unwrap();
    mind.save_session_summary(vec![], vec![], "Session 2")
        .unwrap();

    let stats = mind.stats().unwrap();
    assert_eq!(stats.total_sessions, 2);
    // 1 regular + 2 session summaries = 3 total observations.
    assert_eq!(stats.total_observations, 3);
}

#[test]
fn mind_stats_tracks_oldest_and_newest_memory() {
    let (_dir, mind) = test_mind();
    mind.remember(ObservationType::Discovery, "Read", "first", None, None)
        .unwrap();
    mind.remember(ObservationType::Decision, "Write", "second", None, None)
        .unwrap();

    let stats = mind.stats().unwrap();
    assert!(stats.oldest_memory.is_some(), "oldest should be set");
    assert!(stats.newest_memory.is_some(), "newest should be set");
    assert!(stats.oldest_memory.unwrap() <= stats.newest_memory.unwrap());
}

#[test]
fn mind_stats_reports_file_size() {
    let (_dir, mind) = test_mind();
    mind.remember(ObservationType::Discovery, "Read", "data", None, None)
        .unwrap();

    let stats = mind.stats().unwrap();
    assert!(stats.file_size_bytes > 0, "file_size should be positive");
}

#[test]
fn mind_stats_counts_observation_types() {
    let (_dir, mind) = test_mind();
    mind.remember(ObservationType::Discovery, "Read", "disc1", None, None)
        .unwrap();
    mind.remember(ObservationType::Discovery, "Read", "disc2", None, None)
        .unwrap();
    mind.remember(ObservationType::Decision, "Write", "dec1", None, None)
        .unwrap();
    mind.remember(ObservationType::Bugfix, "Bash", "bug1", None, None)
        .unwrap();

    let stats = mind.stats().unwrap();
    assert_eq!(stats.type_counts.get(&ObservationType::Discovery), Some(&2));
    assert_eq!(stats.type_counts.get(&ObservationType::Decision), Some(&1));
    assert_eq!(stats.type_counts.get(&ObservationType::Bugfix), Some(&1));
    assert!(!stats.type_counts.contains_key(&ObservationType::Pattern));
}

#[test]
fn mind_stats_empty_store() {
    let (_dir, mind) = test_mind();
    let stats = mind.stats().unwrap();
    assert_eq!(stats.total_observations, 0);
    assert_eq!(stats.total_sessions, 0);
    assert!(stats.oldest_memory.is_none());
    assert!(stats.newest_memory.is_none());
    assert!(stats.type_counts.is_empty());
}

// =========================================================================
// T047: Mind::stats (caching)
// =========================================================================

#[test]
fn mind_stats_caches_result() {
    let (_dir, mind) = test_mind();
    mind.remember(ObservationType::Discovery, "Read", "obs1", None, None)
        .unwrap();

    let stats1 = mind.stats().unwrap();
    let stats2 = mind.stats().unwrap();
    // Cached result should be identical.
    assert_eq!(stats1.total_observations, stats2.total_observations);
    assert_eq!(stats1.file_size_bytes, stats2.file_size_bytes);
    assert_eq!(stats1.type_counts, stats2.type_counts);
}

#[test]
fn mind_stats_recomputes_after_new_observation() {
    let (_dir, mind) = test_mind();
    mind.remember(ObservationType::Discovery, "Read", "obs1", None, None)
        .unwrap();

    let stats1 = mind.stats().unwrap();
    assert_eq!(stats1.total_observations, 1);

    mind.remember(ObservationType::Decision, "Write", "obs2", None, None)
        .unwrap();

    let stats2 = mind.stats().unwrap();
    assert_eq!(
        stats2.total_observations, 2,
        "should recompute after new observation"
    );
}
