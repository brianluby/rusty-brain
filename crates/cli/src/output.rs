//! Output formatting: JSON serialization and human-readable tables.

use std::collections::BTreeMap;
use std::io::{IsTerminal, Write};

use chrono::{DateTime, Utc};
use comfy_table::{ContentArrangement, Table};
use serde::Serialize;

use crate::CliError;
use rusty_brain_core::mind::{MemorySearchResult, TimelineEntry};
use types::MindStats;

// ---------------------------------------------------------------------------
// JSON output types (CLI-local mirror types for snake_case serialization)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct FindOutput {
    pub results: Vec<SearchResultJson>,
    pub count: usize,
}

#[derive(Debug, Serialize)]
pub struct SearchResultJson {
    pub obs_type: String,
    pub summary: String,
    pub content_excerpt: Option<String>,
    pub timestamp: String,
    pub score: f64,
    pub tool_name: String,
}

#[derive(Debug, Serialize)]
pub struct AskOutput {
    pub answer: String,
    pub has_results: bool,
}

#[derive(Debug, Serialize)]
pub struct StatsOutput {
    pub total_observations: u64,
    pub total_sessions: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oldest_memory: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub newest_memory: Option<String>,
    pub file_size_bytes: u64,
    pub type_counts: BTreeMap<String, u64>,
}

#[derive(Debug, Serialize)]
pub struct TimelineOutput {
    pub entries: Vec<TimelineEntryJson>,
    pub count: usize,
}

#[derive(Debug, Serialize)]
pub struct TimelineEntryJson {
    pub obs_type: String,
    pub summary: String,
    pub timestamp: String,
}

// ---------------------------------------------------------------------------
// From conversions (upstream → CLI output types)
// ---------------------------------------------------------------------------

impl From<&MemorySearchResult> for SearchResultJson {
    fn from(r: &MemorySearchResult) -> Self {
        Self {
            obs_type: r.obs_type.to_string(),
            summary: r.summary.clone(),
            content_excerpt: r.content_excerpt.clone(),
            timestamp: r.timestamp.to_rfc3339(),
            score: r.score,
            tool_name: r.tool_name.clone(),
        }
    }
}

impl From<&TimelineEntry> for TimelineEntryJson {
    fn from(e: &TimelineEntry) -> Self {
        Self {
            obs_type: e.obs_type.to_string(),
            summary: e.summary.clone(),
            timestamp: e.timestamp.to_rfc3339(),
        }
    }
}

impl From<&MindStats> for StatsOutput {
    fn from(s: &MindStats) -> Self {
        Self {
            total_observations: s.total_observations,
            total_sessions: s.total_sessions,
            oldest_memory: s.oldest_memory.map(|dt| dt.to_rfc3339()),
            newest_memory: s.newest_memory.map(|dt| dt.to_rfc3339()),
            file_size_bytes: s.file_size_bytes,
            type_counts: s
                .type_counts
                .iter()
                .map(|(k, v)| (k.to_string(), *v))
                .collect(),
        }
    }
}

// ---------------------------------------------------------------------------
// Structured error output (agent-friendly)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct ErrorOutput {
    pub code: &'static str,
    pub message: String,
}

// ---------------------------------------------------------------------------
// JSON output helper
// ---------------------------------------------------------------------------

pub fn print_json<T: Serialize>(data: &T) -> Result<(), CliError> {
    let json =
        serde_json::to_string_pretty(data).map_err(|e| CliError::Io(std::io::Error::other(e)))?;
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{json}").map_err(CliError::Io)
}

/// Print a structured JSON error to stderr for agent consumption.
pub fn print_error_json(error: &CliError) {
    let output = ErrorOutput {
        code: error.code(),
        message: error.to_string(),
    };
    if let Ok(json) = serde_json::to_string_pretty(&output) {
        let _ = writeln!(std::io::stderr().lock(), "{json}");
    }
}

// ---------------------------------------------------------------------------
// Human-readable output functions
// ---------------------------------------------------------------------------

pub fn print_find_human(output: &FindOutput) {
    if output.count == 0 {
        println!("No results found.");
        return;
    }

    let mut table = new_table();
    table.set_header(vec!["Type", "Score", "Timestamp", "Summary"]);

    for r in &output.results {
        let timestamp = format_timestamp(&r.timestamp);
        let summary = truncate(&r.summary, 60);
        table.add_row(vec![
            &r.obs_type,
            &format!("{:.2}", r.score),
            &timestamp,
            &summary,
        ]);
    }

    println!("{table}");
}

pub fn print_ask_human(output: &AskOutput) {
    if output.has_results {
        println!("{}", output.answer);
    } else {
        println!("No relevant memories found for your question.");
    }
}

pub fn print_stats_human(output: &StatsOutput) {
    println!("Memory Statistics");
    println!("  Observations: {}", output.total_observations);
    println!("  Sessions:     {}", output.total_sessions);

    if let Some(ref oldest) = output.oldest_memory {
        println!("  Oldest:       {}", format_timestamp(oldest));
    }
    if let Some(ref newest) = output.newest_memory {
        println!("  Newest:       {}", format_timestamp(newest));
    }

    println!("  File size:    {}", format_bytes(output.file_size_bytes));

    if !output.type_counts.is_empty() {
        println!();
        let mut table = new_table();
        table.set_header(vec!["Type", "Count"]);

        let mut sorted: Vec<_> = output.type_counts.iter().collect();
        sorted.sort_by(|a, b| b.1.cmp(a.1));

        for (obs_type, count) in sorted {
            table.add_row(vec![obs_type.as_str(), &count.to_string()]);
        }

        println!("{table}");
    }
}

pub fn print_timeline_human(output: &TimelineOutput) {
    if output.count == 0 {
        println!("No timeline entries.");
        return;
    }

    let mut table = new_table();
    table.set_header(vec!["Timestamp", "Type", "Summary"]);

    for e in &output.entries {
        let timestamp = format_timestamp(&e.timestamp);
        let summary = truncate(&e.summary, 60);
        table.add_row(vec![&timestamp, &e.obs_type, &summary]);
    }

    println!("{table}");
}

// ---------------------------------------------------------------------------
// Terminal detection
// ---------------------------------------------------------------------------

/// Create a table pre-configured for the current output context.
/// Uses dynamic content arrangement for terminals, disabled for pipes.
fn new_table() -> Table {
    let mut table = Table::new();
    if std::io::stdout().is_terminal() {
        table.set_content_arrangement(ContentArrangement::Dynamic);
    } else {
        table.set_content_arrangement(ContentArrangement::Disabled);
    }
    table
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let mut end = max_len;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &s[..end])
    }
}

fn format_timestamp(rfc3339: &str) -> String {
    DateTime::parse_from_rfc3339(rfc3339).map_or_else(
        |_| rfc3339.to_string(),
        |dt| {
            dt.with_timezone(&Utc)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        },
    )
}

#[allow(clippy::cast_precision_loss)]
fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, HashMap};

    use chrono::Utc;
    use rusty_brain_core::mind::{MemorySearchResult, TimelineEntry};
    use types::ObservationType;

    // -------------------------------------------------------------------------
    // T031: JSON and table output rendering tests
    // -------------------------------------------------------------------------

    // --- FindOutput JSON serialization ---

    #[test]
    fn find_output_json_empty_results() {
        let output = FindOutput {
            results: vec![],
            count: 0,
        };
        let json = serde_json::to_string(&output).expect("serialization must succeed");
        assert!(json.contains("\"count\":0"));
        assert!(json.contains("\"results\":[]"));
    }

    #[test]
    fn find_output_json_with_results() {
        let output = FindOutput {
            results: vec![SearchResultJson {
                obs_type: "discovery".to_string(),
                summary: "Found a bug".to_string(),
                content_excerpt: Some("excerpt here".to_string()),
                timestamp: "2025-01-01T00:00:00+00:00".to_string(),
                score: 0.95,
                tool_name: "Read".to_string(),
            }],
            count: 1,
        };
        let json = serde_json::to_string_pretty(&output).expect("serialization must succeed");
        assert!(json.contains("\"count\": 1"));
        assert!(json.contains("\"obs_type\": \"discovery\""));
        assert!(json.contains("\"score\": 0.95"));
        assert!(json.contains("\"tool_name\": \"Read\""));
    }

    // --- AskOutput JSON serialization ---

    #[test]
    fn ask_output_json_with_results() {
        let output = AskOutput {
            answer: "The answer is 42".to_string(),
            has_results: true,
        };
        let json = serde_json::to_string(&output).expect("serialization must succeed");
        assert!(json.contains("\"has_results\":true"));
        assert!(json.contains("The answer is 42"));
    }

    #[test]
    fn ask_output_json_without_results() {
        let output = AskOutput {
            answer: "No relevant memories found for your question.".to_string(),
            has_results: false,
        };
        let json = serde_json::to_string(&output).expect("serialization must succeed");
        assert!(json.contains("\"has_results\":false"));
    }

    // --- StatsOutput JSON serialization ---

    #[test]
    fn stats_output_json_full() {
        let mut type_counts = BTreeMap::new();
        type_counts.insert("discovery".to_string(), 5);
        type_counts.insert("bugfix".to_string(), 3);
        let output = StatsOutput {
            total_observations: 100,
            total_sessions: 10,
            oldest_memory: Some("2024-01-01T00:00:00+00:00".to_string()),
            newest_memory: Some("2025-06-15T12:00:00+00:00".to_string()),
            file_size_bytes: 1_048_576,
            type_counts,
        };
        let json = serde_json::to_string_pretty(&output).expect("serialization must succeed");
        assert!(json.contains("\"total_observations\": 100"));
        assert!(json.contains("\"total_sessions\": 10"));
        assert!(json.contains("\"oldest_memory\""));
        assert!(json.contains("\"newest_memory\""));
    }

    #[test]
    fn stats_output_json_omits_none_timestamps() {
        let output = StatsOutput {
            total_observations: 0,
            total_sessions: 0,
            oldest_memory: None,
            newest_memory: None,
            file_size_bytes: 0,
            type_counts: BTreeMap::new(),
        };
        let json = serde_json::to_string(&output).expect("serialization must succeed");
        assert!(
            !json.contains("oldest_memory"),
            "None timestamps must be omitted"
        );
        assert!(
            !json.contains("newest_memory"),
            "None timestamps must be omitted"
        );
    }

    // --- TimelineOutput JSON serialization ---

    #[test]
    fn timeline_output_json_empty() {
        let output = TimelineOutput {
            entries: vec![],
            count: 0,
        };
        let json = serde_json::to_string(&output).expect("serialization must succeed");
        assert!(json.contains("\"count\":0"));
        assert!(json.contains("\"entries\":[]"));
    }

    #[test]
    fn timeline_output_json_with_entries() {
        let output = TimelineOutput {
            entries: vec![TimelineEntryJson {
                obs_type: "decision".to_string(),
                summary: "Chose Rust".to_string(),
                timestamp: "2025-03-01T10:30:00+00:00".to_string(),
            }],
            count: 1,
        };
        let json = serde_json::to_string_pretty(&output).expect("serialization must succeed");
        assert!(json.contains("\"obs_type\": \"decision\""));
        assert!(json.contains("Chose Rust"));
    }

    // --- ErrorOutput JSON serialization ---

    #[test]
    fn error_output_json_serialization() {
        let output = ErrorOutput {
            code: "E_CLI_EMPTY_PATTERN",
            message: "Search pattern must not be empty.".to_string(),
        };
        let json = serde_json::to_string_pretty(&output).expect("serialization must succeed");
        assert!(json.contains("\"code\": \"E_CLI_EMPTY_PATTERN\""));
        assert!(json.contains("Search pattern must not be empty."));
    }

    // --- From conversions ---

    #[test]
    fn search_result_json_from_memory_search_result() {
        let now = Utc::now();
        let msr = MemorySearchResult {
            obs_type: ObservationType::Discovery,
            summary: "test summary".to_string(),
            content_excerpt: Some("excerpt".to_string()),
            timestamp: now,
            score: 0.85,
            tool_name: "Bash".to_string(),
        };
        let json_result = SearchResultJson::from(&msr);
        assert_eq!(json_result.obs_type, "discovery");
        assert_eq!(json_result.summary, "test summary");
        assert_eq!(json_result.content_excerpt, Some("excerpt".to_string()));
        assert!((json_result.score - 0.85).abs() < f64::EPSILON);
        assert_eq!(json_result.tool_name, "Bash");
        // Timestamp should be RFC3339
        assert!(json_result.timestamp.contains('T'));
    }

    #[test]
    fn search_result_json_from_no_excerpt() {
        let msr = MemorySearchResult {
            obs_type: ObservationType::Bugfix,
            summary: "fixed it".to_string(),
            content_excerpt: None,
            timestamp: Utc::now(),
            score: 1.0,
            tool_name: "Read".to_string(),
        };
        let json_result = SearchResultJson::from(&msr);
        assert!(json_result.content_excerpt.is_none());
    }

    #[test]
    fn timeline_entry_json_from_timeline_entry() {
        let now = Utc::now();
        let entry = TimelineEntry {
            obs_type: ObservationType::Pattern,
            summary: "pattern found".to_string(),
            timestamp: now,
            tool_name: "Grep".to_string(),
        };
        let json_entry = TimelineEntryJson::from(&entry);
        assert_eq!(json_entry.obs_type, "pattern");
        assert_eq!(json_entry.summary, "pattern found");
        assert!(json_entry.timestamp.contains('T'));
    }

    #[test]
    fn stats_output_from_mind_stats() {
        let now = Utc::now();
        let mut type_counts = HashMap::new();
        type_counts.insert(ObservationType::Discovery, 5_u64);
        type_counts.insert(ObservationType::Warning, 2_u64);
        let stats = MindStats {
            total_observations: 7,
            total_sessions: 3,
            oldest_memory: Some(now),
            newest_memory: Some(now),
            file_size_bytes: 2048,
            type_counts,
        };
        let output = StatsOutput::from(&stats);
        assert_eq!(output.total_observations, 7);
        assert_eq!(output.total_sessions, 3);
        assert!(output.oldest_memory.is_some());
        assert!(output.newest_memory.is_some());
        assert_eq!(output.file_size_bytes, 2048);
        assert_eq!(output.type_counts.len(), 2);
        assert_eq!(output.type_counts.get("discovery"), Some(&5));
        assert_eq!(output.type_counts.get("warning"), Some(&2));
    }

    #[test]
    fn stats_output_from_empty_mind_stats() {
        let stats = MindStats {
            total_observations: 0,
            total_sessions: 0,
            oldest_memory: None,
            newest_memory: None,
            file_size_bytes: 0,
            type_counts: HashMap::new(),
        };
        let output = StatsOutput::from(&stats);
        assert_eq!(output.total_observations, 0);
        assert!(output.oldest_memory.is_none());
        assert!(output.newest_memory.is_none());
        assert!(output.type_counts.is_empty());
    }

    // --- Helper function tests ---

    #[test]
    fn truncate_short_string_unchanged() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_exact_length_unchanged() {
        assert_eq!(truncate("12345", 5), "12345");
    }

    #[test]
    fn truncate_long_string_adds_ellipsis() {
        let result = truncate("hello world", 5);
        assert_eq!(result, "hello...");
    }

    #[test]
    fn truncate_empty_string() {
        assert_eq!(truncate("", 10), "");
    }

    #[test]
    fn truncate_unicode_respects_char_boundary() {
        // Multi-byte character: each char is 4 bytes
        let input = "\u{1F600}\u{1F600}\u{1F600}"; // 3 emoji
        let result = truncate(input, 2);
        // Should not panic on char boundary issues
        assert!(result.ends_with("..."));
    }

    #[test]
    fn format_timestamp_valid_rfc3339() {
        let result = format_timestamp("2025-01-15T10:30:00+00:00");
        assert_eq!(result, "2025-01-15 10:30:00");
    }

    #[test]
    fn format_timestamp_invalid_returns_original() {
        let result = format_timestamp("not-a-timestamp");
        assert_eq!(result, "not-a-timestamp");
    }

    #[test]
    fn format_timestamp_with_timezone_offset() {
        let result = format_timestamp("2025-06-15T14:00:00+05:30");
        // Should convert to UTC
        assert_eq!(result, "2025-06-15 08:30:00");
    }

    #[test]
    fn format_bytes_bytes() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1023), "1023 B");
    }

    #[test]
    fn format_bytes_kilobytes() {
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(1536), "1.5 KB");
    }

    #[test]
    fn format_bytes_megabytes() {
        assert_eq!(format_bytes(1_048_576), "1.0 MB");
        assert_eq!(format_bytes(10_485_760), "10.0 MB");
    }

    #[test]
    fn format_bytes_gigabytes() {
        assert_eq!(format_bytes(1_073_741_824), "1.0 GB");
    }
}
