//! Benchmark regression test (T059).
//!
//! Runs quick timing measurements and asserts each metric is at least 2x faster
//! than the TypeScript baselines defined in `tests/fixtures/ts_baselines.json`.
//! This is a regular `#[test]`, not a criterion benchmark.

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

/// Required speedup factor: Rust must be at least this many times faster.
const REQUIRED_SPEEDUP: f64 = 2.0;

#[test]
fn startup_time_at_least_2x_faster() {
    let Some(baselines) = load_baselines() else {
        eprintln!("ts_baselines.json not found, skipping regression test");
        return;
    };
    let Some(ts_ms) = get_baseline_value(&baselines, "startup_time_ms") else {
        eprintln!("startup_time_ms baseline not found, skipping");
        return;
    };
    let Some(binary) = find_binary() else {
        eprintln!("rusty-brain binary not found, skipping startup regression test");
        return;
    };

    // Warm up
    let _ = Command::new(&binary).arg("--version").output();

    let iterations: u32 = 20;
    let start = std::time::Instant::now();
    for _ in 0..iterations {
        Command::new(&binary)
            .arg("--version")
            .output()
            .expect("failed to run binary");
    }
    let elapsed = start.elapsed();
    let avg_ms = elapsed.as_secs_f64() * 1000.0 / f64::from(iterations);
    let speedup = ts_ms / avg_ms;

    eprintln!("Startup: Rust={avg_ms:.2}ms, TS={ts_ms:.1}ms, speedup={speedup:.1}x");

    assert!(
        speedup >= REQUIRED_SPEEDUP,
        "startup_time_ms regression: Rust ({avg_ms:.2}ms) is only {speedup:.1}x faster than \
         TypeScript ({ts_ms:.1}ms), required {REQUIRED_SPEEDUP}x"
    );
}

#[test]
fn query_latency_at_least_2x_faster() {
    let Some(baselines) = load_baselines() else {
        eprintln!("ts_baselines.json not found, skipping regression test");
        return;
    };
    let Some(ts_ms) = get_baseline_value(&baselines, "query_latency_ms") else {
        eprintln!("query_latency_ms baseline not found, skipping");
        return;
    };

    // Use Mind directly to measure query latency
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("bench-regression.mv2");
    let config = types::MindConfig {
        memory_path: path,
        ..types::MindConfig::default()
    };
    let mind = rusty_brain_core::mind::Mind::open(config).expect("open mind");

    // Preload 100 observations to match the baseline workload
    for i in 0..100 {
        mind.remember(
            types::ObservationType::Discovery,
            "bench",
            &format!("caching pattern {i}"),
            None,
            None,
        )
        .expect("remember");
    }

    // Warm up
    let _ = mind.search("caching pattern", Some(10));

    let iterations: u32 = 50;
    let start = std::time::Instant::now();
    for _ in 0..iterations {
        mind.search("caching pattern", Some(10)).expect("search");
    }
    let elapsed = start.elapsed();
    let avg_ms = elapsed.as_secs_f64() * 1000.0 / f64::from(iterations);
    let speedup = ts_ms / avg_ms;

    eprintln!("Query: Rust={avg_ms:.2}ms, TS={ts_ms:.1}ms, speedup={speedup:.1}x");

    assert!(
        speedup >= REQUIRED_SPEEDUP,
        "query_latency_ms regression: Rust ({avg_ms:.2}ms) is only {speedup:.1}x faster than \
         TypeScript ({ts_ms:.1}ms), required {REQUIRED_SPEEDUP}x"
    );
}

#[test]
fn compression_throughput_at_least_2x_faster() {
    let Some(baselines) = load_baselines() else {
        eprintln!("ts_baselines.json not found, skipping regression test");
        return;
    };
    let Some(ts_mb_s) = get_baseline_value(&baselines, "compression_throughput_mb_s") else {
        eprintln!("compression_throughput_mb_s baseline not found, skipping");
        return;
    };

    let config = compression::CompressionConfig::default();
    // 10KB input to match the baseline workload
    let input =
        "error: build failed at step N\nwarning: deprecated usage\nok: test passed\n".repeat(300);
    let input_len = input.len();

    // Warm up
    let _ = compression::compress(&config, "Bash", &input, Some("test"));

    let iterations: u32 = 500;
    let start = std::time::Instant::now();
    for _ in 0..iterations {
        let _ = compression::compress(&config, "Bash", &input, Some("test"));
    }
    let elapsed = start.elapsed();
    let total_bytes = input_len * usize::try_from(iterations).expect("fits in usize");
    #[expect(clippy::cast_precision_loss, reason = "byte count fits in f64")]
    let throughput_mb_s = (total_bytes as f64 / (1024.0 * 1024.0)) / elapsed.as_secs_f64();
    let speedup = throughput_mb_s / ts_mb_s;

    eprintln!(
        "Compression: Rust={throughput_mb_s:.2}MB/s, TS={ts_mb_s:.1}MB/s, speedup={speedup:.1}x"
    );

    assert!(
        speedup >= REQUIRED_SPEEDUP,
        "compression_throughput_mb_s regression: Rust ({throughput_mb_s:.2}MB/s) is only \
         {speedup:.1}x faster than TypeScript ({ts_mb_s:.1}MB/s), required {REQUIRED_SPEEDUP}x"
    );
}
