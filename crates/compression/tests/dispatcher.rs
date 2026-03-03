//! Dispatcher integration tests — exercises `compress()` through its public API.

use compression::{CompressionConfig, compress};

fn large_input(size: usize) -> String {
    "a".repeat(size)
}

// --- Basic dispatcher behavior ---

#[test]
fn empty_input_returns_unchanged() {
    let config = CompressionConfig::default();
    let result = compress(&config, "Read", "", None);
    assert!(!result.compression_applied);
    assert_eq!(result.text, "");
    assert_eq!(result.original_size, 0);
    assert!(result.statistics.is_none());
}

#[test]
fn whitespace_only_returns_unchanged() {
    let config = CompressionConfig::default();
    let result = compress(&config, "Read", "   \n\t  ", None);
    assert!(!result.compression_applied);
    assert_eq!(result.text, "   \n\t  ");
}

#[test]
fn below_threshold_returns_unchanged() {
    let config = CompressionConfig::default();
    let input = "a".repeat(2_999);
    let result = compress(&config, "Read", &input, None);
    assert!(!result.compression_applied);
    assert_eq!(result.text, input);
    assert_eq!(result.original_size, 2_999);
}

#[test]
fn at_threshold_returns_unchanged() {
    let config = CompressionConfig::default();
    let input = "a".repeat(3_000);
    let result = compress(&config, "Read", &input, None);
    assert!(!result.compression_applied);
}

#[test]
fn above_threshold_compresses() {
    let config = CompressionConfig::default();
    let input = large_input(5_000);
    let result = compress(&config, "Read", &input, None);
    assert!(result.compression_applied);
    assert!(result.text.chars().count() <= config.target_budget);
}

#[test]
fn unknown_tool_routes_to_generic() {
    let config = CompressionConfig::default();
    let input = large_input(5_000);
    let result = compress(&config, "CustomTool", &input, None);
    assert!(result.compression_applied);
    assert!(result.text.chars().count() <= config.target_budget);
}

#[test]
fn statistics_present_when_compressed() {
    let config = CompressionConfig::default();
    let input = large_input(5_000);
    let result = compress(&config, "Read", &input, None);
    assert!(result.statistics.is_some());
    let stats = result.statistics.unwrap();
    assert!(stats.ratio >= 1.0);
    assert!(stats.chars_saved > 0);
    assert!(stats.percentage_saved > 0.0);
    assert!(stats.percentage_saved <= 100.0);
}

#[test]
fn statistics_none_when_not_compressed() {
    let config = CompressionConfig::default();
    let result = compress(&config, "Read", "small", None);
    assert!(result.statistics.is_none());
}

#[test]
fn original_size_correct() {
    let config = CompressionConfig::default();
    let input = large_input(5_000);
    let result = compress(&config, "Read", &input, None);
    assert_eq!(result.original_size, 5_000);
}

#[test]
fn case_insensitive_tool_name() {
    let config = CompressionConfig::default();
    let input = large_input(5_000);
    let r1 = compress(&config, "read", &input, None);
    let r2 = compress(&config, "Read", &input, None);
    let r3 = compress(&config, "READ", &input, None);
    assert!(r1.compression_applied);
    assert!(r2.compression_applied);
    assert!(r3.compression_applied);
}

#[test]
fn budget_guarantee() {
    let config = CompressionConfig::default();
    let input = large_input(50_000);
    let result = compress(&config, "Bash", &input, None);
    assert!(result.text.chars().count() <= config.target_budget);
}

// --- T042: Exhaustive budget guarantee (covers all tool types x multiple sizes) ---

#[test]
fn exhaustive_budget_guarantee_all_tool_types() {
    let config = CompressionConfig::default();
    let tool_names = [
        "Read",
        "Bash",
        "Grep",
        "Glob",
        "Edit",
        "Write",
        "CustomTool",
        "Unknown",
    ];
    let sizes = [3_001, 5_000, 10_000, 50_000, 100_000];

    for tool in &tool_names {
        for &size in &sizes {
            let input = large_input(size);
            let result = compress(&config, tool, &input, None);
            if result.compression_applied {
                assert!(
                    result.text.chars().count() <= config.target_budget,
                    "Budget violated for tool={tool}, size={size}: got {} chars",
                    result.text.chars().count()
                );
            }
        }
    }
}

#[test]
fn exhaustive_budget_guarantee_structured_content() {
    let config = CompressionConfig::default();
    let cases: Vec<(&str, String, Option<&str>)> = vec![
        (
            "Read",
            {
                let mut s = String::from("import React from 'react';\n");
                for i in 0..500 {
                    s.push_str(&format!("const val{i} = {i};\n"));
                }
                s
            },
            Some("app.js"),
        ),
        (
            "Bash",
            {
                let mut s = String::new();
                for i in 0..500 {
                    s.push_str(&format!("error: something failed at line {i}\n"));
                }
                s
            },
            Some("npm test"),
        ),
        (
            "Grep",
            {
                let mut s = String::new();
                for i in 0..500 {
                    s.push_str(&format!("src/mod{}/file.rs:10: match {i}\n", i % 20));
                }
                s
            },
            Some("pattern"),
        ),
        (
            "Glob",
            {
                let mut s = String::new();
                for i in 0..500 {
                    s.push_str(&format!("src/dir{}/file{}.rs\n", i % 20, i));
                }
                s
            },
            Some("**/*.rs"),
        ),
        ("Edit", "a\n".repeat(5000), Some("src/main.rs")),
    ];

    for (tool, input, ctx) in &cases {
        let result = compress(&config, tool, input, *ctx);
        if result.compression_applied {
            assert!(
                result.text.chars().count() <= config.target_budget,
                "Budget violated for tool={tool}: got {} chars",
                result.text.chars().count()
            );
        }
    }
}

// --- T044: Custom CompressionConfig ---

#[test]
fn custom_config_threshold_5000_budget_3000() {
    let config = CompressionConfig {
        compression_threshold: 5_000,
        target_budget: 3_000,
    };

    // Below custom threshold: no compression
    let small = large_input(4_999);
    let result = compress(&config, "Read", &small, None);
    assert!(!result.compression_applied);

    // At custom threshold: no compression
    let at = large_input(5_000);
    let result = compress(&config, "Read", &at, None);
    assert!(!result.compression_applied);

    // Above custom threshold: compression with custom budget
    let big = large_input(10_000);
    let result = compress(&config, "Read", &big, None);
    assert!(result.compression_applied);
    assert!(
        result.text.chars().count() <= 3_000,
        "Custom budget violated: got {} chars",
        result.text.chars().count()
    );
}

#[test]
fn custom_config_all_tools_respect_budget() {
    let config = CompressionConfig {
        compression_threshold: 5_000,
        target_budget: 3_000,
    };
    let input = large_input(15_000);

    for tool in &[
        "Read",
        "Bash",
        "Grep",
        "Glob",
        "Edit",
        "Write",
        "CustomTool",
    ] {
        let result = compress(&config, tool, &input, None);
        assert!(result.compression_applied);
        assert!(
            result.text.chars().count() <= 3_000,
            "Budget violated for {tool}: got {} chars",
            result.text.chars().count()
        );
    }
}

// --- T045: Edge case tests ---

#[test]
fn edge_ec1_empty_output_all_tools() {
    let config = CompressionConfig::default();
    for tool in &[
        "Read",
        "Bash",
        "Grep",
        "Glob",
        "Edit",
        "Write",
        "CustomTool",
    ] {
        let result = compress(&config, tool, "", None);
        assert!(!result.compression_applied, "EC-1 failed for {tool}");
        assert_eq!(result.text, "");
    }
}

#[test]
fn edge_ec2_whitespace_only_all_tools() {
    let config = CompressionConfig::default();
    let whitespace_inputs = ["   ", "\n\n\n", "\t\t", "  \n  \t  "];
    for ws in &whitespace_inputs {
        for tool in &["Read", "Bash", "Grep", "Glob", "Edit"] {
            let result = compress(&config, tool, ws, None);
            assert!(
                !result.compression_applied,
                "EC-2 failed for {tool} with {ws:?}"
            );
            assert_eq!(result.text, *ws);
        }
    }
}

#[test]
fn edge_ec3_no_construct_read_falls_to_generic() {
    let config = CompressionConfig::default();
    let input = "x".repeat(5_000);
    let result = compress(&config, "Read", &input, Some("data.bin"));
    assert!(result.compression_applied);
    assert!(
        result.text.contains("truncated")
            || result.text.contains("omitted")
            || result.text.len() < input.len(),
        "EC-3: expected generic fallback behavior"
    );
}

#[test]
fn edge_ec4_grep_no_file_paths() {
    let config = CompressionConfig::default();
    let mut input = String::new();
    for i in 0..500 {
        input.push_str(&format!("just some random text line {i}\n"));
    }
    let result = compress(&config, "Grep", &input, None);
    assert!(result.compression_applied);
    assert!(result.text.chars().count() <= config.target_budget);
}

#[test]
fn edge_ec5_glob_neither_line_nor_json() {
    let config = CompressionConfig::default();
    let input = "x".repeat(5_000);
    let result = compress(&config, "Glob", &input, None);
    assert!(result.compression_applied);
    assert!(result.text.chars().count() <= config.target_budget);
}

#[test]
fn edge_ec6_case_insensitive_tool_names() {
    let config = CompressionConfig::default();
    let input = large_input(5_000);
    let variants = [
        ("read", "READ", "Read"),
        ("bash", "BASH", "Bash"),
        ("grep", "GREP", "Grep"),
        ("glob", "GLOB", "Glob"),
        ("edit", "EDIT", "Edit"),
        ("write", "WRITE", "Write"),
    ];

    for (lower, upper, mixed) in &variants {
        let r1 = compress(&config, lower, &input, None);
        let r2 = compress(&config, upper, &input, None);
        let r3 = compress(&config, mixed, &input, None);
        assert!(r1.compression_applied, "EC-6: {lower} not compressed");
        assert!(r2.compression_applied, "EC-6: {upper} not compressed");
        assert!(r3.compression_applied, "EC-6: {mixed} not compressed");
    }
}

#[test]
fn edge_ec7_multibyte_unicode_char_counting() {
    let config = CompressionConfig::default();
    let emoji = "\u{1F600}"; // 4 bytes, 1 char
    let input = emoji.repeat(5_000); // 5000 chars, 20000 bytes
    assert_eq!(input.chars().count(), 5_000);
    assert_eq!(input.len(), 20_000);

    let result = compress(&config, "Read", &input, None);
    assert!(result.compression_applied);
    assert!(
        result.text.chars().count() <= config.target_budget,
        "EC-7: Unicode budget violated: {} chars (but {} bytes)",
        result.text.chars().count(),
        result.text.len()
    );
    assert_eq!(
        result.original_size, 5_000,
        "EC-7: original_size should count chars"
    );
}
