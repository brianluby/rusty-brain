//! Tests verifying success criteria SC-002 and SC-003.

use compression::{CompressionConfig, compress};

// --- T051: SC-002 — Compression ratio >= 10x on 20K+ char inputs ---

/// Generate a realistic JavaScript source file with many lines.
fn large_js_source(target_chars: usize) -> String {
    let mut s = String::with_capacity(target_chars + 200);
    s.push_str("import React from 'react';\n");
    s.push_str("import { useState, useEffect } from 'react';\n");
    s.push_str("import axios from 'axios';\n\n");
    let mut i = 0;
    while s.chars().count() < target_chars {
        s.push_str(&format!(
            "function handler{i}(request, response) {{\n  const data = request.body;\n  \
             console.log('Processing request', data);\n  response.json({{ ok: true }});\n}}\n\n"
        ));
        i += 1;
    }
    s
}

/// Generate a realistic bash build log with scattered errors.
fn large_bash_log(target_chars: usize) -> String {
    let mut s = String::with_capacity(target_chars + 200);
    s.push_str("$ npm run build\n");
    let mut i = 0;
    while s.chars().count() < target_chars {
        if i % 200 == 50 {
            s.push_str(&format!("error: compilation failed at module {i}\n"));
        } else if i % 200 == 100 {
            s.push_str(&format!("warning: unused variable in module {i}\n"));
        } else {
            s.push_str(&format!(
                "[{i}/5000] Compiling module_{i}.rs ... ok (0.12s)\n"
            ));
        }
        i += 1;
    }
    s.push_str("Build completed with errors.\n");
    s
}

/// Generate realistic grep output with many matches across files.
fn large_grep_output(target_chars: usize) -> String {
    let mut s = String::with_capacity(target_chars + 200);
    let mut i = 0;
    while s.chars().count() < target_chars {
        let file_num = i % 50;
        s.push_str(&format!(
            "src/modules/mod{file_num}/handler.rs:{i}: let result = process_data(input);\n"
        ));
        i += 1;
    }
    s
}

/// Generate realistic glob output with many file paths.
fn large_glob_output(target_chars: usize) -> String {
    let mut s = String::with_capacity(target_chars + 200);
    let mut i = 0;
    while s.chars().count() < target_chars {
        let dir_num = i % 30;
        s.push_str(&format!(
            "src/components/feature{dir_num}/component_{i}.tsx\n"
        ));
        i += 1;
    }
    s
}

/// Generate realistic edit output with a large diff.
fn large_edit_output(target_chars: usize) -> String {
    let mut s = String::with_capacity(target_chars + 200);
    s.push_str("File: src/main.rs\n");
    s.push_str("--- a/src/main.rs\n+++ b/src/main.rs\n");
    let mut i = 0;
    while s.chars().count() < target_chars {
        s.push_str(&format!(
            "-    let old_value_{i} = compute_old({i});\n\
             +    let new_value_{i} = compute_new({i});\n"
        ));
        i += 1;
    }
    s
}

/// Generate large generic text.
fn large_generic_text(target_chars: usize) -> String {
    let mut s = String::with_capacity(target_chars + 200);
    let mut i = 0;
    while s.chars().count() < target_chars {
        s.push_str(&format!(
            "Line {i}: Lorem ipsum dolor sit amet, consectetur adipiscing elit.\n"
        ));
        i += 1;
    }
    s
}

#[test]
fn sc002_read_compression_ratio_10x_on_20k_plus() {
    let config = CompressionConfig::default();
    let input = large_js_source(25_000);
    assert!(input.chars().count() >= 20_000);

    let result = compress(&config, "Read", &input, Some("app.js"));
    assert!(result.compression_applied);
    let stats = result.statistics.as_ref().expect("stats should be present");
    assert!(
        stats.ratio >= 10.0,
        "SC-002: Read compression ratio {:.1}x < 10x on {}+ char input",
        stats.ratio,
        input.chars().count()
    );
}

#[test]
fn sc002_bash_compression_ratio_10x_on_20k_plus() {
    let config = CompressionConfig::default();
    let input = large_bash_log(25_000);
    assert!(input.chars().count() >= 20_000);

    let result = compress(&config, "Bash", &input, Some("npm run build"));
    assert!(result.compression_applied);
    let stats = result.statistics.as_ref().expect("stats should be present");
    assert!(
        stats.ratio >= 10.0,
        "SC-002: Bash compression ratio {:.1}x < 10x on {}+ char input",
        stats.ratio,
        input.chars().count()
    );
}

#[test]
fn sc002_grep_compression_ratio_10x_on_20k_plus() {
    let config = CompressionConfig::default();
    let input = large_grep_output(25_000);
    assert!(input.chars().count() >= 20_000);

    let result = compress(&config, "Grep", &input, Some("process_data"));
    assert!(result.compression_applied);
    let stats = result.statistics.as_ref().expect("stats should be present");
    assert!(
        stats.ratio >= 10.0,
        "SC-002: Grep compression ratio {:.1}x < 10x on {}+ char input",
        stats.ratio,
        input.chars().count()
    );
}

#[test]
fn sc002_glob_compression_ratio_10x_on_20k_plus() {
    let config = CompressionConfig::default();
    let input = large_glob_output(25_000);
    assert!(input.chars().count() >= 20_000);

    let result = compress(&config, "Glob", &input, Some("**/*.tsx"));
    assert!(result.compression_applied);
    let stats = result.statistics.as_ref().expect("stats should be present");
    assert!(
        stats.ratio >= 10.0,
        "SC-002: Glob compression ratio {:.1}x < 10x on {}+ char input",
        stats.ratio,
        input.chars().count()
    );
}

#[test]
fn sc002_edit_compression_ratio_10x_on_20k_plus() {
    let config = CompressionConfig::default();
    let input = large_edit_output(25_000);
    assert!(input.chars().count() >= 20_000);

    let result = compress(&config, "Edit", &input, Some("src/main.rs"));
    assert!(result.compression_applied);
    let stats = result.statistics.as_ref().expect("stats should be present");
    assert!(
        stats.ratio >= 10.0,
        "SC-002: Edit compression ratio {:.1}x < 10x on {}+ char input",
        stats.ratio,
        input.chars().count()
    );
}

#[test]
fn sc002_generic_compression_ratio_10x_on_20k_plus() {
    let config = CompressionConfig::default();
    let input = large_generic_text(25_000);
    assert!(input.chars().count() >= 20_000);

    let result = compress(&config, "WebFetch", &input, None);
    assert!(result.compression_applied);
    let stats = result.statistics.as_ref().expect("stats should be present");
    assert!(
        stats.ratio >= 10.0,
        "SC-002: Generic compression ratio {:.1}x < 10x on {}+ char input",
        stats.ratio,
        input.chars().count()
    );
}

// --- T052: SC-003 — File-read preserves >= 80% of constructs ---

/// Build a JS source with known, countable constructs.
/// Returns (source_code, list_of_construct_signatures).
fn js_source_with_known_constructs() -> (String, Vec<&'static str>) {
    let constructs = vec![
        "import React from 'react';",
        "import { useState } from 'react';",
        "import { useEffect } from 'react';",
        "import axios from 'axios';",
        "import { Router } from 'express';",
        "export default function App() {",
        "export const API_URL = 'https://api.example.com';",
        "export { helper };",
        "function processData(input) {",
        "function validateInput(data) {",
        "function formatOutput(result) {",
        "async function fetchUsers() {",
        "class UserService {",
        "class DataProcessor {",
        "interface UserProps {",
        "// TODO: add error handling",
        "// FIXME: race condition here",
    ];

    // Build a large file with the constructs spread among filler
    let mut source = String::new();
    for (i, construct) in constructs.iter().enumerate() {
        source.push_str(construct);
        source.push('\n');
        // Add filler lines between constructs to make the file large
        for j in 0..60 {
            source.push_str(&format!("  const temp_{i}_{j} = computeValue({i}, {j});\n"));
        }
    }

    // Pad to ensure we exceed the threshold
    while source.chars().count() < 5_000 {
        source.push_str("  // filler line to reach threshold\n");
    }

    (source, constructs)
}

#[test]
fn sc003_read_preserves_80_percent_constructs_js() {
    let config = CompressionConfig::default();
    let (source, constructs) = js_source_with_known_constructs();
    let total_constructs = constructs.len();

    assert!(
        source.chars().count() > config.compression_threshold,
        "Test source must exceed threshold"
    );

    let result = compress(&config, "Read", &source, Some("app.js"));
    assert!(result.compression_applied);

    // Count how many known constructs appear in the compressed output.
    // We check for a distinctive substring of each construct.
    let preserved = constructs
        .iter()
        .filter(|c| {
            // Extract a key phrase from the construct to search for
            let key = if c.starts_with("import") || c.starts_with("from") {
                // For imports, check the module name or imported name
                *c
            } else if c.starts_with("export") {
                *c
            } else if c.starts_with("function")
                || c.starts_with("async function")
                || c.starts_with("class")
                || c.starts_with("interface")
            {
                *c
            } else {
                // Error markers — check for the keyword
                *c
            };
            // Use contains with trimmed version — the compressed output
            // may have slightly different formatting
            let trimmed = key.trim();
            result.text.contains(trimmed)
        })
        .count();

    let preservation_rate = preserved as f64 / total_constructs as f64 * 100.0;
    assert!(
        preservation_rate >= 80.0,
        "SC-003: Only {preserved}/{total_constructs} ({preservation_rate:.1}%) constructs preserved, need >= 80%"
    );
}

/// Same test for Python source.
#[test]
fn sc003_read_preserves_80_percent_constructs_python() {
    let config = CompressionConfig::default();

    let constructs: Vec<&str> = vec![
        "import os",
        "import sys",
        "from pathlib import Path",
        "from typing import List",
        "from collections import defaultdict",
        "def process_data(input_data):",
        "def validate_input(data):",
        "def format_output(result):",
        "async def fetch_users():",
        "class UserService:",
        "class DataProcessor:",
        "# TODO: add error handling",
        "# FIXME: race condition",
    ];

    let mut source = String::new();
    for (i, construct) in constructs.iter().enumerate() {
        source.push_str(construct);
        source.push('\n');
        for j in 0..60 {
            source.push_str(&format!("    temp_{i}_{j} = compute_value({i}, {j})\n"));
        }
    }
    while source.chars().count() < 5_000 {
        source.push_str("    # filler\n");
    }

    let total = constructs.len();
    let result = compress(&config, "Read", &source, Some("app.py"));
    assert!(result.compression_applied);

    let preserved = constructs
        .iter()
        .filter(|c| result.text.contains(c.trim()))
        .count();
    let rate = preserved as f64 / total as f64 * 100.0;
    assert!(
        rate >= 80.0,
        "SC-003 (Python): Only {preserved}/{total} ({rate:.1}%) constructs preserved, need >= 80%"
    );
}

// --- T053: SC-006 — < 5ms per 10K-char input ---

#[test]
fn sc006_latency_under_5ms_for_10k_input() {
    let config = CompressionConfig::default();
    let size = 10_000;
    let margin_ms = 5;

    let inputs: Vec<(&str, String, Option<&str>)> = vec![
        ("Read", large_js_source(size), Some("app.js")),
        ("Bash", large_bash_log(size), Some("npm run build")),
        ("Grep", large_grep_output(size), Some("pattern")),
        ("Glob", large_glob_output(size), Some("**/*.rs")),
        ("Edit", large_edit_output(size), Some("src/main.rs")),
        ("WebFetch", large_generic_text(size), None),
    ];

    for (tool, input, ctx) in &inputs {
        // Warm up (first call may be slower due to lazy regex init)
        let _ = compress(&config, tool, input, *ctx);

        let start = std::time::Instant::now();
        let _result = compress(&config, tool, input, *ctx);
        let elapsed = start.elapsed();

        assert!(
            elapsed.as_millis() < margin_ms,
            "SC-006: {tool} took {}ms (limit: {margin_ms}ms) for {size}-char input",
            elapsed.as_millis()
        );
    }
}

/// Same test for Rust source.
#[test]
fn sc003_read_preserves_80_percent_constructs_rust() {
    let config = CompressionConfig::default();

    let constructs: Vec<&str> = vec![
        "use std::io;",
        "use std::collections::HashMap;",
        "use std::sync::Arc;",
        "mod utils;",
        "pub fn process_data(input: &str) -> Result<String, Error> {",
        "pub fn validate_input(data: &Data) -> bool {",
        "fn format_output(result: &Result) -> String {",
        "async fn fetch_users() -> Vec<User> {",
        "struct Config {",
        "enum State {",
        "trait Handler {",
        "impl Config {",
        "// TODO: add error handling",
        "// FIXME: race condition",
    ];

    let mut source = String::new();
    for (i, construct) in constructs.iter().enumerate() {
        source.push_str(construct);
        source.push('\n');
        for j in 0..60 {
            source.push_str(&format!(
                "    let temp_{i}_{j} = compute_value({i}, {j});\n"
            ));
        }
    }
    while source.chars().count() < 5_000 {
        source.push_str("    // filler\n");
    }

    let total = constructs.len();
    let result = compress(&config, "Read", &source, Some("lib.rs"));
    assert!(result.compression_applied);

    let preserved = constructs
        .iter()
        .filter(|c| result.text.contains(c.trim()))
        .count();
    let rate = preserved as f64 / total as f64 * 100.0;
    assert!(
        rate >= 80.0,
        "SC-003 (Rust): Only {preserved}/{total} ({rate:.1}%) constructs preserved, need >= 80%"
    );
}
