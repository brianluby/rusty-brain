# Implementation Plan: Agent Installs

**Branch**: `011-agent-installs` | **Date**: 2026-03-05 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/specs/011-agent-installs/spec.md`

## Summary

Add a `rusty-brain install` subcommand that auto-detects installed AI coding agents (OpenCode, Copilot CLI, Codex CLI, Gemini CLI) and generates agent-specific configuration files so each agent can invoke rusty-brain for persistent memory. Uses a trait-based installer pattern within the existing `platforms` crate (per AR decision), with atomic file writes, `.bak` backups, and structured JSON output for agentic self-installation.

## Technical Context

**Language/Version**: Rust stable, edition 2024, MSRV 1.85.0
**Primary Dependencies**: clap 4 (derive), serde/serde_json, tracing, tempfile (promote from dev-dep to regular dep)
**Storage**: Local filesystem only — config files written to agent directories, no database
**Testing**: `cargo test` (unit + integration), `assert_cmd`/`predicates` for CLI tests, `tempfile` for test isolation
**Target Platform**: macOS, Linux, Windows (cross-platform)
**Project Type**: Rust workspace with existing crates (cli, core, platforms, types, hooks, compression, opencode)
**Performance Goals**: Single-agent install < 5s, multi-agent install < 30s (local filesystem operations)
**Constraints**: No network calls, atomic writes (temp+rename), no interactive prompts in non-TTY mode, no `unsafe` code
**Scale/Scope**: 4 agent platforms, ~8-12 new source files across platforms and cli crates

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | Notes |
|-----------|--------|-------|
| I. Crate-First | PASS | Install logic goes in existing `platforms` crate per AR |
| II. Rust-First | PASS | Stable Rust, no unsafe, memvid boundary not affected |
| III. Agent-Friendly | PASS | JSON output (M-7), no interactive prompts (M-11), machine-parseable errors (M-10) |
| IV. Contract-First | PASS | `AgentInstaller` trait defined in AR before implementation |
| V. Test-First | PASS | Test strategy defined in AR; unit + integration tests planned |
| VI. Complete Requirement Delivery | PASS | All 13 Must Have + 4 Should Have requirements have acceptance criteria |
| VII. Memory Integrity | PASS | Install does not read/write `.mv2` files; only references paths |
| VIII. Performance Discipline | PASS | Install performance targets defined (< 5s / < 30s) |
| IX. Security-First | PASS | SEC review complete; 10 SEC requirements mapped (SEC-1 through SEC-10) |
| X. Error Handling | PASS | `InstallError` enum with stable codes defined in AR |
| XI. Observability | PASS | Tracing via `RUSTY_BRAIN_LOG`; structured output |
| XII. Simplicity | PASS | Extends existing patterns (AdapterRegistry -> InstallerRegistry) |
| XIII. Dependency Policy | PASS | Only `tempfile` promoted from dev to regular; no new external deps |

**All gates PASS. No violations to justify.**

## Project Structure

### Documentation (this feature)

```text
specs/011-agent-installs/
├── spec.md              # Feature specification
├── prd.md               # Product requirements document
├── ar.md                # Architecture review
├── sec.md               # Security review
├── plan.md              # This file
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
├── quickstart.md        # Phase 1 output
├── contracts/           # Phase 1 output
│   └── agent-installer-trait.rs
├── checklists/
│   └── requirements.md
└── tasks.md             # Phase 2 output (from /speckit.tasks)
```

### Source Code (repository root)

```text
crates/
├── platforms/src/
│   ├── installer/                    # NEW: install module tree
│   │   ├── mod.rs                    # Module declarations, AgentInstaller trait
│   │   ├── orchestrator.rs           # InstallOrchestrator workflow
│   │   ├── registry.rs              # InstallerRegistry (mirrors AdapterRegistry)
│   │   ├── writer.rs                # ConfigWriter (atomic write + backup)
│   │   └── agents/                  # Per-agent installer implementations
│   │       ├── mod.rs
│   │       ├── opencode.rs          # OpenCodeInstaller
│   │       ├── copilot.rs           # CopilotInstaller
│   │       ├── codex.rs             # CodexInstaller
│   │       └── gemini.rs            # GeminiInstaller
│   ├── lib.rs                       # MODIFY: add `pub mod installer;`
│   └── ... (existing files unchanged)
├── types/src/
│   ├── install.rs                   # NEW: InstallError, InstallStatus, etc.
│   └── lib.rs                       # MODIFY: add `pub mod install;`
├── cli/src/
│   ├── args.rs                      # MODIFY: add Install subcommand variant
│   ├── commands.rs                  # MODIFY: add install dispatch
│   └── install_cmd.rs              # NEW: install command handler
└── ... (other crates unchanged)
```

**Structure Decision**: Extends existing workspace layout. New `installer/` subtree in `platforms` crate contains all install logic. Types in `types` crate for cross-crate use. CLI handler in `cli` crate.

## Constitution Re-Check (Post Phase 1 Design)

| Principle | Status | Notes |
|-----------|--------|-------|
| I. Crate-First | PASS | All code in existing crates (platforms, types, cli) |
| II. Rust-First | PASS | No unsafe; memvid not touched |
| III. Agent-Friendly | PASS | JSON output, machine-parseable errors, no prompts — confirmed in contracts |
| IV. Contract-First | PASS | `AgentInstaller` trait, `ConfigWriter`, `InstallOrchestrator` contracts defined in `contracts/agent-installer-trait.rs` |
| V. Test-First | PASS | Test strategy in AR; data model supports pure function testing |
| VI. Complete Requirement Delivery | PASS | All M-1..M-13 and S-1..S-4 covered by data model entities and contracts |
| IX. Security-First | PASS | SEC-1..SEC-10 addressed: path validation (SEC-4), allowlist (SEC-5), subprocess timeout (SEC-6), atomic writes (SEC-9) |
| XII. Simplicity | PASS | Mirrors existing AdapterRegistry pattern; no new abstractions beyond what 4 agents require |
| XIII. Dependency Policy | PASS | Only `tempfile` promoted; no new external crates |

**All gates PASS post-design. No violations.**

## Complexity Tracking

No constitution violations. No complexity justification needed.
