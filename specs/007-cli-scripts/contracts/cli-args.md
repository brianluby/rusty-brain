# Contract: CLI Argument Schema

**Date**: 2026-03-02 | **Source**: AR §Interface Definitions, PRD §Interface Contract

## Binary

Name: `rusty-brain`
Crate: `crates/cli` (binary target)

## Global Options

```
rusty-brain [OPTIONS] <SUBCOMMAND>

OPTIONS:
    --memory-path <PATH>    Path to .mv2 memory file (overrides auto-detection)
    -v, --verbose           Enable DEBUG-level tracing output to stderr
    -h, --help              Print help information
    -V, --version           Print version information
```

## Subcommands

### find

```
rusty-brain find [OPTIONS] <PATTERN>

ARGS:
    <PATTERN>    Text pattern to search for in memories (required, non-empty)

OPTIONS:
    --limit <N>         Maximum number of results [default: 10, range: 1..]
    --type <TYPE>       Filter by observation type (case-insensitive)
    --json              Output results as JSON
    -h, --help          Print help information
```

### ask

```
rusty-brain ask [OPTIONS] <QUESTION>

ARGS:
    <QUESTION>    Natural language question about memory (required, non-empty)

OPTIONS:
    --json              Output answer as JSON
    -h, --help          Print help information
```

### stats

```
rusty-brain stats [OPTIONS]

OPTIONS:
    --json              Output statistics as JSON
    -h, --help          Print help information
```

### timeline

```
rusty-brain timeline [OPTIONS]

OPTIONS:
    --limit <N>         Maximum number of entries [default: 10, range: 1..]
    --type <TYPE>       Filter by observation type (case-insensitive)
    --oldest-first      Show oldest entries first (default: most recent first)
    --json              Output entries as JSON
    -h, --help          Print help information
```

## Clap Derive Definition

```rust
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use types::ObservationType;

#[derive(Parser)]
#[command(
    name = "rusty-brain",
    about = "Query your AI agent's memory",
    version,
    arg_required_else_help = true,
)]
pub struct Cli {
    /// Path to memory file (overrides auto-detection)
    #[arg(long, global = true)]
    pub memory_path: Option<PathBuf>,

    /// Enable verbose debug output to stderr
    #[arg(short, long, global = true)]
    pub verbose: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Search memories by text pattern
    Find {
        /// Search pattern
        pattern: String,
        /// Maximum results
        #[arg(long, default_value_t = 10, value_parser = clap::value_parser!(usize).range(1..))]
        limit: usize,
        /// Filter by observation type
        #[arg(long, value_parser = parse_obs_type)]
        r#type: Option<ObservationType>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Ask a question about your memory
    Ask {
        /// Natural language question
        question: String,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// View memory statistics
    Stats {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// View chronological timeline
    Timeline {
        /// Maximum entries
        #[arg(long, default_value_t = 10, value_parser = clap::value_parser!(usize).range(1..))]
        limit: usize,
        /// Filter by observation type
        #[arg(long, value_parser = parse_obs_type)]
        r#type: Option<ObservationType>,
        /// Show oldest entries first
        #[arg(long)]
        oldest_first: bool,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
}

/// Parse observation type from string (case-insensitive).
/// On error, lists all valid type names.
fn parse_obs_type(s: &str) -> Result<ObservationType, String> {
    s.parse::<ObservationType>().map_err(|_| {
        format!(
            "invalid observation type '{}'; valid types: discovery, decision, problem, \
             solution, pattern, warning, success, refactor, bugfix, feature",
            s
        )
    })
}
```

## Exit Codes

| Code | Meaning | Trigger |
|------|---------|---------|
| 0 | Success | All subcommands on success |
| 1 | General error | Invalid arguments, missing file, corrupted memory, API error |
| 2 | Lock timeout | Memory file locked after 5 retries with exponential backoff |

## Validation Constraints

| Input | Constraint | Enforced by |
|-------|-----------|-------------|
| `<PATTERN>` | Non-empty string | clap required positional arg |
| `<QUESTION>` | Non-empty string | clap required positional arg |
| `--limit` | Positive integer (>=1) | `value_parser!(usize).range(1..)` |
| `--type` | Valid ObservationType variant | `parse_obs_type()` |
| `--memory-path` | Existing file (not directory) | Pre-Mind::open() validation in main.rs |
