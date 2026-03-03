//! Criterion benchmark for `Mind::remember` (store) latency.
//!
//! Target: <500ms p95 at 10K observations (SC-008).
//! Note: Preloads 100 observations for CI feasibility. Manual 10K validation
//! can be run by changing `setup_mind(100)` to `setup_mind(10_000)`.

use criterion::{Criterion, criterion_group, criterion_main};
use rusty_brain_core::mind::Mind;
use types::{MindConfig, ObservationType};

fn setup_mind(n: usize) -> (tempfile::TempDir, Mind) {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let path = dir.path().join("bench-store.mv2");
    let config = MindConfig {
        memory_path: path,
        ..MindConfig::default()
    };
    let mind = Mind::open(config).expect("failed to open mind");
    for i in 0..n {
        mind.remember(
            ObservationType::Discovery,
            "bench",
            &format!("preloaded observation {i}"),
            Some("benchmark content payload for measuring store performance"),
            None,
        )
        .expect("failed to preload observation");
    }
    (dir, mind)
}

fn bench_remember(c: &mut Criterion) {
    let (_dir, mind) = setup_mind(100);

    let mut i = 0u64;
    c.bench_function("Mind::remember (100 existing)", |b| {
        b.iter(|| {
            i += 1;
            mind.remember(
                ObservationType::Discovery,
                "bench",
                &format!("bench observation {i}"),
                Some("payload content for benchmark"),
                None,
            )
            .expect("remember failed");
        });
    });
}

criterion_group!(benches, bench_remember);
criterion_main!(benches);
