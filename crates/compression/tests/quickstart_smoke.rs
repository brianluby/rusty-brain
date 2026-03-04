//! Smoke test verifying the quickstart.md usage example compiles and works.

use compression::{CompressionConfig, compress};

#[test]
fn quickstart_default_config_usage() {
    let config = CompressionConfig::default();
    assert_eq!(config.compression_threshold, 3_000);
    assert_eq!(config.target_budget, 2_000);

    // Simulate a large file read
    let large_file_content = "use std::io;\n".repeat(500);
    let result = compress(
        &config,
        "Read",
        &large_file_content,
        Some("/path/to/file.rs"),
    );

    if result.compression_applied {
        assert!(result.text.chars().count() <= config.target_budget);
        let stats = result
            .statistics
            .as_ref()
            .expect("stats present when compressed");
        assert!(stats.ratio >= 1.0);
        assert!(stats.percentage_saved > 0.0);
    }
}

#[test]
fn quickstart_custom_config_usage() {
    let config = CompressionConfig {
        compression_threshold: 5_000,
        target_budget: 3_000,
    };

    let large_file_content = "fn hello() {}\n".repeat(1000);
    let result = compress(&config, "Read", &large_file_content, Some("app.rs"));

    assert!(result.compression_applied);
    assert!(result.text.chars().count() <= 3_000);
}

#[test]
fn quickstart_all_tool_types_listed() {
    let config = CompressionConfig::default();
    let large_input = "x\n".repeat(5_000);

    // All tools from the quickstart table
    for tool in &["Read", "Bash", "Grep", "Glob", "Edit", "Write", "other"] {
        let result = compress(&config, tool, &large_input, None);
        assert!(
            result.compression_applied,
            "Tool {tool} should compress large input"
        );
        assert!(
            result.text.chars().count() <= config.target_budget,
            "Tool {tool} budget violated"
        );
    }
}

#[test]
fn quickstart_passthrough_behavior() {
    let config = CompressionConfig::default();

    // Empty
    let result = compress(&config, "Read", "", None);
    assert!(!result.compression_applied);

    // Whitespace-only
    let result = compress(&config, "Read", "  \n  ", None);
    assert!(!result.compression_applied);

    // Below threshold
    let result = compress(&config, "Read", "small output", None);
    assert!(!result.compression_applied);
    assert!(result.statistics.is_none());
}
