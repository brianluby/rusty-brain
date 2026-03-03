//! Edit/Write tool compressor — file path + change summary.

use std::fmt::Write;

use crate::config::CompressionConfig;

const PREVIEW_CHARS: usize = 500;

/// Compress edit or write output to file path + change summary.
///
/// When `is_write` is true, produces a "File created" indicator.
/// When `is_write` is false, produces a "Changes applied" indicator.
pub fn compress(
    config: &CompressionConfig,
    output: &str,
    input_context: Option<&str>,
    is_write: bool,
) -> String {
    // config accepted for signature consistency; budget enforcement is handled by the dispatcher
    let _ = config;
    let action = if is_write {
        "File created"
    } else {
        "Changes applied"
    };

    let mut result = String::new();

    if let Some(path) = input_context {
        let _ = writeln!(result, "[{action}: {path}]");
    } else {
        let _ = writeln!(result, "[{action}]");
    }

    let line_count = output.lines().count();
    let _ = write!(result, "[{line_count} lines]\n\n");

    // Include a preview of the content (single-pass check for truncation)
    let mut chars = output.chars();
    let preview: String = chars.by_ref().take(PREVIEW_CHARS).collect();
    result.push_str(&preview);
    if chars.next().is_some() {
        result.push_str("\n[...content truncated...]");
    }

    result
}

#[cfg(test)]
mod tests {
    use crate::{CompressionConfig, compress as dispatch};

    use super::*;

    #[test]
    fn edit_shows_changes_applied() {
        let config = CompressionConfig::default();
        let output = "line 1\nline 2\nline 3\n";
        let result = compress(&config, output, Some("src/main.rs"), false);
        assert!(result.contains("Changes applied"));
        assert!(result.contains("src/main.rs"));
    }

    #[test]
    fn write_shows_file_created() {
        let config = CompressionConfig::default();
        let output = "new file content\n";
        let result = compress(&config, output, Some("src/new.rs"), true);
        assert!(result.contains("File created"));
        assert!(result.contains("src/new.rs"));
    }

    #[test]
    fn large_diff_truncated() {
        let config = CompressionConfig::default();
        let output = "x".repeat(5_000);
        let result = compress(&config, &output, Some("src/big.rs"), false);
        assert!(result.contains("[...content truncated...]"));
    }

    #[test]
    fn short_edit_not_truncated() {
        let config = CompressionConfig::default();
        let output = "small change\n";
        let result = compress(&config, output, Some("src/small.rs"), false);
        assert!(!result.contains("[...content truncated...]"));
    }

    #[test]
    fn through_dispatcher_edit() {
        let config = CompressionConfig::default();
        let output = "diff content\n".repeat(500);
        let result = dispatch(&config, "Edit", &output, Some("src/file.rs"));
        if result.compression_applied {
            assert!(result.text.chars().count() <= config.target_budget);
            assert!(result.text.contains("Changes applied"));
        }
    }

    #[test]
    fn through_dispatcher_write() {
        let config = CompressionConfig::default();
        let output = "new content\n".repeat(500);
        let result = dispatch(&config, "Write", &output, Some("src/new.rs"));
        if result.compression_applied {
            assert!(result.text.chars().count() <= config.target_budget);
            assert!(result.text.contains("File created"));
        }
    }

    #[test]
    fn no_input_context() {
        let config = CompressionConfig::default();
        let result = compress(&config, "content", None, false);
        assert!(result.contains("[Changes applied]"));
    }
}
