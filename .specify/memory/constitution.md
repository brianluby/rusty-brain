# rusty-brain Constitution

## Preamble

rusty-brain is a Rust-based memory system designed to be consumed by AI coding agents (such as claude-code and opencode). It is inspired by [agent-brain](https://github.com/brianluby/agent-brain). It leverages [memvid](https://github.com/brianluby/memvid) for video-encoded memory storage and retrieval. This constitution governs all feature-level specs, plans, tasks, and implementation decisions in this repository, managed through the specify workflow. 

---

## Core Principles

### I. Crate-First Architecture

- All feature work MUST be implemented within the existing Rust crate layout unless a new crate is explicitly justified in the feature plan.
- New modules MUST have a clear runtime or test responsibility tied to documented requirements.
- Workspace boundaries between core memory logic, memvid integration, and agent-facing interfaces MUST remain well-defined.

### II. Rust-First Implementation

- Production and test implementations MUST use stable Rust.
- `unsafe` code MUST NOT be introduced without an explicit architecture justification and reviewer sign-off.
- Integration boundaries with memvid MUST be isolated behind clean Rust abstractions (traits or wrapper modules) so that upstream memvid changes don't ripple through the codebase.

### III. Agent-Friendly Interface Design

- All public APIs MUST be designed for consumption by non-human agents (CLI, structured output, machine-readable errors).
- Command output MUST default to structured formats (JSON, TOML, or similar) that agents can parse without fragile text scraping.
- Interactive prompts MUST NOT appear in agent-facing code paths; all inputs MUST be passable as arguments or configuration.
- API changes MUST be backward-compatible within a major version, or accompanied by a migration guide in the feature plan.

### IV. Contract-First Development

- A feature MUST define or update interface contracts before implementation tasks are finalized.
- Contracts MUST align with the actual implementation style (e.g., Rust trait definitions for core APIs, CLI argument schemas for agent interfaces).
- Memory storage and retrieval contracts MUST specify expected encoding, latency bounds, and failure modes.

### V. Test-First Development (Non-Negotiable)

- For each new behavior, tests MUST be authored before implementation is considered complete.
- Implementation is done when targeted tests pass and regression tests remain green.
- Memory round-trip tests (store → encode → retrieve → verify) MUST exist for any storage path.

### VI. Complete Requirement Delivery

- All Must-Have requirements in the active feature scope MUST be covered by executable tasks.
- A feature is not complete if a baseline acceptance criterion has no verification task.

### VII. Memory Integrity and Data Safety

- Stored memories MUST be retrievable without data loss or silent corruption after encoding.
- All write operations MUST be atomic or safely recoverable; partial writes MUST NOT leave the memory store in an inconsistent state.
- Memory indices and metadata MUST be validated on load; corrupted state MUST produce actionable errors, never silent fallback to empty.

### VIII. Performance and Scope Discipline

- Performance targets MUST be measurable when performance is in scope (e.g., retrieval latency, encoding throughput, memory footprint).
- Features explicitly marked out-of-scope for performance benchmarking MUST NOT add speculative benchmark work.
- Memory search and retrieval MUST remain responsive at the documented scale targets.

### IX. Security-First Design

- Security requirements from SEC artifacts MUST be mapped to tasks when applicable.
- Memory contents MUST be treated as potentially sensitive; no memory data may be logged at INFO level or above without explicit opt-in.
- Local-only operation is the default; any network capability (sync, remote storage) MUST be opt-in and documented in SEC artifacts.
- Secret material (API keys, tokens) MUST NOT be stored in the memory system or passed through agent-facing output.

### X. Error Handling Standards

- User-facing and agent-facing failures MUST be actionable, machine-parseable, and testable.
- Errors MUST include a stable error code or category alongside human-readable messages.
- Assertions against failure text SHOULD favor stable substrings or error codes over brittle full-message equality.

### XI. Observability and Debuggability

- Test and runtime behavior MUST remain diagnosable through deterministic artifacts (snapshots, fixtures, structured outputs, and clear diagnostics).
- Silent failure handling is prohibited.
- Agent-consumable diagnostic output (e.g., `--verbose`, `--debug` flags with structured output) MUST be available for troubleshooting workflows.

### XII. Simplicity and Pragmatism

- Extend existing harnesses and patterns before introducing new frameworks.
- Scope expansion MUST be explicit and justified by requirements.
- Prefer well-understood Rust patterns (enums for state, `Result` for errors, traits for extension) over clever abstractions.

### XIII. Dependency Policy

- New dependencies MUST be avoided unless required by documented requirements and approved in plan artifacts.
- Existing dependencies SHOULD be reused when they satisfy feature needs.
- memvid version MUST be pinned in `Cargo.toml` and upgrades MUST be tested against memory round-trip and retrieval correctness before merge.

---

## Quality Gates

All implementation-ready work MUST pass these gates before merge:

- `cargo test` — all tests green
- `cargo clippy --workspace -- -D warnings` — no lint violations
- `cargo fmt --check` — formatting compliant
- Agent integration smoke test — CLI commands produce valid structured output

---

## Specify Workflow

1. **Clarify** — Resolve requirements and high-impact ambiguities via specify artifacts.
2. **Design** — Produce `spec.md` and `plan.md` with interface contracts and architecture decisions.
3. **Task** — Generate `tasks.md` with explicit requirement coverage and acceptance criteria.
4. **Implement** — Test-first execution; pass all quality gates before marking tasks complete.
5. **Verify** — Feature artifacts (`spec.md`, `plan.md`, `tasks.md`) MUST stay mutually consistent on scope, strategy, and acceptance behavior.

---

## Governance

- This constitution governs all feature-level specs, plans, tasks, and implementation decisions in this repository.
- Constitution conflicts are resolved by updating feature artifacts to comply; weakening constitutional rules requires a separate explicit constitution amendment.
- Amendments MUST include rationale, affected principles, and migration impact on active features.

**Version**: 2.0.0 | **Ratified**: 2026-03-01 | **Last Amended**: 2026-03-01