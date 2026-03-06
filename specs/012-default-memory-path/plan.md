# Implementation Plan: Default Memory Path Change

**Branch**: `012-default-memory-path` | **Date**: 2026-03-05 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/specs/012-default-memory-path/spec.md`

## Summary

Change the default memory directory from `.agent-brain/` to `.rusty-brain/` across the entire codebase. This involves updating the default path constant, extending legacy path detection to a three-tier chain (`.claude/` → `.agent-brain/` → `.rusty-brain/`), adding runtime fallback to `.agent-brain/` for existing installations, moving supporting files (dedup cache, version marker) into the `.rusty-brain/` directory, and updating all documentation references.

## Technical Context

**Language/Version**: Rust stable, edition 2024, MSRV 1.85.0
**Primary Dependencies**: serde, serde_json, fs2, chrono (all existing workspace deps)
**Storage**: `.mv2` files on local filesystem, `.dedup-cache.json`, `.install-version`
**Testing**: `cargo test` (unit + integration), `cargo clippy -- -D warnings`, `cargo fmt --check`
**Target Platform**: macOS, Linux (cross-platform via Rust std)
**Project Type**: Rust workspace (multi-crate)
**Performance Goals**: N/A — path resolution is not performance-critical
**Constraints**: Zero data loss for existing users, no automatic file migration
**Scale/Scope**: ~16 source files affected, ~6 test files, documentation files

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | Notes |
|-----------|--------|-------|
| I. Crate-First | PASS | All changes within existing crate layout (`types`, `platforms`, `hooks`, `opencode`) |
| II. Rust-First | PASS | Stable Rust only, no `unsafe` |
| III. Agent-Friendly | PASS | Diagnostic messages are structured; migration instructions are machine-parseable |
| IV. Contract-First | PASS | Contracts defined in `contracts/` before implementation |
| V. Test-First | PASS | Tests must be updated before/alongside implementation |
| VI. Complete Delivery | PASS | All FR-001 through FR-010 mapped to implementation tasks |
| VII. Memory Integrity | PASS | No changes to memory file format; atomic writes preserved; fallback ensures no data loss |
| VIII. Performance | PASS | Not in scope; no speculative benchmark work |
| IX. Security-First | PASS | Path traversal checks preserved; no memory content logging changes |
| X. Error Handling | PASS | Diagnostic messages include stable error context; migration instructions are actionable |
| XI. Observability | PASS | Legacy detection produces structured diagnostics |
| XII. Simplicity | PASS | Minimal changes to existing patterns; extends existing detection, doesn't introduce new frameworks |
| XIII. Dependency Policy | PASS | No new dependencies |

**Post-Phase 1 re-check**: All gates still pass. `detect_legacy_path` signature changes from `Option<Diagnostic>` to `Vec<Diagnostic>` but this is a non-breaking internal API change.

## Project Structure

### Documentation (this feature)

```text
specs/012-default-memory-path/
├── plan.md              # This file
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
├── quickstart.md        # Phase 1 output
├── contracts/           # Phase 1 output
│   ├── path-policy-api.rs
│   ├── bootstrap-api.rs
│   └── dedup-api.rs
└── tasks.md             # Phase 2 output (/speckit.tasks)
```

### Source Code (affected files)

```text
crates/
├── types/
│   ├── src/config.rs          # MindConfig::default() path constant
│   └── tests/config_test.rs   # Default path assertions
├── platforms/
│   ├── src/path_policy.rs     # DEFAULT_MEMORY_PATH, LEGACY_CLAUDE_MEMORY_PATH, resolve_memory_path()
│   └── src/bootstrap.rs       # detect_legacy_paths(), build_mind_config()
├── hooks/
│   ├── src/dedup.rs           # DedupCache::new() hardcoded path
│   ├── src/smart_install.rs   # VERSION_FILENAME location
│   ├── src/context.rs         # Test path references
│   ├── tests/legacy_path_test.rs
│   ├── tests/dedup_test.rs
│   ├── tests/env_compat_test.rs
│   ├── tests/layout_compat_test.rs
│   └── tests/permissions_test.rs
├── opencode/
│   ├── tests/chat_hook_test.rs
│   └── tests/env_compat_test.rs
└── cli/                        # (verify no hardcoded paths)

CLAUDE.md                       # Documentation references
```

**Structure Decision**: Existing multi-crate workspace. No new crates or modules needed. All changes are modifications to existing files.

## Complexity Tracking

No constitution violations to justify.

## Key Design Decisions

### 1. Runtime Fallback in bootstrap.rs (R-004)

`build_mind_config()` gains a filesystem check: if `.rusty-brain/mind.mv2` doesn't exist but `.agent-brain/mind.mv2` does, use the legacy path. This implements FR-004 (graceful fallback) without auto-migration (FR-008).

### 2. detect_legacy_path → detect_legacy_paths (R-002)

Return type changes from `Option<Diagnostic>` to `Vec<Diagnostic>` to handle multiple legacy paths simultaneously. The function now checks both `.agent-brain/` and `.claude/` against the new canonical `.rusty-brain/`.

### 3. Supporting Files Follow Memory Directory (R-003)

- `DedupCache::new()` changes from `.agent-brain/.dedup-cache.json` to `.rusty-brain/.dedup-cache.json`
- `smart_install.rs` changes from `cwd/.install-version` to `cwd/.rusty-brain/.install-version`
- The dedup cache path should ideally derive from the resolved memory path's parent directory rather than hardcoding, for consistency when custom paths are used

### 4. Constant Naming (R-001, R-006)

- `DEFAULT_LEGACY_PATH` renamed to `DEFAULT_MEMORY_PATH` (it's no longer "legacy")
- New `LEGACY_AGENT_BRAIN_PATH` constant for the old default
- `LEGACY_CLAUDE_MEMORY_PATH` unchanged
- `DEFAULT_MEMORY_DIR` added for directory-level references (dedup cache, install version)

## Implementation Phases

### Phase A: Core Constants & Config (no filesystem behavior change)
1. Update `MindConfig::default()` path to `.rusty-brain/mind.mv2`
2. Update `path_policy.rs` constants
3. Update all unit tests for new default values

### Phase B: Legacy Detection & Fallback
1. Extend `detect_legacy_path` to three-tier chain
2. Add `resolve_effective_path` fallback in `bootstrap.rs`
3. Wire fallback into `build_mind_config`
4. Update integration tests

### Phase C: Supporting Files
1. Update `DedupCache::new()` path
2. Update `smart_install.rs` version file location
3. Update related tests

### Phase D: Documentation & Cleanup
1. Update `CLAUDE.md` references
2. Update any skill files
3. Final test pass: `cargo test && cargo clippy -- -D warnings && cargo fmt --check`
