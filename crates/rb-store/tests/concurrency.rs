//! Concurrency gate: single writer + N concurrent readers on one WAL file.
//!
//! Mirrors the daemon's storage-layer access pattern (one write connection,
//! many read connections, same DB file, WAL mode). Asserts no SQLITE_BUSY
//! surfaces and that every committed write is eventually readable.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use rb_store::{SqliteStore, Store};
use rb_types::{MemoryNote, MemoryType, Namespace};

const DIM: usize = 4;
const READERS: usize = 8;
const WRITES: usize = 200;

/// True if a storage error looks like a SQLite busy/locked contention error.
/// WAL + a single writer must make these impossible; any occurrence fails the test.
fn is_busy(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    m.contains("busy") || m.contains("database is locked") || m.contains("locked")
}

#[test]
fn single_writer_many_readers_no_busy_no_lost_writes() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("concurrency.db");

    // Writer connection (created first so the file + schema + WAL exist).
    let writer = SqliteStore::open(&db_path, DIM).unwrap();

    let ns = Namespace::Project("conc".to_string());

    // Shared signals.
    let stop = Arc::new(AtomicBool::new(false));
    let busy_seen = Arc::new(AtomicUsize::new(0));

    // Spawn N reader threads, each with its OWN SqliteStore on the same file.
    let mut reader_handles = Vec::with_capacity(READERS);
    for _ in 0..READERS {
        let path = db_path.clone();
        let ns = ns.clone();
        let stop = Arc::clone(&stop);
        let busy_seen = Arc::clone(&busy_seen);
        let handle = thread::spawn(move || {
            // Reader opens its own connection; WAL allows concurrent reads.
            let reader = SqliteStore::open(&path, DIM).unwrap();
            let probe: [f32; DIM] = [0.25, 0.25, 0.25, 0.25];
            while !stop.load(Ordering::Relaxed) {
                if let Err(e) = reader.list(&ns, None, 50) {
                    if is_busy(&e.to_string()) {
                        busy_seen.fetch_add(1, Ordering::Relaxed);
                    } else {
                        panic!("unexpected reader error in list: {e}");
                    }
                }
                if let Err(e) = reader.keyword_search(&ns, "memory", 50) {
                    if is_busy(&e.to_string()) {
                        busy_seen.fetch_add(1, Ordering::Relaxed);
                    } else {
                        panic!("unexpected reader error in keyword_search: {e}");
                    }
                }
                if let Err(e) = reader.vector_search(&ns, &probe, 5) {
                    if is_busy(&e.to_string()) {
                        busy_seen.fetch_add(1, Ordering::Relaxed);
                    } else {
                        panic!("unexpected reader error in vector_search: {e}");
                    }
                }
            }
        });
        reader_handles.push(handle);
    }

    // Writer thread: serialized inserts through the single write connection.
    let write_busy = Arc::clone(&busy_seen);
    let write_ns = ns.clone();
    let writer_handle = thread::spawn(move || {
        for i in 0..WRITES {
            let content = format!("memory note number {i} about concurrent access");
            let mut note = MemoryNote::new(write_ns.clone(), content, MemoryType::Insight, 5);
            note.summary = format!("note {i}");
            note.keywords = vec!["memory".to_string(), "concurrent".to_string()];
            let emb: [f32; DIM] = [i as f32, 0.0, 0.0, 1.0];
            if let Err(e) = writer.insert_memory(&note, Some(&emb)) {
                if is_busy(&e.to_string()) {
                    write_busy.fetch_add(1, Ordering::Relaxed);
                } else {
                    panic!("unexpected writer error: {e}");
                }
            }
        }
    });

    // Wait for the writer to finish, then stop readers.
    writer_handle.join().unwrap();
    stop.store(true, Ordering::Relaxed);
    for h in reader_handles {
        h.join().unwrap();
    }

    // Assertion 1: no busy/locked errors anywhere.
    assert_eq!(
        busy_seen.load(Ordering::Relaxed),
        0,
        "WAL + single writer must yield zero SQLITE_BUSY/locked errors"
    );

    // Assertion 2: no lost writes. A fresh reader sees all WRITES rows.
    // Poll briefly to allow WAL visibility to settle across connections.
    let verifier = SqliteStore::open(&db_path, DIM).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let count = loop {
        let n = verifier.list(&ns, None, WRITES + 10).unwrap().len();
        if n >= WRITES || Instant::now() >= deadline {
            break n;
        }
        thread::sleep(Duration::from_millis(25));
    };
    assert_eq!(
        count, WRITES,
        "all {WRITES} writes must be readable (no lost writes)"
    );
}
