//! Subcommand execution logic.

use rusty_brain_core::mind::Mind;
use types::ObservationType;

use crate::CliError;
use crate::output::{
    self, AskOutput, FindOutput, SearchResultJson, StatsOutput, TimelineEntryJson, TimelineOutput,
};

pub fn run_find(
    mind: &Mind,
    pattern: &str,
    limit: usize,
    type_filter: Option<ObservationType>,
    json: bool,
) -> Result<(), CliError> {
    if pattern.is_empty() {
        return Err(CliError::EmptyPattern);
    }

    let fetch_limit = if type_filter.is_some() {
        limit.saturating_mul(10)
    } else {
        limit
    };

    let results = mind.search(pattern, Some(fetch_limit))?;

    // Apply post-query type filter.
    let filtered: Vec<_> = match type_filter {
        Some(ref t) => results.iter().filter(|r| r.obs_type == *t).collect(),
        None => results.iter().collect(),
    };

    let output_results: Vec<SearchResultJson> =
        filtered.iter().take(limit).map(|r| (*r).into()).collect();
    let find_output = FindOutput {
        count: output_results.len(),
        results: output_results,
    };

    if json {
        output::print_json(&find_output)
    } else {
        output::print_find_human(&find_output);
        Ok(())
    }
}

pub fn run_ask(mind: &Mind, question: &str, json: bool) -> Result<(), CliError> {
    let answer = mind.ask(question)?;

    let ask_output = match answer {
        Some(text) => AskOutput {
            answer: text,
            has_results: true,
        },
        None => AskOutput {
            answer: "No relevant memories found for your question.".to_string(),
            has_results: false,
        },
    };

    if json {
        output::print_json(&ask_output)
    } else {
        output::print_ask_human(&ask_output);
        Ok(())
    }
}

pub fn run_stats(mind: &Mind, json: bool) -> Result<(), CliError> {
    let stats = mind.stats()?;
    let stats_output = StatsOutput::from(&stats);

    if json {
        output::print_json(&stats_output)
    } else {
        output::print_stats_human(&stats_output);
        Ok(())
    }
}

pub fn run_timeline(
    mind: &Mind,
    limit: usize,
    type_filter: Option<ObservationType>,
    oldest_first: bool,
    json: bool,
) -> Result<(), CliError> {
    // reverse=true for default newest-first; reverse=false for oldest-first
    let fetch_limit = if type_filter.is_some() {
        limit.saturating_mul(10)
    } else {
        limit
    };

    let entries = mind.timeline(fetch_limit, !oldest_first)?;

    // Apply post-query type filter.
    let filtered: Vec<_> = match type_filter {
        Some(ref t) => entries.iter().filter(|e| e.obs_type == *t).collect(),
        None => entries.iter().collect(),
    };

    let output_entries: Vec<TimelineEntryJson> =
        filtered.iter().take(limit).map(|e| (*e).into()).collect();
    let timeline_output = TimelineOutput {
        count: output_entries.len(),
        entries: output_entries,
    };

    if json {
        output::print_json(&timeline_output)
    } else {
        output::print_timeline_human(&timeline_output);
        Ok(())
    }
}
