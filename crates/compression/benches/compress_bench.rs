//! Criterion benchmarks for tool-output compression (SC-006: < 5ms per 10K-char input).

use criterion::{Criterion, black_box, criterion_group, criterion_main};

use compression::{CompressionConfig, compress};

fn make_js_content(chars: usize) -> String {
    let mut content = String::from("import React from 'react';\n");
    content.push_str("import { useState, useEffect } from 'react';\n");
    content.push_str("export default function App() {\n");
    let mut char_count = content.chars().count();
    let mut i = 0;
    while char_count < chars {
        let line = format!("  const value{i} = computeValue({i});\n");
        char_count += line.chars().count();
        content.push_str(&line);
        i += 1;
    }
    truncate_to_char_boundary(&mut content, chars);
    content
}

fn make_bash_content(chars: usize) -> String {
    let mut content = String::new();
    let mut char_count = 0;
    let mut i = 0;
    while char_count < chars {
        let line = match i % 3 {
            0 => format!("error: build failed at step {i}\n"),
            1 => format!("warning: deprecated usage in line {i}\n"),
            _ => format!("ok: test {i} passed\n"),
        };
        char_count += line.chars().count();
        content.push_str(&line);
        i += 1;
    }
    truncate_to_char_boundary(&mut content, chars);
    content
}

fn make_grep_content(chars: usize) -> String {
    let mut content = String::new();
    let mut char_count = 0;
    let mut i = 0;
    while char_count < chars {
        let line = format!(
            "src/module{}/handler.rs:{}:    let result = process(input);\n",
            i % 20,
            i * 10
        );
        char_count += line.chars().count();
        content.push_str(&line);
        i += 1;
    }
    truncate_to_char_boundary(&mut content, chars);
    content
}

fn make_glob_content(chars: usize) -> String {
    let mut content = String::new();
    let mut char_count = 0;
    let mut i = 0;
    while char_count < chars {
        let line = format!("src/module{}/file{}.rs\n", i % 30, i);
        char_count += line.chars().count();
        content.push_str(&line);
        i += 1;
    }
    truncate_to_char_boundary(&mut content, chars);
    content
}

fn make_edit_content(chars: usize) -> String {
    let mut content = String::new();
    let mut char_count = 0;
    let mut i = 0;
    while char_count < chars {
        let line = format!("-old line {i}\n+new line {i}\n");
        char_count += line.chars().count();
        content.push_str(&line);
        i += 1;
    }
    truncate_to_char_boundary(&mut content, chars);
    content
}

fn make_generic_content(chars: usize) -> String {
    let mut content = String::new();
    let mut char_count = 0;
    let mut i = 0;
    while char_count < chars {
        let line = format!("output line {i}: some generic tool data\n");
        char_count += line.chars().count();
        content.push_str(&line);
        i += 1;
    }
    truncate_to_char_boundary(&mut content, chars);
    content
}

/// Truncate a string to at most `max_chars` characters on a char boundary.
fn truncate_to_char_boundary(s: &mut String, max_chars: usize) {
    if let Some((idx, _)) = s.char_indices().nth(max_chars) {
        s.truncate(idx);
    }
}

fn bench_compress(c: &mut Criterion) {
    let config = CompressionConfig::default();
    let size = 10_000;

    let js_content = make_js_content(size);
    let bash_content = make_bash_content(size);
    let grep_content = make_grep_content(size);
    let glob_content = make_glob_content(size);
    let edit_content = make_edit_content(size);
    let generic_content = make_generic_content(size);

    c.bench_function("compress/read_js_10k", |b| {
        b.iter(|| {
            compress(
                black_box(&config),
                "Read",
                black_box(&js_content),
                Some("app.js"),
            )
        });
    });

    c.bench_function("compress/bash_10k", |b| {
        b.iter(|| {
            compress(
                black_box(&config),
                "Bash",
                black_box(&bash_content),
                Some("npm test"),
            )
        });
    });

    c.bench_function("compress/grep_10k", |b| {
        b.iter(|| {
            compress(
                black_box(&config),
                "Grep",
                black_box(&grep_content),
                Some("pattern"),
            )
        });
    });

    c.bench_function("compress/glob_10k", |b| {
        b.iter(|| {
            compress(
                black_box(&config),
                "Glob",
                black_box(&glob_content),
                Some("**/*.rs"),
            )
        });
    });

    c.bench_function("compress/edit_10k", |b| {
        b.iter(|| {
            compress(
                black_box(&config),
                "Edit",
                black_box(&edit_content),
                Some("src/main.rs"),
            )
        });
    });

    c.bench_function("compress/generic_10k", |b| {
        b.iter(|| {
            compress(
                black_box(&config),
                "CustomTool",
                black_box(&generic_content),
                None,
            )
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

fn bench_compress_vs_baseline(c: &mut Criterion) {
    let config = CompressionConfig::default();
    // Use 10KB input to match the TypeScript baseline workload
    let input_bytes = 10 * 1024;
    let content = make_bash_content(input_bytes);
    let content_len = content.len();

    // Pre-measure throughput for comparison print
    let start = std::time::Instant::now();
    let iterations: u32 = 1000;
    for _ in 0..iterations {
        let _ = compress(
            black_box(&config),
            "Bash",
            black_box(&content),
            Some("npm test"),
        );
    }
    let elapsed = start.elapsed();
    let total_bytes = content_len * usize::try_from(iterations).expect("fits in usize");
    #[expect(clippy::cast_precision_loss, reason = "byte count fits in f64")]
    let throughput_mb_s = (total_bytes as f64 / (1024.0 * 1024.0)) / elapsed.as_secs_f64();

    if let Some(baselines) = load_baselines() {
        if let Some(ts_mb_s) = get_baseline_value(&baselines, "compression_throughput_mb_s") {
            let speedup = throughput_mb_s / ts_mb_s;
            println!();
            println!("=== TypeScript Baseline Comparison (Compression) ===");
            println!("  Rust throughput:       {throughput_mb_s:.2} MB/s");
            println!("  TypeScript baseline:   {ts_mb_s:.1} MB/s");
            println!("  Speedup factor:        {speedup:.1}x");
            println!("=====================================================");
        }
    } else {
        println!("(ts_baselines.json not found, skipping comparison)");
    }

    c.bench_function("compress/bash_10kb_throughput", |b| {
        b.iter(|| {
            compress(
                black_box(&config),
                "Bash",
                black_box(&content),
                Some("npm test"),
            )
        });
    });
}

criterion_group!(benches, bench_compress, bench_compress_vs_baseline);
criterion_main!(benches);
