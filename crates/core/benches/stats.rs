//! Criterion benchmark for `Mind::stats` latency.
//!
//! Target: <2s p95 at 10K observations (SC-005).
//! Note: Preloads 100 observations for CI feasibility. Manual 10K validation
//! can be run by changing `setup_mind(100)` to `setup_mind(10_000)`.

use criterion::{Criterion, criterion_group, criterion_main};
use rusty_brain_core::mind::Mind;
use types::{MindConfig, ObservationType};

fn setup_mind(n: usize) -> (tempfile::TempDir, Mind) {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let path = dir.path().join("bench-stats.mv2");
    let config = MindConfig {
        memory_path: path,
        ..MindConfig::default()
    };
    let mind = Mind::open(config).expect("failed to open mind");
    let types = [
        ObservationType::Discovery,
        ObservationType::Decision,
        ObservationType::Bugfix,
        ObservationType::Success,
    ];
    for i in 0..n {
        mind.remember(
            types[i % types.len()],
            "bench",
            &format!("observation number {i}"),
            None,
            None,
        )
        .expect("failed to preload observation");
    }
    (dir, mind)
}

fn bench_stats(c: &mut Criterion) {
    let (_dir, mind) = setup_mind(100);

    c.bench_function("Mind::remember+stats (100 base, cache-invalidated)", |b| {
        b.iter(|| {
            // Invalidate cache by adding an observation before each stats call.
            mind.remember(ObservationType::Discovery, "bench", "extra", None, None)
                .expect("remember failed");
            mind.stats().expect("stats failed");
        });
    });
}

criterion_group!(benches, bench_stats);
criterion_main!(benches);
