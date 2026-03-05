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

fn load_baselines() -> Option<serde_json::Value> {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let fixtures = manifest_dir
        .parent()?
        .parent()?
        .join("tests")
        .join("fixtures")
        .join("ts_baselines.json");
    let content = std::fs::read_to_string(&fixtures).ok()?;
    serde_json::from_str(&content).ok()
}

fn get_baseline_value(baselines: &serde_json::Value, metric: &str) -> Option<f64> {
    baselines["baselines"]
        .as_array()?
        .iter()
        .find(|b| b["metric"].as_str() == Some(metric))
        .and_then(|b| b["value"].as_f64())
}

fn bench_get_context_vs_baseline(c: &mut Criterion) {
    let (_dir, mind) = setup_mind(100);

    let start = std::time::Instant::now();
    let iterations: u32 = 100;
    for _ in 0..iterations {
        mind.get_context(Some("architecture"))
            .expect("get_context failed");
    }
    let elapsed = start.elapsed();
    let avg_ms = elapsed.as_secs_f64() * 1000.0 / f64::from(iterations);

    // Compare against query_latency_ms as context-build is a read operation
    if let Some(baselines) = load_baselines() {
        if let Some(ts_ms) = get_baseline_value(&baselines, "query_latency_ms") {
            let speedup = ts_ms / avg_ms;
            println!();
            println!("=== TypeScript Baseline Comparison (Context) ===");
            println!("  Rust context build time:   {avg_ms:.3} ms");
            println!("  TypeScript query baseline: {ts_ms:.1} ms");
            println!("  Speedup factor:            {speedup:.1}x");
            println!("=================================================");
        }
    } else {
        println!("(ts_baselines.json not found, skipping comparison)");
    }

    c.bench_function("Mind::get_context vs baseline (100 observations)", |b| {
        b.iter(|| {
            mind.get_context(Some("architecture"))
                .expect("get_context failed");
        });
    });
}

criterion_group!(benches, bench_get_context, bench_get_context_vs_baseline);
criterion_main!(benches);
