//! Bash tool compressor — preserves errors, warnings, success indicators.

use std::fmt::Write;
use std::sync::LazyLock;

use regex::Regex;

use crate::config::CompressionConfig;

static ERROR_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:error[:\[]|ERR!|FAILED|panic(?:ked)?|fatal|cannot find|not found|undefined|segfault|abort|exit(?:ed)?\s+(?:with\s+)?(?:status|code)\s+[1-9])").expect("BUG: invalid regex literal")
});

static WARNING_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:warn(?:ing)?[:\[]|WARN|deprecated)").expect("BUG: invalid regex literal")
});

static SUCCESS_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:success|passed|✓|✔|ok\b|complet(?:e|ed)|done\b|\bbuilt\b|finished|all\s+\d+\s+tests?\s+passed)")
        .expect("BUG: invalid regex literal")
});

/// Compress bash command output by preserving important lines.
///
/// Prioritizes errors, then warnings, then success indicators.
/// Discards intermediate informational output.
pub fn compress(config: &CompressionConfig, output: &str, input_context: Option<&str>) -> String {
    // config accepted for signature consistency; budget enforcement is handled by the dispatcher
    let _ = config;
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let mut successes = Vec::new();

    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if ERROR_PATTERN.is_match(trimmed) {
            errors.push(trimmed);
        } else if WARNING_PATTERN.is_match(trimmed) {
            warnings.push(trimmed);
        } else if SUCCESS_PATTERN.is_match(trimmed) {
            successes.push(trimmed);
        }
    }

    let total_lines = output.lines().count();
    let mut result = String::new();

    if let Some(cmd) = input_context {
        let _ = writeln!(result, "[Command: {cmd}]");
    }
    let _ = write!(result, "[{total_lines} lines total]\n\n");

    if !errors.is_empty() {
        let _ = writeln!(result, "ERRORS ({}):", errors.len());
        for line in &errors {
            let _ = writeln!(result, "{line}");
        }
        result.push('\n');
    }

    if !warnings.is_empty() {
        let _ = writeln!(result, "WARNINGS ({}):", warnings.len());
        for line in &warnings {
            let _ = writeln!(result, "{line}");
        }
        result.push('\n');
    }

    if !successes.is_empty() {
        let _ = writeln!(result, "SUCCESS ({}):", successes.len());
        for line in &successes {
            let _ = writeln!(result, "{line}");
        }
    }

    if errors.is_empty() && warnings.is_empty() && successes.is_empty() {
        result.push_str("[No errors, warnings, or success indicators found]\n");
    }

    result
}

#[cfg(test)]
mod tests {
    use crate::{CompressionConfig, compress as dispatch};

    use super::*;

    fn build_log_with_errors() -> String {
        let mut log = String::new();
        log.push_str("Compiling project v0.1.0\n");
        for i in 0..200 {
            log.push_str(&format!("  Compiling dep-{i} v1.0.0\n"));
        }
        log.push_str("error[E0308]: mismatched types\n");
        for i in 0..100 {
            log.push_str(&format!("  Processing file-{i}.rs\n"));
        }
        log.push_str("error: aborting due to previous error\n");
        for i in 0..100 {
            log.push_str(&format!("  Cleaning up temp-{i}\n"));
        }
        log.push_str("error: could not compile `project`\n");
        log
    }

    #[test]
    fn preserves_all_error_lines() {
        let config = CompressionConfig::default();
        let log = build_log_with_errors();
        let result = compress(&config, &log, Some("cargo build"));
        assert!(result.contains("error[E0308]"));
        assert!(result.contains("error: aborting"));
        assert!(result.contains("error: could not compile"));
    }

    #[test]
    fn preserves_success_indicators() {
        let config = CompressionConfig::default();
        let mut log = String::new();
        for i in 0..300 {
            log.push_str(&format!("  Running test {i}...\n"));
        }
        log.push_str("All 300 tests passed\n");
        log.push_str("Build successful\n");
        let result = compress(&config, &log, None);
        assert!(result.contains("All 300 tests passed"));
        assert!(result.contains("Build successful"));
    }

    #[test]
    fn preserves_warnings() {
        let config = CompressionConfig::default();
        let log = "info: lots of noise\nwarning: unused variable\ninfo: more noise\n";
        let result = compress(&config, log, None);
        assert!(result.contains("warning: unused variable"));
    }

    #[test]
    fn command_in_header() {
        let config = CompressionConfig::default();
        let result = compress(&config, "some output", Some("npm test"));
        assert!(result.contains("[Command: npm test]"));
    }

    #[test]
    fn through_dispatcher_budget_guarantee() {
        let config = CompressionConfig::default();
        let log = build_log_with_errors();
        assert!(log.chars().count() > config.compression_threshold);
        let result = dispatch(&config, "Bash", &log, Some("cargo build"));
        assert!(result.compression_applied);
        assert!(result.text.chars().count() <= config.target_budget);
    }

    #[test]
    fn empty_output_handled() {
        let config = CompressionConfig::default();
        let result = compress(&config, "", None);
        assert!(result.contains("0 lines total"));
    }
}
