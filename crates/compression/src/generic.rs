//! Generic fallback compressor — head/tail truncation.

use std::fmt::Write;

use crate::config::CompressionConfig;

const HEAD_LINES: usize = 15;
const TAIL_LINES: usize = 10;

/// Compress using generic head/tail truncation strategy.
///
/// Preserves the first 15 lines and last 10 lines with an omission marker.
///
/// **Note:** This function does not enforce `config.target_budget`. When the input
/// has fewer than `HEAD_LINES + TAIL_LINES` lines, it is returned unchanged
/// regardless of character count. The dispatcher in `lib.rs` applies
/// `enforce_budget()` as a final pass after all compressors.
pub fn compress(_config: &CompressionConfig, output: &str, _input_context: Option<&str>) -> String {
    let lines: Vec<&str> = output.lines().collect();
    let total = lines.len();

    if total <= HEAD_LINES + TAIL_LINES {
        return output.to_string();
    }

    let head = &lines[..HEAD_LINES];
    let tail = &lines[total - TAIL_LINES..];
    let omitted = total - HEAD_LINES - TAIL_LINES;

    let mut result = String::new();
    for line in head {
        result.push_str(line);
        result.push('\n');
    }
    let _ = writeln!(result, "[...{omitted} lines omitted...]");
    for (i, line) in tail.iter().enumerate() {
        result.push_str(line);
        if i + 1 < tail.len() {
            result.push('\n');
        }
    }
    let _ = write!(result, "\n[{total} lines total]");
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_lines(n: usize) -> String {
        (1..=n)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn short_input_returned_as_is() {
        let config = CompressionConfig::default();
        let input = "just a few lines\nof text";
        let result = compress(&config, input, None);
        assert_eq!(result, input);
    }

    #[test]
    fn single_line_input_returned_as_is() {
        let config = CompressionConfig::default();
        let input = "single line";
        let result = compress(&config, input, None);
        assert_eq!(result, input);
    }

    #[test]
    fn head_tail_preservation() {
        let config = CompressionConfig::default();
        let input = make_lines(100);
        let result = compress(&config, &input, None);
        let lines: Vec<&str> = result.lines().collect();

        // First 15 lines preserved exactly
        for i in 1..=15 {
            let expected = format!("line {i}");
            assert_eq!(
                lines[i - 1],
                expected,
                "head line {i} mismatch: got {:?}",
                lines[i - 1]
            );
        }
        // Last 10 lines before the summary preserved exactly
        // Output format: 15 head lines, 1 omission marker, 10 tail lines, 1 total marker
        let tail_start = lines.len() - 11; // 10 tail lines + 1 "[N lines total]"
        for (j, i) in (91..=100).enumerate() {
            let expected = format!("line {i}");
            assert_eq!(
                lines[tail_start + j],
                expected,
                "tail line {i} mismatch: got {:?}",
                lines[tail_start + j]
            );
        }
    }

    #[test]
    fn omission_indicator_present() {
        let config = CompressionConfig::default();
        let input = make_lines(100);
        let result = compress(&config, &input, None);
        assert!(
            result.contains("[...75 lines omitted...]"),
            "missing omission indicator in: {result}"
        );
    }

    #[test]
    fn total_line_count_stated() {
        let config = CompressionConfig::default();
        let input = make_lines(100);
        let result = compress(&config, &input, None);
        assert!(
            result.contains("[100 lines total]"),
            "missing total line count marker in: {result}"
        );
    }

    // --- Acceptance tests (US6) via full dispatcher ---

    #[test]
    fn dispatcher_custom_tool_routes_to_generic() {
        let config = CompressionConfig::default();
        // Create 15K-char output with many lines
        let input = make_lines(500);
        assert!(input.chars().count() > config.compression_threshold);

        let result = crate::compress(&config, "CustomTool", &input, None);
        assert!(result.compression_applied);
        assert!(result.text.chars().count() <= config.target_budget);
        // First 15 lines preserved
        assert!(result.text.contains("line 1\n"));
        // Omission indicator
        assert!(result.text.contains("lines omitted"));
    }

    #[test]
    fn dispatcher_webfetch_routes_to_generic() {
        let config = CompressionConfig::default();
        let input = make_lines(500);
        assert!(input.chars().count() > config.compression_threshold);

        let result = crate::compress(&config, "WebFetch", &input, None);
        assert!(result.compression_applied);
        assert!(result.text.chars().count() <= config.target_budget);
    }

    #[test]
    fn acceptance_15k_char_output() {
        let config = CompressionConfig::default();
        // Build ~15K chars of output
        let mut input = String::new();
        for i in 0..500 {
            input.push_str(&format!("output line {i}: some data here\n"));
        }
        assert!(input.chars().count() > 10_000);

        let result = crate::compress(&config, "NotebookEdit", &input, None);
        assert!(result.compression_applied);
        assert!(result.text.chars().count() <= config.target_budget);
        // Head preserved
        assert!(result.text.contains("output line 0"));
        // Total line count stated
        assert!(result.text.contains("500"));
    }
}
