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
// JSON output helper
// ---------------------------------------------------------------------------

pub fn print_json<T: Serialize>(data: &T) -> Result<(), CliError> {
    let json =
        serde_json::to_string_pretty(data).map_err(|e| CliError::Io(std::io::Error::other(e)))?;
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{json}").map_err(CliError::Io)
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
