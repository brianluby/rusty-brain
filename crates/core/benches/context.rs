//! Criterion benchmark for `Mind::get_context` latency.
//!
//! Target: <2s p95 at 10K observations (SC-010).
//! Note: Preloads 100 observations for CI feasibility. Manual 10K validation
//! can be run by changing `setup_mind(100)` to `setup_mind(10_000)`.

use criterion::{Criterion, criterion_group, criterion_main};
use rusty_brain_core::mind::Mind;
use types::{MindConfig, ObservationType};

fn setup_mind(n: usize) -> (tempfile::TempDir, Mind) {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let path = dir.path().join("bench-context.mv2");
    let config = MindConfig {
        memory_path: path,
        ..MindConfig::default()
    };
    let mind = Mind::open(config).expect("failed to open mind");
    for i in 0..n {
        mind.remember(
            ObservationType::Discovery,
            "bench",
            &format!("system architecture {i}"),
            None,
            None,
        )
        .expect("failed to preload observation");
    }
    // Add a session summary so context has all sections.
    mind.save_session_summary(
        vec!["chose microservices".to_string()],
        vec!["src/main.rs".to_string()],
        "Architecture review session",
    )
    .expect("failed to save session summary");
    (dir, mind)
}

fn bench_get_context(c: &mut Criterion) {
    let (_dir, mind) = setup_mind(100);

    c.bench_function("Mind::get_context (100 observations)", |b| {
        b.iter(|| {
            mind.get_context(Some("architecture"))
                .expect("get_context failed");
        });
    });
}

criterion_group!(benches, bench_get_context);
criterion_main!(benches);
