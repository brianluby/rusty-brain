# Implementation Plan: Plugin Packaging & Distribution

**Branch**: `009-plugin-packaging` | **Date**: 2026-03-04 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/specs/009-plugin-packaging/spec.md`

## Summary

Add a GitHub Actions release pipeline and install scripts that build cross-platform binaries for 5 targets, package them with SHA-256 checksums, and install the binary plus Claude Code plugin manifests to well-known paths (`~/.local/bin` and `~/.claude/plugins/rusty-brain/`). No Rust code changes are needed — the existing `crates/cli` and `crates/hooks` binaries are the build targets. All new files are scripts, YAML workflows, and JSON/Markdown manifests.

## Technical Context

**Language/Version**: Rust stable, edition 2024, MSRV 1.85.0 (binary already built by workspace). Shell scripts: POSIX sh + PowerShell 5.1+.
**Primary Dependencies**: cross-rs (CI only, for Linux musl cross-compilation), `houseabsolute/actions-rust-cross` GitHub Action. No new Rust crate dependencies.
**Storage**: N/A (no new storage; existing `.mv2` files are preserved, never touched).
**Testing**: ShellCheck (lint for install.sh), bats-core (unit tests for install.sh functions), Pester (PowerShell tests for install.ps1), GitHub Actions matrix (integration: binary smoke tests on all 5 targets).
**Target Platform**: Linux x86_64 (musl), Linux aarch64 (musl), macOS x86_64, macOS aarch64, Windows x86_64 (MSVC).
**Project Type**: Distribution/packaging overlay on existing single-binary Rust workspace.
**Performance Goals**: Install completes in <60 seconds on broadband. Binary size <50 MB compressed per target.
**Constraints**: `install.sh` must be POSIX sh (no bashisms). Linux targets use musl for full static linking. macOS minimum 11.0 (Big Sur). Windows uses MSVC toolchain. No shell config file modification. SHA-256 verification mandatory before binary placement.
**Scale/Scope**: 5 platform targets. ~200 lines per install script. ~150 lines release workflow. ~20 static manifest/skill files.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | Notes |
|-----------|--------|-------|
| I. Crate-First Architecture | PASS | No new crates. All new files are scripts, workflows, and manifests outside `crates/`. |
| II. Rust-First Implementation | PASS | No Rust code changes. Builds existing `crates/cli` and `crates/hooks` binaries. |
| III. Agent-Friendly Interface Design | PASS | Plugin manifests and skill definitions follow Claude Code's structured discovery format. |
| IV. Contract-First Development | PASS | Release asset naming contract, plugin.json schema, and SKILL.md format defined in AR before implementation. |
| V. Test-First Development | PASS | Install script tests (bats/Pester) written before scripts. CI matrix validates all 5 targets. |
| VI. Complete Requirement Delivery | PASS | All M-1 through M-11 Must Have requirements mapped to implementation components (see AR traceability matrix). |
| VII. Memory Integrity and Data Safety | PASS | Install scripts explicitly never touch `~/.agent-brain/` or `.mv2` files (SEC-1). |
| VIII. Performance and Scope Discipline | PASS | Install speed (<60s) is measurable via CI timed tests. No speculative performance work. |
| IX. Security-First Design | PASS | SEC-1 through SEC-12 mapped from security review. HTTPS enforced, SHA-256 verification, no `eval`, pinned action SHAs. |
| X. Error Handling Standards | PASS | Install scripts provide machine-parseable errors with specific failure categories (unsupported platform, network failure, checksum mismatch, permission denied). |
| XI. Observability and Debuggability | PASS | Install scripts print progress to stdout. `--version` flag provides version tracking. |
| XII. Simplicity and Pragmatism | PASS | Custom scripts (~200 lines each) over cargo-dist abstraction. No new frameworks introduced. |
| XIII. Dependency Policy | PASS | No new Rust crate dependencies. CI-only dependency: cross-rs (Docker-based, not bundled). |

**Gate Result: PASS** — No violations. Proceed to Phase 0.

## Project Structure

### Documentation (this feature)

```text
specs/009-plugin-packaging/
├── plan.md              # This file
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
├── quickstart.md        # Phase 1 output
├── contracts/           # Phase 1 output
│   ├── plugin-json.schema.json
│   ├── marketplace-json.schema.json
│   ├── hooks-json.schema.json
│   ├── release-asset-naming.md
│   └── install-script-interface.md
└── tasks.md             # Phase 2 output (/speckit.tasks)
```

### Source Code (repository root)

```text
# New files added by this feature (no existing files modified)
packaging/
├── claude-code/
│   ├── .claude-plugin/
│   │   └── plugin.json              # Plugin identity manifest
│   ├── hooks/
│   │   └── hooks.json               # Hook registration (SessionStart, PostToolUse, Stop)
│   ├── skills/
│   │   ├── mind/
│   │   │   └── SKILL.md             # mind skill definition
│   │   └── memory/
│   │       └── SKILL.md             # memory skill definition
│   ├── commands/
│   │   ├── ask.md                   # /ask slash command
│   │   ├── search.md                # /search slash command
│   │   ├── recent.md                # /recent slash command
│   │   └── stats.md                 # /stats slash command
│   ├── marketplace.json             # Marketplace registry
│   └── README.md                    # Plugin README (optional)
└── opencode/
    └── commands/
        ├── mind-ask.md              # /ask command for OpenCode
        ├── mind-search.md           # /search command
        ├── mind-recent.md           # /recent command
        └── mind-stats.md            # /stats command

.github/workflows/
└── release.yml                      # Release CI pipeline (alongside existing ci.yml)

install.sh                           # POSIX sh install script (macOS/Linux)
install.ps1                          # PowerShell install script (Windows)

tests/
├── install_script_test.bats         # bats-core tests for install.sh
└── install_script_test.ps1          # Pester tests for install.ps1
```

**Structure Decision**: Distribution overlay — all new files are outside the `crates/` workspace. The `packaging/` directory holds static manifest and skill files. Install scripts and release workflow are at conventional repo-root locations. This maintains clean separation between Rust source code and distribution packaging.

## Complexity Tracking

No constitution violations — this section is intentionally empty.
