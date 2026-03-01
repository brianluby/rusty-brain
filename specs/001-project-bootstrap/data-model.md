# Data Model: Project Bootstrap

**Feature**: 001-project-bootstrap
**Date**: 2026-03-01

## Overview

Phase 0 is a scaffolding feature with no runtime data entities. The "data model" for this phase describes the configuration structures that define the workspace, not runtime objects.

## Entities

### Workspace Configuration

| Field | Type | Description |
|-------|------|-------------|
| members | `Vec<String>` | Glob pattern(s) for crate discovery (`crates/*`) |
| resolver | `String` | Dependency resolver version (`"3"`) |

### Workspace Package Metadata

| Field | Type | Description |
|-------|------|-------------|
| edition | `String` | Rust edition (`"2024"`) |
| rust-version | `String` | Minimum supported Rust version (`"1.85.0"`) |
| license | `String` | Project license |
| repository | `String` | Git repository URL |
| authors | `Vec<String>` | Project authors |

### Crate Definition

| Field | Type | Description |
|-------|------|-------------|
| name | `String` | Crate package name |
| type | `Enum(Library, Binary)` | Determines `lib.rs` vs `main.rs` |
| description | `String` | Purpose of the crate |

**Instances**:

| Crate | Type | Description |
|-------|------|-------------|
| `core` | Library | Memory engine (Mind) |
| `types` | Library | Shared types and errors |
| `platforms` | Library | Platform adapter system |
| `compression` | Library | Tool-output compression |
| `hooks` | Binary | Claude Code hook binaries |
| `cli` | Binary | CLI scripts (find, ask, stats, timeline) |
| `opencode` | Library | OpenCode editor adapter |

### Workspace Dependency

| Field | Type | Description |
|-------|------|-------------|
| name | `String` | Crate name as published or git reference |
| version | `String` | Version constraint or git rev |
| source | `Enum(CratesIo, Git)` | Where to fetch from |
| features | `Vec<String>` | Enabled features (optional) |

**Instances**:

| Dependency | Source | Version/Pin | Notes |
|------------|--------|-------------|-------|
| `memvid-core` | Git (`brianluby/memvid`) | rev `fbddef4...` | Core storage engine |
| `serde` | crates.io | `1.0` | With `derive` feature |
| `serde_json` | crates.io | `1.0` | JSON serialization |
| `thiserror` | crates.io | `2.0` | Error derive macros |
| `tokio` | crates.io | `1` | With `full` feature |
| `tracing` | crates.io | `0.1` | Structured logging |
| `chrono` | crates.io | `0.4` | With `serde` feature |
| `uuid` | crates.io | `1` | With `v4`, `serde` features |
| `clap` | crates.io | `4` | With `derive` feature |
| `semver` | crates.io | `1` | Version parsing |

## Relationships

```
Workspace 1──* Crate
Workspace 1──* WorkspaceDependency
Crate *──* WorkspaceDependency (via workspace inheritance)
```

## State Transitions

N/A — Phase 0 has no runtime state.
