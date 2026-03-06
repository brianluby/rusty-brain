# Research: Default Memory Path Change

**Feature**: 012-default-memory-path
**Date**: 2026-03-05

## R-001: Current Path Resolution Architecture

**Decision**: The path resolution system has three layers that all need updating:

1. **`crates/types/src/config.rs`** — `MindConfig::default()` hardcodes `.agent-brain/mind.mv2`
2. **`crates/platforms/src/path_policy.rs`** — `DEFAULT_LEGACY_PATH` constant and `LEGACY_CLAUDE_MEMORY_PATH` constant; `resolve_memory_path()` returns the default path in `LegacyFirst` mode
3. **`crates/platforms/src/bootstrap.rs`** — `detect_legacy_path()` checks `.claude/mind.mv2` against `.agent-brain/mind.mv2`; `build_mind_config()` orchestrates resolution

**Rationale**: These three files form the complete path resolution chain. Changing the default requires updating all three in a coordinated manner.

**Alternatives considered**: Single-constant approach (define once, import everywhere) — rejected because the path policy module intentionally separates resolution concerns from configuration defaults.

## R-002: Legacy Path Detection Chain Design

**Decision**: Extend from two-tier (`.claude/` → `.agent-brain/`) to three-tier (`.claude/` → `.agent-brain/` → `.rusty-brain/`) with `.rusty-brain/` as canonical.

**Current behavior** (`detect_legacy_path` in `crates/platforms/src/bootstrap.rs`):
- Checks `.claude/mind.mv2` (legacy) vs `.agent-brain/mind.mv2` (canonical)
- Returns migration warning if legacy exists without canonical
- Returns duplicate warning if both exist

**New behavior**:
- `.rusty-brain/mind.mv2` becomes canonical
- `.agent-brain/mind.mv2` becomes a legacy path (middle tier)
- `.claude/mind.mv2` remains the oldest legacy path
- Detection must check all three paths and produce appropriate diagnostics
- When `.agent-brain/` exists but `.rusty-brain/` doesn't, use `.agent-brain/` (FR-004 graceful fallback)

**Rationale**: Preserves backward compatibility while guiding users to the new canonical path. FR-008 mandates no automatic file movement.

**Alternatives considered**: Auto-migration on first run — rejected per FR-008 (migration is user-initiated).

## R-003: Supporting File Locations

**Decision**: Supporting files (`DedupCache`, `.install-version`) use the same directory as the memory file.

**Current locations**:
- `DedupCache::new()` in `crates/hooks/src/dedup.rs` hardcodes `project_dir.join(".agent-brain").join(CACHE_FILENAME)`
- `smart_install.rs` writes `.install-version` to `cwd` (project root), NOT inside `.agent-brain/`

**New locations**:
- `DedupCache` → `.rusty-brain/.dedup-cache.json` (update hardcoded path)
- `.install-version` → `.rusty-brain/.install-version` (move inside the data directory per FR-006)

**Rationale**: FR-006 requires all supporting files in `.rusty-brain/`. Self-contained directory structure aids discoverability.

**Alternatives considered**: Keep `.install-version` at project root — rejected because FR-006 explicitly requires all files in `.rusty-brain/`.

## R-004: Fallback Resolution Order

**Decision**: Path resolution priority (highest to lowest):
1. `MEMVID_PLATFORM_MEMORY_PATH` environment variable (custom override)
2. Platform opt-in path (`.{platform}/mind-{platform}.mv2`) when `MEMVID_PLATFORM_PATH_OPT_IN=1`
3. `.rusty-brain/mind.mv2` (new canonical default)
4. `.agent-brain/mind.mv2` (legacy fallback, used if exists and `.rusty-brain/` doesn't)
5. `.claude/mind.mv2` (oldest legacy, detection only — never used as fallback)

**Rationale**: FR-007 preserves custom path behavior. FR-004 requires graceful fallback to `.agent-brain/`. FR-005 requires `.rusty-brain/` preference when both exist.

**Alternatives considered**: No runtime fallback (just change the default and let users deal with it) — rejected because FR-004 mandates using existing `.agent-brain/` data.

## R-005: Documentation and Skill File Updates

**Decision**: All references to `.agent-brain/` in documentation, CLAUDE.md, skill definitions, and configuration templates must be updated to `.rusty-brain/` (FR-010).

**Files requiring updates** (non-Rust):
- `CLAUDE.md` (project instructions)
- Any skill files referencing `.agent-brain/`
- Configuration templates

**Rationale**: FR-010 explicitly requires this. Users and agents reading documentation should see the current canonical path.

## R-006: PathMode Enum Extension

**Decision**: The `PathMode` enum in `path_policy.rs` needs no new variant. The existing `LegacyFirst` mode changes its default path from `.agent-brain/` to `.rusty-brain/`. A new runtime fallback mechanism handles the `.agent-brain/` → `.rusty-brain/` migration.

**Rationale**: `LegacyFirst` semantically means "use the non-platform-scoped default" — changing what that default points to doesn't change the mode's meaning. The fallback logic belongs in `bootstrap.rs` where filesystem checks already happen.

**Alternatives considered**: Adding a `MigrationFallback` variant — rejected as over-engineering; the fallback is a bootstrap concern, not a path-policy concern.
