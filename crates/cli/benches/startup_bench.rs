//! Criterion benchmark for CLI cold-start time.
//!
//! Measures time from process spawn to completion using `--version`.
//! Compares against TypeScript startup_time_ms baseline.

use criterion::{Criterion, criterion_group, criterion_main};
use std::process::Command;

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

fn find_binary() -> Option<std::path::PathBuf> {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.parent()?.parent()?;

    // Try release first, then debug
    for profile in &["release", "debug"] {
        let bin = workspace_root
            .join("target")
            .join(profile)
            .join("rusty-brain");
        if bin.exists() {
            return Some(bin);
        }
    }
    None
}

fn bench_startup(c: &mut Criterion) {
    let Some(binary) = find_binary() else {
        println!("(rusty-brain binary not found, skipping startup benchmark)");
        return;
    };

    // Pre-measure for baseline comparison
    let iterations: u32 = 50;
    let start = std::time::Instant::now();
    for _ in 0..iterations {
        let _ = Command::new(&binary).arg("--version").output();
    }
    let elapsed = start.elapsed();
    let avg_ms = elapsed.as_secs_f64() * 1000.0 / f64::from(iterations);

    if let Some(baselines) = load_baselines() {
        if let Some(ts_ms) = get_baseline_value(&baselines, "startup_time_ms") {
            let speedup = ts_ms / avg_ms;
            println!();
            println!("=== TypeScript Baseline Comparison (Startup) ===");
            println!("  Rust startup time:    {avg_ms:.2} ms");
            println!("  TypeScript baseline:  {ts_ms:.1} ms");
            println!("  Speedup factor:       {speedup:.1}x");
            println!("=================================================");
        }
    } else {
        println!("(ts_baselines.json not found, skipping comparison)");
    }

    c.bench_function("CLI cold start (--version)", |b| {
        b.iter(|| {
            Command::new(&binary)
                .arg("--version")
                .output()
                .expect("failed to run binary");
        });
    });
}

criterion_group!(benches, bench_startup);
criterion_main!(benches);
