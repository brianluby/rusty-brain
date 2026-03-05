//! Tool-output compression for rusty-brain.
//!
//! Compresses large tool outputs to ~500 tokens while preserving
//! the most semantically important content for each tool type.

mod bash;
mod config;
mod edit;
mod generic;
mod glob;
mod grep;
mod lang;
mod read;
mod regex_util;
mod truncate;
mod types;

pub use config::CompressionConfig;
pub use types::{CompressedResult, CompressionStatistics, ToolType};

use std::panic;

use truncate::enforce_budget;
use types::ToolType::{Bash, Edit, Glob, Grep, Other, Read, Write};

/// Compress a tool output according to its tool type.
///
/// This is the primary entry point for the compression pipeline.
/// It is infallible: it never panics and never returns an error.
///
/// # Behavior
///
/// 1. Empty or whitespace-only input → returned unchanged (`compression_applied: false`)
/// 2. Input at or below `config.compression_threshold` → returned unchanged
/// 3. Dispatch to specialized compressor by `tool_name` (case-insensitive)
/// 4. On compressor failure → fall back to generic compressor, log warning
/// 5. Final truncation enforces `config.target_budget`
/// 6. Build `CompressedResult` with statistics
///
/// The config is expected to be valid (see `CompressionConfig::validate()`).
/// In debug builds, invalid configs trigger a panic via `debug_assert!`.
#[must_use]
pub fn compress(
    config: &CompressionConfig,
    tool_name: &str,
    output: &str,
    input_context: Option<&str>,
) -> CompressedResult {
    debug_assert!(
        config.validate().is_ok(),
        "invalid CompressionConfig: {}",
        config.validate().unwrap_err()
    );

    let original_size = output.chars().count();

    // Pass-through: empty, whitespace-only, or below threshold
    if output.is_empty()
        || output.trim().is_empty()
        || original_size <= config.compression_threshold
    {
        return CompressedResult {
            text: output.to_string(),
            compression_applied: false,
            original_size,
            statistics: None,
        };
    }

    // Dispatch to specialized compressor, with catch_unwind for panic safety
    let tool_type = ToolType::from(tool_name);
    let compressed = dispatch_with_fallback(config, &tool_type, output, input_context);

    // Final budget enforcement
    let text = enforce_budget(&compressed, config.target_budget);
    let compressed_size = text.chars().count();

    #[allow(clippy::cast_precision_loss)]
    let statistics = CompressionStatistics {
        ratio: original_size as f64 / compressed_size.max(1) as f64,
        chars_saved: original_size.saturating_sub(compressed_size),
        percentage_saved: if original_size > 0 {
            (original_size.saturating_sub(compressed_size)) as f64 / original_size as f64 * 100.0
        } else {
            0.0
        },
    };

    CompressedResult {
        text,
        compression_applied: true,
        original_size,
        statistics: Some(statistics),
    }
}

/// Dispatch to the specialized compressor. On panic, fall back to generic.
fn dispatch_with_fallback(
    config: &CompressionConfig,
    tool_type: &ToolType,
    output: &str,
    input_context: Option<&str>,
) -> String {
    // Generic compressor is the fallback — call it directly without catch_unwind.
    //
    // The #[cfg(test)] / #[cfg(not(test))] split exists solely to allow the
    // panic-recovery integration test: `Other("panic_test")` triggers a deliberate
    // panic inside catch_unwind so we can verify the fallback path. In production,
    // all `Other(...)` variants route straight to generic without catch_unwind overhead.
    #[cfg(not(test))]
    if matches!(tool_type, Other(_)) {
        return generic::compress(config, output, input_context);
    }
    #[cfg(test)]
    if matches!(tool_type, Other(name) if name != "panic_test") {
        return generic::compress(config, output, input_context);
    }

    let result = panic::catch_unwind(panic::AssertUnwindSafe(|| match tool_type {
        Read => read::compress(config, output, input_context),
        Bash => bash::compress(config, output, input_context),
        Grep => grep::compress(config, output, input_context),
        Glob => glob::compress(config, output, input_context),
        Edit => edit::compress(config, output, input_context, false),
        Write => edit::compress(config, output, input_context, true),
        #[cfg(test)]
        Other(name) if name == "panic_test" => panic!("simulated compressor panic"),
        Other(_) => unreachable!(),
    }));

    result.unwrap_or_else(|_| {
        tracing::warn!(
            tool = %tool_type,
            "Specialized compressor panicked, falling back to generic"
        );
        generic::compress(config, output, input_context)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn large_input(size: usize) -> String {
        "a".repeat(size)
    }

    /// Build a large structured input with numbered lines.
    fn large_lines(count: usize) -> String {
        let mut s = String::new();
        for i in 0..count {
            s.push_str(&format!("line {i}: some content here for testing\n"));
        }
        s
    }

    // --- T043: Panic recovery (must stay as unit tests — depends on #[cfg(test)] paths) ---

    #[test]
    fn panic_recovery_falls_back_to_generic() {
        let config = CompressionConfig::default();
        let input = large_input(5_000);
        // "panic_test" is a test-only Other variant that triggers a panic inside catch_unwind
        let result = compress(&config, "panic_test", &input, None);
        // Should not panic — should fall back to generic compressor
        assert!(result.compression_applied);
        assert!(result.text.chars().count() <= config.target_budget);
    }

    #[test]
    fn panic_recovery_produces_valid_output() {
        let config = CompressionConfig::default();
        // Use structured content to verify generic fallback produces meaningful output
        let mut input = String::new();
        for i in 0..500 {
            input.push_str(&format!("line {i}: some content here\n"));
        }
        let result = compress(&config, "panic_test", &input, None);
        assert!(result.compression_applied);
        // Generic compressor preserves head lines
        assert!(result.text.contains("line 0"));
    }

    // -------------------------------------------------------------------------
    // T032: compress() entry point — various tool types, sizes, configs
    // -------------------------------------------------------------------------

    // --- Pass-through behavior ---

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
    fn whitespace_only_input_returns_unchanged() {
        let config = CompressionConfig::default();
        let result = compress(&config, "Bash", "   \n\t  ", None);
        assert!(!result.compression_applied);
        assert_eq!(result.text, "   \n\t  ");
        assert!(result.statistics.is_none());
    }

    #[test]
    fn below_threshold_returns_unchanged() {
        let config = CompressionConfig::default();
        let input = "short output";
        let result = compress(&config, "Read", input, None);
        assert!(!result.compression_applied);
        assert_eq!(result.text, input);
        assert_eq!(result.original_size, input.chars().count());
        assert!(result.statistics.is_none());
    }

    #[test]
    fn at_threshold_returns_unchanged() {
        let config = CompressionConfig::default();
        let input = "x".repeat(config.compression_threshold);
        let result = compress(&config, "Read", &input, None);
        assert!(!result.compression_applied);
        assert_eq!(result.text, input);
    }

    // --- Tool-type dispatch: Read ---

    #[test]
    fn read_tool_compresses_large_input() {
        let config = CompressionConfig::default();
        let input = large_lines(500);
        let result = compress(&config, "Read", &input, None);
        assert!(result.compression_applied);
        assert!(result.text.chars().count() <= config.target_budget);
        assert!(result.statistics.is_some());
    }

    #[test]
    fn read_tool_with_context() {
        let config = CompressionConfig::default();
        let input = large_lines(500);
        let result = compress(&config, "Read", &input, Some("file.rs"));
        assert!(result.compression_applied);
        assert!(result.text.chars().count() <= config.target_budget);
    }

    // --- Tool-type dispatch: Bash ---

    #[test]
    fn bash_tool_compresses_large_input() {
        let config = CompressionConfig::default();
        let input = large_lines(500);
        let result = compress(&config, "Bash", &input, None);
        assert!(result.compression_applied);
        assert!(result.text.chars().count() <= config.target_budget);
    }

    #[test]
    fn bash_tool_case_insensitive() {
        let config = CompressionConfig::default();
        let input = large_lines(500);
        let result = compress(&config, "BASH", &input, None);
        assert!(result.compression_applied);
    }

    // --- Tool-type dispatch: Grep ---

    #[test]
    fn grep_tool_compresses_large_input() {
        let config = CompressionConfig::default();
        let input = large_lines(500);
        let result = compress(&config, "Grep", &input, None);
        assert!(result.compression_applied);
        assert!(result.text.chars().count() <= config.target_budget);
    }

    // --- Tool-type dispatch: Glob ---

    #[test]
    fn glob_tool_compresses_large_input() {
        let config = CompressionConfig::default();
        let mut input = String::new();
        for i in 0..1000 {
            input.push_str(&format!("src/module_{i}/file_{i}.rs\n"));
        }
        let result = compress(&config, "Glob", &input, None);
        assert!(result.compression_applied);
        assert!(result.text.chars().count() <= config.target_budget);
    }

    // --- Tool-type dispatch: Edit ---

    #[test]
    fn edit_tool_compresses_large_input() {
        let config = CompressionConfig::default();
        let input = large_lines(500);
        let result = compress(&config, "Edit", &input, None);
        assert!(result.compression_applied);
        assert!(result.text.chars().count() <= config.target_budget);
    }

    // --- Tool-type dispatch: Write ---

    #[test]
    fn write_tool_compresses_large_input() {
        let config = CompressionConfig::default();
        let input = large_lines(500);
        let result = compress(&config, "Write", &input, None);
        assert!(result.compression_applied);
        assert!(result.text.chars().count() <= config.target_budget);
    }

    // --- Tool-type dispatch: unknown/Other ---

    #[test]
    fn unknown_tool_uses_generic_compressor() {
        let config = CompressionConfig::default();
        let input = large_lines(500);
        let result = compress(&config, "WebFetch", &input, None);
        assert!(result.compression_applied);
        assert!(result.text.chars().count() <= config.target_budget);
    }

    #[test]
    fn ls_tool_uses_generic_compressor() {
        let config = CompressionConfig::default();
        let input = large_lines(500);
        let result = compress(&config, "LS", &input, None);
        assert!(result.compression_applied);
        assert!(result.text.chars().count() <= config.target_budget);
    }

    // --- Statistics correctness ---

    #[test]
    fn statistics_ratio_is_at_least_one() {
        let config = CompressionConfig::default();
        let input = large_input(5_000);
        let result = compress(&config, "Read", &input, None);
        let stats = result.statistics.expect("must have statistics");
        assert!(
            stats.ratio >= 1.0,
            "ratio must be >= 1.0, got {}",
            stats.ratio
        );
    }

    #[test]
    fn statistics_chars_saved_is_correct() {
        let config = CompressionConfig::default();
        let input = large_input(5_000);
        let result = compress(&config, "Read", &input, None);
        let stats = result.statistics.expect("must have statistics");
        let compressed_size = result.text.chars().count();
        assert_eq!(stats.chars_saved, result.original_size - compressed_size);
    }

    #[test]
    fn statistics_percentage_saved_in_range() {
        let config = CompressionConfig::default();
        let input = large_input(5_000);
        let result = compress(&config, "Read", &input, None);
        let stats = result.statistics.expect("must have statistics");
        assert!(
            stats.percentage_saved >= 0.0 && stats.percentage_saved <= 100.0,
            "percentage_saved must be 0-100, got {}",
            stats.percentage_saved
        );
    }

    #[test]
    fn original_size_is_correct() {
        let config = CompressionConfig::default();
        let input = large_input(5_000);
        let result = compress(&config, "Read", &input, None);
        assert_eq!(result.original_size, 5_000);
    }

    // --- Custom config combinations ---

    #[test]
    fn custom_config_lower_threshold() {
        let config = CompressionConfig {
            compression_threshold: 100,
            target_budget: 50,
        };
        let input = "x".repeat(200);
        let result = compress(&config, "Read", &input, None);
        assert!(result.compression_applied);
        assert!(result.text.chars().count() <= 50);
    }

    #[test]
    fn custom_config_larger_budget() {
        let config = CompressionConfig {
            compression_threshold: 5_000,
            target_budget: 4_000,
        };
        let input = large_lines(500);
        let result = compress(&config, "Read", &input, None);
        assert!(result.compression_applied);
        assert!(result.text.chars().count() <= 4_000);
    }

    // --- Input just above threshold ---

    #[test]
    fn input_just_above_threshold_is_compressed() {
        let config = CompressionConfig::default();
        let input = "x".repeat(config.compression_threshold + 1);
        let result = compress(&config, "Read", &input, None);
        assert!(result.compression_applied);
        assert!(result.text.chars().count() <= config.target_budget);
    }

    // --- All tool types produce valid output (no crash/panic) ---

    #[test]
    fn all_known_tools_produce_valid_output() {
        let config = CompressionConfig::default();
        let input = large_lines(500);
        for tool in &["Read", "Bash", "Grep", "Glob", "Edit", "Write"] {
            let result = compress(&config, tool, &input, None);
            assert!(
                result.compression_applied,
                "{tool}: must compress large input"
            );
            assert!(
                result.text.chars().count() <= config.target_budget,
                "{tool}: must respect target_budget"
            );
            assert!(
                result.statistics.is_some(),
                "{tool}: must produce statistics"
            );
        }
    }

    // --- Case insensitivity for all tools ---

    #[test]
    fn tool_name_case_insensitive_for_all_tools() {
        let config = CompressionConfig::default();
        let input = large_lines(500);
        for (lower, upper) in &[
            ("read", "READ"),
            ("bash", "BASH"),
            ("grep", "GREP"),
            ("glob", "GLOB"),
            ("edit", "EDIT"),
            ("write", "WRITE"),
        ] {
            let r1 = compress(&config, lower, &input, None);
            let r2 = compress(&config, upper, &input, None);
            assert_eq!(
                r1.compression_applied, r2.compression_applied,
                "case must not affect compression: {lower} vs {upper}"
            );
        }
    }
}
