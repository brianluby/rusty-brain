//! Grep tool compressor — groups matches by file with counts.

use std::collections::BTreeMap;
use std::fmt::Write;

use crate::config::CompressionConfig;

const TOP_MATCHES: usize = 10;

/// Compress grep output by grouping matches by file.
///
/// Parses `file:line:content` format, groups by file path, and shows
/// match counts per file with top individual matches.
pub fn compress(config: &CompressionConfig, output: &str, input_context: Option<&str>) -> String {
    // config accepted for signature consistency; budget enforcement is handled by the dispatcher
    let _ = config;
    let mut file_matches: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    let mut ungrouped = Vec::new();

    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Try to parse file:line:content or file:line format
        if let Some((file_part, _rest)) = trimmed.split_once(':') {
            // Heuristic: file paths contain / or have a dotted extension, and never contain spaces
            if !file_part.contains(' ') && (file_part.contains('/') || file_part.contains('.')) {
                file_matches.entry(file_part).or_default().push(trimmed);
            } else {
                ungrouped.push(trimmed);
            }
        } else {
            ungrouped.push(trimmed);
        }
    }

    // If no file grouping was possible, fall through to generic compressor
    if file_matches.is_empty() {
        return crate::generic::compress(config, output, input_context);
    }

    let total_files = file_matches.len();
    let total_matches: usize = file_matches.values().map(Vec::len).sum();

    let mut result = String::new();

    if let Some(query) = input_context {
        let _ = writeln!(result, "[Grep: {query}]");
    }
    let _ = write!(
        result,
        "[{total_matches} matches across {total_files} files]\n\n"
    );

    // Sort files by match count descending, then alphabetically for determinism
    let mut sorted_files: Vec<_> = file_matches.iter().collect();
    sorted_files.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then_with(|| a.0.cmp(b.0)));

    result.push_str("Files:\n");
    for (file, matches) in &sorted_files {
        let _ = writeln!(result, "  {file}: {} matches", matches.len());
    }

    let _ = write!(result, "\nTop {TOP_MATCHES} matches:\n");
    let mut shown = 0;
    for (_file, matches) in &sorted_files {
        for m in *matches {
            if shown >= TOP_MATCHES {
                break;
            }
            let _ = writeln!(result, "  {m}");
            shown += 1;
        }
        if shown >= TOP_MATCHES {
            break;
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use crate::{CompressionConfig, compress as dispatch};

    use super::*;

    fn large_grep_output() -> String {
        let mut output = String::new();
        for file_idx in 0..40 {
            for match_idx in 0..5 {
                output.push_str(&format!(
                    "src/file{file_idx}.rs:{match_idx}:    let value = compute();\n"
                ));
            }
        }
        output
    }

    #[test]
    fn groups_by_file_with_counts() {
        let config = CompressionConfig::default();
        let output = large_grep_output();
        let result = compress(&config, &output, Some("compute"));
        assert!(result.contains("200 matches across 40 files"));
        assert!(result.contains("src/file0.rs: 5 matches"));
    }

    #[test]
    fn shows_top_matches() {
        let config = CompressionConfig::default();
        let output = large_grep_output();
        let result = compress(&config, &output, Some("compute"));
        assert!(result.contains("Top 10 matches"));
    }

    #[test]
    fn query_in_header() {
        let config = CompressionConfig::default();
        let result = compress(&config, "src/a.rs:1:found\n", Some("search_term"));
        assert!(result.contains("[Grep: search_term]"));
    }

    #[test]
    fn no_file_paths_returns_raw() {
        let config = CompressionConfig::default();
        let input = "match 1\nmatch 2\nmatch 3\n";
        let result = compress(&config, input, None);
        // No grouping possible, returned as-is for generic fallback
        assert_eq!(result, input);
    }

    #[test]
    fn rejects_error_lines_as_file_paths() {
        let config = CompressionConfig::default();
        // "error message.txt" contains a space — should NOT be grouped as a file path
        let input = "error message.txt: something failed\nreal error\n";
        let result = compress(&config, input, None);
        // Falls through to generic since no valid file paths found
        assert_eq!(result, input);
    }

    #[test]
    fn deterministic_output() {
        let config = CompressionConfig::default();
        let output = large_grep_output();
        let result1 = compress(&config, &output, Some("compute"));
        let result2 = compress(&config, &output, Some("compute"));
        assert_eq!(result1, result2);
    }

    #[test]
    fn through_dispatcher_budget() {
        let config = CompressionConfig::default();
        let output = large_grep_output();
        assert!(output.chars().count() > config.compression_threshold);
        let result = dispatch(&config, "Grep", &output, Some("compute"));
        assert!(result.compression_applied);
        assert!(result.text.chars().count() <= config.target_budget);
    }
}
