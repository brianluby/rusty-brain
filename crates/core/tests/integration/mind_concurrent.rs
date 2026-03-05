// Integration tests for concurrent access with file locking.
//
// Uses real `MemvidStore` backend against temp `.mv2` files.

mod common {
    include!("../common/mod.rs");
}

use std::sync::Arc;
use std::thread;

use rusty_brain_core::mind::Mind;
use types::ObservationType;

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
                mind.with_lock(|m| {
                    m.remember(
                        ObservationType::Discovery,
                        "Read",
                        &format!("thread {i} observation {j}"),
                        None,
                        None,
                    )
                })
                .unwrap();
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
