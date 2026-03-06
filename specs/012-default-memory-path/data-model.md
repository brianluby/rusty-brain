# Data Model: Default Memory Path Change

**Feature**: 012-default-memory-path
**Date**: 2026-03-05

## Entities

### Memory Directory Layout

The `.rusty-brain/` directory is the self-contained data directory for rusty-brain at the repository root.

```
.rusty-brain/
├── mind.mv2              # memvid-encoded memory database
├── .dedup-cache.json     # hash-based dedup cache (post-tool-use)
├── .dedup-cache.json.lock # file lock for dedup cache
└── .install-version      # binary version marker for smart-install
```

### Path Resolution Order

| Priority | Source | Path Pattern | Condition |
|----------|--------|-------------|-----------|
| 1 | `MEMVID_PLATFORM_MEMORY_PATH` env var | User-defined | Always wins when set |
| 2 | Platform opt-in | `.{platform}/mind-{platform}.mv2` | `MEMVID_PLATFORM_PATH_OPT_IN=1` |
| 3 | New canonical default | `.rusty-brain/mind.mv2` | Default (no override) |
| 4 | Legacy fallback | `.agent-brain/mind.mv2` | Used if exists AND `.rusty-brain/mind.mv2` does NOT exist |
| 5 | Oldest legacy (detect only) | `.claude/mind.mv2` | Detection/warning only — never used as runtime fallback |

### Legacy Detection Matrix

| `.claude/` exists | `.agent-brain/` exists | `.rusty-brain/` exists | Action |
|---|---|---|---|
| No | No | No | Create `.rusty-brain/mind.mv2` on first write |
| No | No | Yes | Use `.rusty-brain/mind.mv2` |
| No | Yes | No | Use `.agent-brain/mind.mv2` + suggest migration to `.rusty-brain/` |
| No | Yes | Yes | Use `.rusty-brain/mind.mv2` + warn about duplicate at `.agent-brain/` |
| Yes | No | No | Suggest migration from `.claude/` to `.rusty-brain/` |
| Yes | No | Yes | Use `.rusty-brain/mind.mv2` + warn about `.claude/` |
| Yes | Yes | No | Use `.agent-brain/mind.mv2` + suggest migration to `.rusty-brain/` + warn about `.claude/` |
| Yes | Yes | Yes | Use `.rusty-brain/mind.mv2` + warn about both `.agent-brain/` and `.claude/` |

### Constants

| Constant | Old Value | New Value | Location |
|----------|-----------|-----------|----------|
| `DEFAULT_MEMORY_DIR` (new) | N/A | `.rusty-brain` | `crates/platforms/src/path_policy.rs` |
| `DEFAULT_LEGACY_PATH` | `.agent-brain/mind.mv2` | `.rusty-brain/mind.mv2` | `crates/platforms/src/path_policy.rs` |
| `LEGACY_AGENT_BRAIN_PATH` (new) | N/A | `.agent-brain/mind.mv2` | `crates/platforms/src/path_policy.rs` |
| `LEGACY_CLAUDE_MEMORY_PATH` | `.claude/mind.mv2` | `.claude/mind.mv2` (unchanged) | `crates/platforms/src/path_policy.rs` |
| `MindConfig::default().memory_path` | `.agent-brain/mind.mv2` | `.rusty-brain/mind.mv2` | `crates/types/src/config.rs` |

### Diagnostic Messages

| Scenario | Level | Message Template |
|----------|-------|-----------------|
| `.agent-brain/` only (no `.rusty-brain/`) | Info | "Using legacy memory file at `.agent-brain/mind.mv2`. Migrate to `.rusty-brain/mind.mv2`: `mv .agent-brain .rusty-brain`" |
| `.agent-brain/` + `.rusty-brain/` both exist | Warning | "Duplicate memory files: using `.rusty-brain/mind.mv2`. Consider removing `.agent-brain/`." |
| `.claude/` exists (any combo) | Warning | "Legacy memory file at `.claude/mind.mv2`. Migrate to `.rusty-brain/mind.mv2`." |

## State Transitions

```
[No data directory] --first write--> [.rusty-brain/ created]
[.agent-brain/ only] --user runs `mv`--> [.rusty-brain/ only]
[.agent-brain/ + .rusty-brain/] --user removes old--> [.rusty-brain/ only]
```

No automatic state transitions occur — all migrations are user-initiated (FR-008).

## Validation Rules

- Memory path must stay within project directory (existing path traversal check)
- Custom `MEMVID_PLATFORM_MEMORY_PATH` skips all legacy detection
- Platform opt-in (`MEMVID_PLATFORM_PATH_OPT_IN=1`) skips legacy detection
