// Integration tests for concurrent access with file locking.
//
// Uses real `MemvidStore` backend against temp `.mv2` files.

mod common {
    include!("../common/mod.rs");
}

use std::sync::{Arc, Barrier};
use std::thread;

use rusty_brain_core::mind::Mind;
use types::ObservationType;

/// Helper: spawn N writers that each write `writes_per_thread` observations,
/// synchronised by a barrier so all threads start at the same time.
fn run_concurrent_writers(num_threads: usize, writes_per_thread: usize) {
    let (_dir, config) = common::temp_mind_config();
    let mind = Arc::new(Mind::open(config).unwrap());
    let barrier = Arc::new(Barrier::new(num_threads));
    let mut handles = vec![];

    for i in 0..num_threads {
        let mind = Arc::clone(&mind);
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            barrier.wait(); // all threads start together
            for j in 0..writes_per_thread {
                let mut attempts = 0;
                loop {
                    match mind.remember(
                        ObservationType::Discovery,
                        "Read",
                        &format!("writer {i} observation {j}"),
                        Some(&format!("content from writer {i} obs {j}")),
                        None,
                    ) {
                        Ok(_) => break,
                        Err(e) if attempts < 15 => {
                            attempts += 1;
                            let delay =
                                std::time::Duration::from_millis(50 * (1 << attempts.min(6)));
                            std::thread::sleep(delay);
                        }
                        Err(e) => panic!("writer {i} obs {j} failed after {attempts} retries: {e}"),
                    }
                }
            }
        }));
    }

    for handle in handles {
        handle.join().unwrap();
    }

    let expected = (num_threads * writes_per_thread) as u64;
    let stats = mind.stats().unwrap();
    assert_eq!(
        stats.total_observations, expected,
        "all {expected} writes should succeed ({num_threads} writers × {writes_per_thread} each)"
    );
}

// =========================================================================
// T056: Concurrent access (SC-004)
// =========================================================================

/// Tests cross-process file locking via `with_lock`. Uses fewer writes
/// because each locked operation includes a full memvid `put + commit`
/// cycle and the exponential backoff has a finite retry budget.
#[test]
fn concurrent_writes_through_with_lock_no_data_loss() {
    let (_dir, config) = common::temp_mind_config();
    let mind = Arc::new(Mind::open(config).unwrap());

    let num_threads = 2;
    let writes_per_thread = 5;
    let mut handles = vec![];

    for i in 0..num_threads {
        let mind = Arc::clone(&mind);
        handles.push(thread::spawn(move || {
            for j in 0..writes_per_thread {
                let mut attempts = 0;
                loop {
                    match mind.with_lock(|m| {
                        m.remember(
                            ObservationType::Discovery,
                            "Read",
                            &format!("thread {i} observation {j}"),
                            None,
                            None,
                        )
                    }) {
                        Ok(_) => break,
                        Err(e) if attempts < 10 => {
                            attempts += 1;
                            let delay =
                                std::time::Duration::from_millis(50 * (1 << attempts.min(5)));
                            std::thread::sleep(delay);
                        }
                        Err(e) => panic!("with_lock write failed after {attempts} retries: {e}"),
                    }
                }
            }
        }));
    }

    for handle in handles {
        handle.join().unwrap();
    }

    let stats = mind.stats().unwrap();
    assert_eq!(
        stats.total_observations,
        (num_threads * writes_per_thread) as u64,
        "all writes should succeed without data loss"
    );
}

/// Tests data integrity across 100 concurrent writes per SC-004.
///
/// Uses `remember()` directly (Mind's internal Mutex provides in-process
/// thread safety). Lock contention is handled with per-write retries to
/// avoid exhausting the internal backoff budget.
#[test]
fn concurrent_100_writes_via_internal_mutex_no_data_loss() {
    let (_dir, config) = common::temp_mind_config();
    let mind = Arc::new(Mind::open(config).unwrap());

    let num_threads = 2;
    let writes_per_thread = 50; // 2 x 50 = 100 total per SC-004
    let mut handles = vec![];

    for i in 0..num_threads {
        let mind = Arc::clone(&mind);
        handles.push(thread::spawn(move || {
            for j in 0..writes_per_thread {
                // Retry on lock timeout — high contention is expected with
                // 100 concurrent writes through the file-lock path.
                let mut attempts = 0;
                loop {
                    match mind.remember(
                        ObservationType::Discovery,
                        "Read",
                        &format!("thread {i} observation {j}"),
                        None,
                        None,
                    ) {
                        Ok(_) => break,
                        Err(e) if attempts < 10 => {
                            attempts += 1;
                            let delay = std::time::Duration::from_millis(50 * (1 << attempts.min(5)));
                            std::thread::sleep(delay);
                            tracing::debug!(%e, attempt = attempts, "retrying write after lock timeout");
                        }
                        Err(e) => panic!("write failed after {attempts} retries: {e}"),
                    }
                }
            }
        }));
    }

    for handle in handles {
        handle.join().unwrap();
    }

    let stats = mind.stats().unwrap();
    assert_eq!(
        stats.total_observations,
        (num_threads * writes_per_thread) as u64,
        "all 100 writes should succeed without data loss (SC-004)"
    );
}

// =========================================================================
// T048: 4-writer concurrent test (SC-003, Contract 5)
// =========================================================================

#[test]
fn concurrent_4_writers_no_data_loss() {
    run_concurrent_writers(4, 3); // 4 × 3 = 12 observations
}

// =========================================================================
// T049: 8-writer concurrent test (SC-003, Contract 5)
// =========================================================================

#[test]
fn concurrent_8_writers_no_data_loss() {
    run_concurrent_writers(8, 2); // 8 × 2 = 16 observations
}

// =========================================================================
// T050: 16-writer stress test (SC-003, Contract 5)
// =========================================================================

#[test]
fn concurrent_16_writers_no_data_loss() {
    run_concurrent_writers(16, 1); // 16 × 1 = 16 observations
}

// =========================================================================
// T051: Stale lock recovery (Contract 5)
// =========================================================================

/// Create a stale lock file (simulating a crashed process), then open a
/// new Mind instance and verify it can acquire the lock within 5 seconds.
#[test]
fn stale_lock_recovery_within_5_seconds() {
    let (_dir, config) = common::temp_mind_config();

    // Open and close an initial Mind to create the .mv2 file.
    let lock_path = {
        let initial = Mind::open(config.clone()).unwrap();
        let mut p = initial.memory_path().as_os_str().to_os_string();
        p.push(".lock");
        std::path::PathBuf::from(p)
        // `initial` is dropped here, releasing any locks.
    };

    // Create a stale lock file (just the file, not actually locked).
    // This simulates a process that crashed after creating the lock file
    // but before releasing it. Since the OS releases flock on fd close,
    // the file exists but is not locked.
    std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .unwrap();

    // Reopen Mind after the stale lock file exists — this exercises the
    // startup/open path recovery.
    let start = std::time::Instant::now();
    let mind = Mind::open(config).unwrap();
    mind.remember(
        ObservationType::Discovery,
        "Read",
        "observation after stale lock recovery",
        Some("stale lock file should not block new operations"),
        None,
    )
    .unwrap();
    let elapsed = start.elapsed();

    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "stale lock recovery should complete within 5 seconds, took {:?}",
        elapsed
    );

    // Verify data integrity.
    let stats = mind.stats().unwrap();
    assert!(stats.total_observations >= 1);
}

/// Verify that a *held* lock (not stale) causes proper timeout behavior.
#[test]
fn held_lock_causes_timeout() {
    let (_dir, config) = common::temp_mind_config();
    let mind = Mind::open(config.clone()).unwrap();
    let lock_path = {
        let mut p = mind.memory_path().as_os_str().to_os_string();
        p.push(".lock");
        std::path::PathBuf::from(p)
    };

    // Actually hold the lock (simulating another process).
    let _lock_file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .unwrap();
    fs2::FileExt::try_lock_exclusive(&_lock_file).unwrap();

    // with_lock should timeout since we're holding the lock.
    let result = mind.with_lock(|m| {
        m.remember(
            ObservationType::Discovery,
            "Read",
            "should not succeed",
            None,
            None,
        )
    });

    assert!(result.is_err(), "should timeout when lock is held");
    let err_str = result.unwrap_err().to_string();
    assert!(
        err_str.contains("lock") || err_str.contains("timeout"),
        "error should mention lock/timeout, got: {err_str}"
    );
}

// =========================================================================
// T052: Reader-during-write test (Contract 5)
// =========================================================================

/// One writer holds a lock; concurrent readers should either wait for the
/// lock to release or get an error — never read partial/corrupt data.
#[test]
fn reader_during_write_no_partial_data() {
    let (_dir, config) = common::temp_mind_config();
    let mind = Arc::new(Mind::open(config).unwrap());

    // First, store some baseline data.
    mind.remember(
        ObservationType::Discovery,
        "Read",
        "baseline observation before concurrent reader writer test",
        Some("This observation exists before the concurrent read-write test begins"),
        None,
    )
    .unwrap();

    let barrier = Arc::new(Barrier::new(2));

    // Writer thread: writes several observations.
    let writer_mind = Arc::clone(&mind);
    let writer_barrier = Arc::clone(&barrier);
    let writer = thread::spawn(move || {
        writer_barrier.wait();
        for j in 0..5 {
            let mut attempts = 0;
            loop {
                match writer_mind.remember(
                    ObservationType::Decision,
                    "Write",
                    &format!("concurrent write observation {j}"),
                    Some(&format!("content for concurrent write {j}")),
                    None,
                ) {
                    Ok(_) => break,
                    Err(_) if attempts < 10 => {
                        attempts += 1;
                        std::thread::sleep(std::time::Duration::from_millis(
                            50 * (1 << attempts.min(5)),
                        ));
                    }
                    Err(e) => panic!("writer failed: {e}"),
                }
            }
        }
    });

    // Reader thread: searches while writer is active.
    let reader_mind = Arc::clone(&mind);
    let reader_barrier = Arc::clone(&barrier);
    let reader = thread::spawn(move || {
        reader_barrier.wait();
        // Perform multiple reads while writer is active.
        for _ in 0..5 {
            // search() should either return valid results or an error — never panic.
            match reader_mind.search("baseline observation concurrent reader writer", None) {
                Ok(results) => {
                    // If we get results, they should be valid (non-corrupt).
                    for r in &results {
                        assert!(
                            !r.summary.is_empty(),
                            "search result should have non-empty summary"
                        );
                    }
                }
                Err(_) => {
                    // Errors are acceptable (lock contention), but no panics.
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    });

    writer.join().unwrap();
    reader.join().unwrap();

    // After both finish, verify data integrity.
    let stats = mind.stats().unwrap();
    assert!(
        stats.total_observations >= 1,
        "at least baseline observation should exist"
    );
}

// =========================================================================
// T053: Data integrity verification with content matching (Contract 5)
// =========================================================================

/// After all concurrent writes complete, open Mind and verify that every
/// stored observation's content matches what was originally written.
#[test]
fn concurrent_writes_data_integrity_verified() {
    let (_dir, config) = common::temp_mind_config();
    let mind = Arc::new(Mind::open(config).unwrap());
    let barrier = Arc::new(Barrier::new(4));
    let mut handles = vec![];

    // 4 writers, each writes 3 unique observations with identifiable content.
    for i in 0..4 {
        let mind = Arc::clone(&mind);
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            barrier.wait();
            for j in 0..3 {
                let summary = format!("integrity writer {i} observation {j}");
                let content = format!("unique content from writer {i} observation {j}");
                let mut attempts = 0;
                loop {
                    match mind.remember(
                        ObservationType::Discovery,
                        "Read",
                        &summary,
                        Some(&content),
                        None,
                    ) {
                        Ok(_) => break,
                        Err(_) if attempts < 15 => {
                            attempts += 1;
                            std::thread::sleep(std::time::Duration::from_millis(
                                50 * (1 << attempts.min(6)),
                            ));
                        }
                        Err(e) => panic!("integrity write failed: {e}"),
                    }
                }
            }
        }));
    }

    for handle in handles {
        handle.join().unwrap();
    }

    // Verify all 12 observations are present and retrievable.
    let stats = mind.stats().unwrap();
    assert_eq!(
        stats.total_observations, 12,
        "all 12 observations should be stored"
    );

    // Verify all 12 observations via timeline (deterministic, not search-dependent).
    let timeline = mind.timeline(20, false).unwrap();
    assert_eq!(timeline.len(), 12, "timeline should have 12 entries");

    // Verify every writer's observations are present by checking summaries.
    let summaries: Vec<&str> = timeline.iter().map(|e| e.summary.as_str()).collect();
    for i in 0..4 {
        for j in 0..3 {
            let expected = format!("integrity writer {i} observation {j}");
            assert!(
                summaries.contains(&expected.as_str()),
                "missing observation: {expected}\n  found: {summaries:?}"
            );
        }
    }

    // Spot-check content round-trip via search for a subset of writers.
    // (Timeline entries don't carry content, so we use search to verify
    // the stored content payload matches what was originally written.)
    for i in 0..2 {
        let query = format!("unique content from writer {i}");
        let results = mind.search(&query, Some(10)).unwrap();
        let has_content_match = results.iter().any(|r| {
            r.content_excerpt
                .as_deref()
                .is_some_and(|c| c.contains(&format!("writer {i}")))
        });
        assert!(
            has_content_match,
            "content round-trip failed for writer {i}: no matching content_excerpt in results"
        );
    }
}
