# Implementation Plan: Claude Code Hooks

**Branch**: `006-claude-code-hooks` | **Date**: 2026-03-03 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/specs/006-claude-code-hooks/spec.md`

## Summary

Build a single `rusty-brain` binary with four clap subcommands (`session-start`, `post-tool-use`, `stop`, `smart-install`) that implement the Claude Code hook protocol. Each subcommand reads `HookInput` JSON from stdin, delegates to existing `crates/core` (Mind API) and `crates/platforms` (detection/resolution), and writes `HookOutput` JSON to stdout. All code paths fail-open with `continue: true` and exit code 0. The architecture is a layered handler design: I/O layer (stdin/stdout/fail-open boundary) -> dispatch layer (clap subcommands) -> handler layer (pure functions returning `Result<HookOutput, HookError>`).

## Technical Context

**Language/Version**: Rust stable, edition 2024, MSRV 1.85.0
**Primary Dependencies**: clap 4 (subcommand dispatch), serde/serde_json (JSON protocol), tracing (diagnostics), existing workspace crates (core, types, platforms)
**Storage**: `.agent-brain/mind.mv2` (memvid-encrypted observations), `.agent-brain/.dedup-cache.json` (hash-based dedup), `.install-version` (version marker)
**Testing**: `cargo test` — unit tests in-module, integration tests in `crates/hooks/tests/`
**Target Platform**: macOS, Linux (same as workspace)
**Project Type**: Single binary crate within existing workspace (`crates/hooks`)
**Performance Goals**: session-start < 200ms (1K observations), post-tool-use < 100ms (typical tool output)
**Constraints**: No async/tokio in hooks binary (synchronous subprocess); exit code always 0; no stderr unless `RUSTY_BRAIN_LOG` set; all output valid JSON
**Scale/Scope**: Handles memory files with up to 1,000+ observations; concurrent sessions via existing file locking

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | How Satisfied |
|-----------|--------|---------------|
| I. Crate-First Architecture | PASS | All work in existing `crates/hooks` skeleton; no new crates |
| II. Rust-First Implementation | PASS | Stable Rust only; no `unsafe`; memvid isolated behind Mind trait in `crates/core` |
| III. Agent-Friendly Interface | PASS | JSON stdin/stdout; no interactive prompts; structured errors; `HookOutput` is machine-parseable |
| IV. Contract-First Development | PASS | `HookInput`/`HookOutput` contracts already defined in `crates/types`; handler contracts defined in this plan's `contracts/` |
| V. Test-First Development | PASS | Tests authored before implementation per TDD workflow; handler functions independently testable |
| VI. Complete Requirement Delivery | PASS | All M-1 through M-11 mapped to components; acceptance criteria traceable to PRD |
| VII. Memory Integrity | PASS | Uses `Mind::remember` (atomic writes via `crates/core`); dedup cache uses atomic write (temp+rename) |
| VIII. Performance Discipline | PASS | Measurable targets: session-start <200ms, post-tool-use <100ms; benchmarked in integration tests |
| IX. Security-First | PASS | SEC-1 through SEC-10 from sec.md mapped to implementation; no logging of memory content at INFO+; memvid encryption enabled |
| X. Error Handling Standards | PASS | `HookError` enum with stable error codes; fail-open boundary converts all errors to valid `HookOutput` |
| XI. Observability | PASS | `tracing` gated on `RUSTY_BRAIN_LOG`; session_id as structured field; latency warnings at 150ms/75ms thresholds |
| XII. Simplicity | PASS | Plain handler functions, not trait-based dispatch; no framework; extends existing patterns |
| XIII. Dependency Policy | PASS | Only adds `tracing-subscriber` (env filter for `RUSTY_BRAIN_LOG`); all other deps already in workspace |

**Gate Result**: PASS — No violations. One new dependency (`tracing-subscriber`) justified by M-10/S-3 (env-var gated logging).

## Project Structure

### Documentation (this feature)

```text
specs/006-claude-code-hooks/
├── plan.md              # This file
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
├── quickstart.md        # Phase 1 output
├── contracts/           # Phase 1 output
│   └── hooks-api.rs     # Handler function signatures and types
├── ar.md                # Architecture Review (existing)
├── prd.md               # Product Requirements Document (existing)
├── sec.md               # Security Review (existing)
├── spec.md              # Feature specification (existing)
└── checklists/          # Checklists (existing)
```

### Source Code (repository root)

```text
crates/hooks/
├── Cargo.toml           # Dependencies: core, types, platforms, clap, serde_json, tracing, tracing-subscriber
├── src/
│   ├── main.rs          # Entry point: clap parse, init logging, orchestrate I/O, fail-open boundary
│   ├── lib.rs           # Crate root: module declarations, public re-exports for testing
│   ├── io.rs            # read_input(), write_output(), fail_open()
│   ├── dispatch.rs      # Cli struct (clap derive), Subcommand enum
│   ├── error.rs         # HookError enum with stable error codes
│   ├── session_start.rs # handle_session_start(HookInput) -> Result<HookOutput, HookError>
│   ├── post_tool_use.rs # handle_post_tool_use(HookInput) -> Result<HookOutput, HookError>
│   ├── stop.rs          # handle_stop(HookInput) -> Result<HookOutput, HookError>
│   ├── smart_install.rs # handle_smart_install(HookInput) -> Result<HookOutput, HookError>
│   ├── dedup.rs         # DedupCache: file-based 60s dedup with hash keys
│   ├── truncate.rs      # head_tail_truncate(content, max_tokens) -> String
│   ├── git.rs           # detect_modified_files(cwd) -> Vec<String> (subprocess with timeout)
│   ├── manifest.rs      # generate_manifest(binary_path) -> hooks.json content
│   └── context.rs       # format_system_message(InjectedContext, MindStats) -> String
└── tests/
    ├── common/
    │   └── mod.rs        # Shared test helpers (sample HookInput builders, temp mind setup)
    ├── io_test.rs        # Unit: read_input, write_output, fail_open
    ├── truncate_test.rs  # Unit: head/tail truncation
    ├── dedup_test.rs     # Unit: duplicate detection, expiry, prune, corrupt file
    ├── git_test.rs       # Integration: real git repo tempdir
    ├── session_start_test.rs  # Integration: Mind + platforms
    ├── post_tool_use_test.rs  # Integration: observe + dedup
    ├── stop_test.rs      # Integration: git + session summary
    ├── smart_install_test.rs  # Unit: version marker
    ├── manifest_test.rs  # Unit: JSON schema validation
    └── e2e_test.rs       # E2E: invoke binary as subprocess
```

**Structure Decision**: Single binary crate (`crates/hooks`) within the existing workspace. Each handler in its own module file. Tests split between unit (in-module and `tests/`) and integration (in `tests/`). This follows the existing workspace pattern from `crates/core` and `crates/platforms`.

## Complexity Tracking

No constitution violations requiring justification. The one new dependency (`tracing-subscriber`) is minimal and directly required by functional requirements (M-10, S-3).

| Addition | Why Needed | Simpler Alternative Rejected Because |
|----------|------------|-------------------------------------|
| `tracing-subscriber` (env filter) | Required by M-10/S-3: env-var gated diagnostic logging | No logging at all — rejected because XI (Observability) requires diagnosability |
