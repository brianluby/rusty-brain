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

fn bench_remember_vs_baseline(c: &mut Criterion) {
    let (_dir, mind) = setup_mind(100);

    let payload = "payload content for benchmark throughput measurement";
    let payload_bytes = payload.len();
    let start = std::time::Instant::now();
    let iterations: u32 = 100;
    for i in 0..iterations {
        mind.remember(
            ObservationType::Discovery,
            "bench",
            &format!("throughput observation {i}"),
            Some(payload),
            None,
        )
        .expect("remember failed");
    }
    let elapsed = start.elapsed();
    let total_bytes = payload_bytes * usize::try_from(iterations).expect("fits in usize");
    #[expect(clippy::cast_precision_loss, reason = "byte count fits in f64")]
    let throughput_mb_s = (total_bytes as f64 / (1024.0 * 1024.0)) / elapsed.as_secs_f64();
    let avg_ms = elapsed.as_secs_f64() * 1000.0 / f64::from(iterations);

    // Compare against query_latency_ms as a rough write-vs-read reference
    if let Some(baselines) = load_baselines() {
        if let Some(ts_ms) = get_baseline_value(&baselines, "query_latency_ms") {
            let speedup = ts_ms / avg_ms;
            println!();
            println!("=== TypeScript Baseline Comparison (Store) ===");
            println!("  Rust write latency:       {avg_ms:.3} ms");
            println!("  Rust write throughput:     {throughput_mb_s:.3} MB/s");
            println!("  TypeScript query baseline: {ts_ms:.1} ms");
            println!("  Speedup factor (vs read):  {speedup:.1}x");
            println!("===============================================");
        }
    } else {
        println!("(ts_baselines.json not found, skipping comparison)");
    }

    let mut j = 1000u64;
    c.bench_function("Mind::remember throughput (100 existing)", |b| {
        b.iter(|| {
            j += 1;
            mind.remember(
                ObservationType::Discovery,
                "bench",
                &format!("throughput obs {j}"),
                Some(payload),
                None,
            )
            .expect("remember failed");
        });
    });
}

criterion_group!(benches, bench_remember, bench_remember_vs_baseline);
criterion_main!(benches);
