//! Criterion benchmark for `Mind::search` latency.
//!
//! Target: <500ms p95 at 10K observations (SC-009).
//! Note: Preloads 100 observations for CI feasibility. Manual 10K validation
//! can be run by changing `setup_mind(100)` to `setup_mind(10_000)`.

use criterion::{Criterion, criterion_group, criterion_main};
use rusty_brain_core::mind::Mind;
use types::{MindConfig, ObservationType};

fn setup_mind(n: usize) -> (tempfile::TempDir, Mind) {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let path = dir.path().join("bench-search.mv2");
    let config = MindConfig {
        memory_path: path,
        ..MindConfig::default()
    };
    let mind = Mind::open(config).expect("failed to open mind");
    for i in 0..n {
        mind.remember(
            ObservationType::Discovery,
            "bench",
            &format!("caching pattern {i}"),
            None,
            None,
        )
        .expect("failed to preload observation");
    }
    (dir, mind)
}

fn bench_search(c: &mut Criterion) {
    let (_dir, mind) = setup_mind(100);

    c.bench_function("Mind::search (100 observations)", |b| {
        b.iter(|| {
            mind.search("caching pattern", Some(10))
                .expect("search failed");
        });
    });
}

criterion_group!(benches, bench_search);
criterion_main!(benches);
