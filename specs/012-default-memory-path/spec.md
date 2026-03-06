# Feature Specification: Default Memory Path Change

**Feature Branch**: `012-default-memory-path`
**Created**: 2026-03-05
**Status**: Draft
**Input**: User description: "Default install for the mind.mv2 file should be repo root .rusty-brain/mind.mv2"

## User Scenarios & Testing *(mandatory)*

### User Story 1 - New Installation Uses .rusty-brain Directory (Priority: P1)

A developer installs rusty-brain for the first time in a project. When the system initializes, it creates the memory file at `.rusty-brain/mind.mv2` in the repository root. The directory name matches the product name, making it immediately clear what the directory belongs to.

**Why this priority**: New installations are the most common path and must use the correct default from day one. The directory name should match the product identity for discoverability and clarity.

**Independent Test**: Can be fully tested by initializing rusty-brain in a fresh project directory and verifying the memory file is created at `.rusty-brain/mind.mv2`.

**Acceptance Scenarios**:

1. **Given** a project with no existing rusty-brain data, **When** the system initializes for the first time, **Then** the memory file is created at `<repo-root>/.rusty-brain/mind.mv2`.
2. **Given** a fresh installation, **When** the user looks for rusty-brain data in the project, **Then** the `.rusty-brain/` directory is present at the repository root with the `mind.mv2` file inside.
3. **Given** the `.rusty-brain/` directory does not exist, **When** the system initializes, **Then** it creates the directory with appropriate permissions before writing the memory file.

---

### User Story 2 - Migration from .agent-brain Directory (Priority: P1)

A developer who previously used rusty-brain with the old `.agent-brain/` directory updates to a new version. The system detects the existing `.agent-brain/mind.mv2` file and suggests migration to `.rusty-brain/mind.mv2`. Existing memories are preserved and accessible throughout the migration.

**Why this priority**: Existing users must not lose their data. A smooth migration path is essential for adoption of the new directory convention.

**Independent Test**: Can be fully tested by creating an `.agent-brain/mind.mv2` file and verifying the system detects it and provides migration guidance.

**Acceptance Scenarios**:

1. **Given** a project with `.agent-brain/mind.mv2` but no `.rusty-brain/mind.mv2`, **When** the system starts, **Then** it uses the existing `.agent-brain/mind.mv2` file and displays a migration suggestion to move to `.rusty-brain/mind.mv2`.
2. **Given** both `.agent-brain/mind.mv2` and `.rusty-brain/mind.mv2` exist, **When** the system starts, **Then** it uses `.rusty-brain/mind.mv2` (the new default) and warns about the duplicate file at the old location.
3. **Given** a project with only `.agent-brain/mind.mv2`, **When** the user follows the migration suggestion, **Then** they can move the file and the system works seamlessly with the new path.

---

### User Story 3 - Legacy .claude Path Detection (Priority: P2)

A developer who used an even older version of the system with `.claude/mind.mv2` receives guidance to migrate to the new `.rusty-brain/mind.mv2` path. The existing two-tier legacy detection (`.claude/` → `.agent-brain/`) is updated to a three-tier chain (`.claude/` → `.agent-brain/` → `.rusty-brain/`).

**Why this priority**: Users on the oldest legacy path need a clear migration target. The detection chain must be updated to point to the new canonical location.

**Independent Test**: Can be tested by creating a `.claude/mind.mv2` file and verifying the migration suggestion now points to `.rusty-brain/mind.mv2`.

**Acceptance Scenarios**:

1. **Given** a project with only `.claude/mind.mv2`, **When** the system starts, **Then** it suggests migration to `.rusty-brain/mind.mv2` (not `.agent-brain/mind.mv2`).
2. **Given** all three paths exist (`.claude/`, `.agent-brain/`, `.rusty-brain/`), **When** the system starts, **Then** it uses `.rusty-brain/mind.mv2` and warns about the other locations.

---

### User Story 4 - Supporting Files in .rusty-brain Directory (Priority: P1)

A developer using rusty-brain expects all supporting files (deduplication cache, install version marker) to live alongside the memory file in the `.rusty-brain/` directory. The directory structure is consistent and self-contained.

**Why this priority**: All data files must move together to maintain a consistent directory structure. A partial migration (memory file in one place, cache in another) would cause confusion.

**Independent Test**: Can be tested by verifying all supporting files are created within `.rusty-brain/` on a fresh installation.

**Acceptance Scenarios**:

1. **Given** a fresh installation, **When** the system creates supporting files, **Then** the deduplication cache is at `.rusty-brain/.dedup-cache.json` and the version marker is at `.rusty-brain/.install-version`.
2. **Given** a project with files in `.agent-brain/`, **When** the migration suggestion is displayed, **Then** it covers all files in the directory (not just `mind.mv2`).

---

### Edge Cases

- What happens when the user has a custom `memory_path` configured via environment variable?
  - Custom paths override the default. The change only affects the default path when no override is set.
- What happens when `.rusty-brain/` already exists but contains non-rusty-brain files?
  - The system creates `mind.mv2` alongside existing files without disturbing them.
- What happens when the user has both old and new directories with different memory content?
  - The system uses `.rusty-brain/mind.mv2` as canonical and warns about the duplicate, leaving the old file untouched for the user to reconcile.
- What happens when file system permissions prevent creating `.rusty-brain/`?
  - The system reports a clear error about directory creation failure with the path that could not be created.
- What happens when `.gitignore` includes `.agent-brain/` but not `.rusty-brain/`?
  - This is the user's responsibility. Documentation should mention updating `.gitignore` as part of migration steps.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST use `.rusty-brain/mind.mv2` (relative to repository root) as the default memory file path for new installations.
- **FR-002**: System MUST detect `.agent-brain/mind.mv2` as a legacy path and suggest migration to `.rusty-brain/mind.mv2`.
- **FR-003**: System MUST detect `.claude/mind.mv2` as a legacy path and suggest migration to `.rusty-brain/mind.mv2` (updated from the previous `.agent-brain/` target).
- **FR-004**: System MUST use the existing legacy file when `.agent-brain/mind.mv2` exists but `.rusty-brain/mind.mv2` does not (graceful fallback, no data loss).
- **FR-005**: System MUST prefer `.rusty-brain/mind.mv2` over `.agent-brain/mind.mv2` when both exist.
- **FR-006**: System MUST place all supporting files (deduplication cache, version marker) in the `.rusty-brain/` directory for new installations.
- **FR-007**: System MUST continue to respect custom memory paths set via environment variables or configuration, regardless of the default path change.
- **FR-008**: System MUST NOT automatically move or copy files between old and new directories — migration is user-initiated.
- **FR-009**: System MUST provide clear, actionable migration instructions when legacy paths are detected, including the specific file move commands.
- **FR-010**: System MUST update all documentation, skill definitions, and configuration templates to reference `.rusty-brain/` instead of `.agent-brain/`.

### Key Entities

- **Memory Directory**: The `.rusty-brain/` directory at the repository root containing all rusty-brain data files. Replaces `.agent-brain/` as the default location.
- **Legacy Path**: A previously-used memory file location (`.agent-brain/mind.mv2` or `.claude/mind.mv2`) that is detected and triggers migration guidance.
- **Path Resolution Order**: The priority order for finding memory files: (1) custom configured path, (2) `.rusty-brain/mind.mv2`, (3) `.agent-brain/mind.mv2`, (4) `.claude/mind.mv2`.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: 100% of new installations create the memory file at `.rusty-brain/mind.mv2` (not `.agent-brain/mind.mv2`).
- **SC-002**: Existing users with `.agent-brain/mind.mv2` experience zero data loss — the system continues to read their existing file until they choose to migrate.
- **SC-003**: Migration instructions are displayed for 100% of detected legacy paths, with exact commands the user can run.
- **SC-004**: All documentation, skill definitions, and templates reference `.rusty-brain/` within this feature's scope.
- **SC-005**: Custom memory paths continue to work with zero behavioral changes.

## Assumptions

- The `.rusty-brain/` directory name is the final branding choice and will not change again.
- Users are expected to manually move files when migrating — the system does not auto-migrate to avoid unintended data operations.
- The `.rusty-brain/` directory should be added to `.gitignore` by users (memory files are project-local, not shared).
- The environment variable `MEMVID_PLATFORM_PATH_OPT_IN` and any custom `memory_path` configuration continue to override the default regardless of this change.
- The `crates/types` configuration module contains the default path constant that needs updating.

## Scope Boundaries

### In Scope

- Changing the default memory directory from `.agent-brain/` to `.rusty-brain/`
- Adding `.agent-brain/` to the legacy path detection chain
- Updating `.claude/` legacy detection to point to `.rusty-brain/`
- Updating all documentation and skill files to reference `.rusty-brain/`
- Updating configuration defaults and constants
- Updating test fixtures and assertions

### Out of Scope

- Automatic file migration (users move files manually)
- Changing the memory file name (`mind.mv2` stays the same)
- Changing the memory file format
- Changing custom/overridden path behavior
- Updating third-party documentation or external references
