//! Glob tool compressor — groups files by directory with counts.

use std::collections::BTreeMap;
use std::fmt::Write;

use crate::config::CompressionConfig;

const TOP_DIRS: usize = 5;
const SAMPLES_PER_DIR: usize = 3;

/// Compress glob output by grouping files by directory.
///
/// Tries JSON array parsing first (via `serde_json`), falls back to manual
/// comma-split, then line-delimited paths.
/// Groups files by directory and shows top directories with counts.
pub fn compress(config: &CompressionConfig, output: &str, input_context: Option<&str>) -> String {
    let _ = config;
    let paths = parse_paths(output);

    if paths.is_empty() {
        return output.to_string();
    }

    let mut dir_files: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for path in paths {
        let dir = path
            .rsplit_once('/')
            .map_or(".", |(dir, _)| if dir.is_empty() { "/" } else { dir })
            .to_string();
        dir_files.entry(dir).or_default().push(path);
    }

    let total_files: usize = dir_files.values().map(Vec::len).sum();
    let total_dirs = dir_files.len();

    let mut result = String::new();

    if let Some(query) = input_context {
        let _ = writeln!(result, "[Glob: {query}]");
    }
    let _ = write!(
        result,
        "[{total_files} files across {total_dirs} directories]\n\n"
    );

    // Sort directories by file count descending, then alphabetically for determinism
    let mut sorted_dirs: Vec<_> = dir_files.iter().collect();
    sorted_dirs.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then_with(|| a.0.cmp(b.0)));

    for (i, (dir, files)) in sorted_dirs.iter().enumerate() {
        if i >= TOP_DIRS {
            let remaining_dirs = sorted_dirs.len() - TOP_DIRS;
            let remaining_files: usize = sorted_dirs[TOP_DIRS..].iter().map(|(_, f)| f.len()).sum();
            let _ = writeln!(
                result,
                "  ...and {remaining_dirs} more directories ({remaining_files} files)"
            );
            break;
        }
        let _ = writeln!(result, "{dir}/ ({} files):", files.len());
        for (j, file) in files.iter().enumerate() {
            if j >= SAMPLES_PER_DIR {
                let remaining = files.len() - SAMPLES_PER_DIR;
                let _ = writeln!(result, "    ...and {remaining} more");
                break;
            }
            let filename = file
                .rsplit_once('/')
                .map_or(file.as_str(), |(_, name)| name);
            let _ = writeln!(result, "    {filename}");
        }
    }

    result
}

/// Parse paths from output — try JSON array first (via `serde_json`), then line-delimited.
fn parse_paths(output: &str) -> Vec<String> {
    let trimmed = output.trim();

    // JSON arrays: parse with serde_json for correctness (handles commas in paths, escapes)
    if trimmed.starts_with('[') && trimmed.ends_with(']') {
        if let Ok(paths) = serde_json::from_str::<Vec<String>>(trimmed) {
            return paths;
        }
        // Fallback: manual split for non-standard JSON-like arrays
        let inner = &trimmed[1..trimmed.len() - 1];
        return inner
            .split(',')
            .map(|s| s.trim().trim_matches('"').to_string())
            .filter(|s| !s.is_empty())
            .collect();
    }

    // Line-delimited paths
    output
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|line| !line.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use crate::{CompressionConfig, compress as dispatch};

    use super::*;

    fn large_glob_output() -> String {
        let mut output = String::new();
        for dir_idx in 0..20 {
            for file_idx in 0..25 {
                output.push_str(&format!("src/module{dir_idx}/file{file_idx}.rs\n"));
            }
        }
        output
    }

    fn json_glob_output() -> String {
        let mut paths = Vec::new();
        for dir_idx in 0..10 {
            for file_idx in 0..20 {
                paths.push(format!("src/mod{dir_idx}/f{file_idx}.ts"));
            }
        }
        serde_json::to_string(&paths).unwrap()
    }

    #[test]
    fn groups_by_directory() {
        let config = CompressionConfig::default();
        let output = large_glob_output();
        let result = compress(&config, &output, Some("**/*.rs"));
        assert!(result.contains("500 files across 20 directories"));
    }

    #[test]
    fn shows_top_directories() {
        let config = CompressionConfig::default();
        let output = large_glob_output();
        let result = compress(&config, &output, Some("**/*.rs"));
        assert!(result.contains("25 files"));
    }

    #[test]
    fn json_format_parsed() {
        let config = CompressionConfig::default();
        let output = json_glob_output();
        let result = compress(&config, &output, Some("**/*.ts"));
        assert!(result.contains("200 files"));
        assert!(result.contains("10 directories"));
    }

    #[test]
    fn json_with_commas_in_paths() {
        let paths = vec![
            "src/file,v2.rs".to_string(),
            "src/another,file.rs".to_string(),
        ];
        let json = serde_json::to_string(&paths).unwrap();
        let config = CompressionConfig::default();
        let result = compress(&config, &json, None);
        assert!(result.contains("2 files"));
    }

    #[test]
    fn query_in_header() {
        let config = CompressionConfig::default();
        let result = compress(&config, "src/a.rs\nsrc/b.rs\n", Some("**/*.rs"));
        assert!(result.contains("[Glob: **/*.rs]"));
    }

    #[test]
    fn empty_paths_returns_raw() {
        let config = CompressionConfig::default();
        let result = compress(&config, "", None);
        assert_eq!(result, "");
    }

    #[test]
    fn deterministic_output() {
        let config = CompressionConfig::default();
        let output = large_glob_output();
        let result1 = compress(&config, &output, Some("**/*.rs"));
        let result2 = compress(&config, &output, Some("**/*.rs"));
        assert_eq!(result1, result2);
    }

    #[test]
    fn through_dispatcher_budget() {
        let config = CompressionConfig::default();
        let output = large_glob_output();
        assert!(output.chars().count() > config.compression_threshold);
        let result = dispatch(&config, "Glob", &output, Some("**/*.rs"));
        assert!(result.compression_applied);
        assert!(result.text.chars().count() <= config.target_budget);
    }
}
