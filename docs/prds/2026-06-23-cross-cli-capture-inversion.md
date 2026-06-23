# PRD: Fix Non-Claude CLI Capture Regression After W3.1 Capture Inversion

## Status

Draft, implementation-ready after real lifecycle fixtures are recorded.

## Owner Area

Primary: `rb-agents` adapters and `rb-hooks` capture lifecycle.

Touchpoints:

- `crates/rb-agents/src/gemini.rs`
- `crates/rb-agents/src/codex.rs`
- `crates/rb-agents/src/opencode.rs`
- `crates/rb-agents/src/event.rs`
- `crates/rb-hooks/src/capture.rs`
- `crates/rb-hooks/src/dispatch.rs`
- `crates/rb-hooks/src/main.rs`
- `crates/rb-hooks/tests/integration.rs`
- `crates/rb-agents/tests/cross_adapter.rs`
- `docs/follow-ups/2026-06-13-cross-cli-capture-inversion.md`

## Problem

W3.1 changed capture so `PostToolUse` writes no durable memories. It now appends observations to per-session scratch. Scratch is folded into one durable memory only on canonical `HookEvent::SessionEnd`.

Claude Code is correct because it maps native `SessionEnd` to canonical `SessionEnd`. Non-Claude adapters currently append scratch but may never flush it:

| CLI | Current terminal-looking event | Current canonical mapping |
| --- | --- | --- |
| Gemini | `SessionEnd` | `Stop` |
| Codex | `Stop` | `Stop` |
| OpenCode | `session.idle` | `Stop` |

Because canonical `Stop` is intentionally a no-op, these paths can regress from per-event capture to no automatic capture. Scratch then ages out.

## Goals

- Restore automatic session summary capture for Gemini, Codex, and OpenCode where the host exposes a reliable terminus.
- Preserve Claude's W3.1 behavior: `PostToolUse` writes scratch, `Stop` stores nothing, `SessionEnd` folds once.
- Require recorded fixtures before changing terminal-event mappings.
- Distinguish per-turn stop from true session terminus.
- Define an explicit fallback for CLIs with no reliable terminus.
- Keep hooks fail-open with valid output and `continue: true`.

## Non-Goals

- Do not reintroduce per-tool durable writes.
- Do not make canonical `Stop` fold globally.
- Do not change daemon protocol or memory schema.
- Do not add prompt-time retrieval parity here; that belongs to the cross-agent parity task.
- Do not add OpenCode installer support unless fixture recording requires a small tested integration.

## Functional Requirements

### X1. Fixture-Backed Lifecycle Classification

For Gemini, Codex, and OpenCode, record real native hook payloads from a multi-turn session and classify:

- session start
- tool completion
- per-turn stop
- true session terminus
- compaction, if available
- unknown or malformed events

Do not accept mapping changes based on event names alone.

Add fixture directories:

```text
crates/rb-hooks/tests/fixtures/gemini/
crates/rb-hooks/tests/fixtures/codex/
crates/rb-hooks/tests/fixtures/opencode/
```

Each directory needs a `README.md` with:

- CLI name and version
- OS and capture date
- recording recipe
- sanitization notes
- event cadence from a multi-turn run
- chosen mapping or fallback
- known absences, such as no transcript path

### X2. Adapter Mapping Rules

Rules:

- A verified true terminus maps to `HookEvent::SessionEnd`.
- A verified per-turn stop maps to `HookEvent::Stop`.
- Idle events may not map to `SessionEnd` unless fixtures prove terminal behavior.
- Unknown or malformed events map to `Other`.
- Parsing remains fail-open with no panics on missing fields.

Likely decisions to verify:

- Gemini native `SessionEnd`: determine whether it is session-terminal or per-turn.
- Codex native `Stop`: determine whether it is terminal or per-turn.
- OpenCode `session.idle` and `session.deleted`: determine whether either is terminal.

### X3. Fallback Policy

If a CLI lacks a reliable terminus, implement an adapter-specific checkpoint fallback rather than changing `Stop`.

Preferred behavior:

- Add a `SessionCheckpoint` event or equivalent internal capture path.
- Route only the affected adapter's verified best available boundary to checkpoint.
- Fold current scratch into a summary without clearing scratch.
- Supersede or update one live session summary.
- Preserve early and late observations across multiple turns.
- Never use checkpoint for Claude Code.

If fallback is rejected for a CLI, document automatic capture as unsupported for that CLI and do not claim the regression is fixed.

### X4. Hook Dispatch and Daemon Connection

If only `SessionEnd` mappings are added, existing daemon-connection behavior should be enough.

If checkpoint is added:

- `event_needs_daemon` includes checkpoint.
- dispatch routes checkpoint.
- capture implements fold-without-clear.
- `PostToolUse` still skips daemon connection.
- `Stop` still skips daemon connection.

### X5. Tests

Add fixture-backed parse tests for each adapter:

- session start parses exactly
- tool completion parses exactly
- per-turn stop parses as `Stop`
- true terminus parses as `SessionEnd`, if available
- fallback boundary parses to checkpoint, if used

Add lifecycle tests:

- `PostToolUse` writes no memory.
- per-turn `Stop` writes no memory.
- terminus or checkpoint writes one useful summary.
- folded summary includes touched file or command.
- mock daemon receives the correct `identity_agent`.
- a multi-turn fallback sequence keeps one live summary and preserves early and late observations.
- Claude `Stop` remains a no-op.

## Acceptance Criteria

- Fixture directories exist for Gemini, Codex, and OpenCode, or a blocker is documented.
- Each adapter has a tested mapping table.
- No adapter maps a per-turn stop to `SessionEnd`.
- Non-Claude lifecycle tests prove scratch folds through a verified terminus or checkpoint.
- A 40-turn simulated session produces at most five live memories per restored CLI.
- Malformed payloads remain fail-open.
- `cargo test -p rb-agents` passes.
- `cargo test -p rb-hooks` passes.
- Follow-up documentation records final mapping decisions.

## Verification

Run:

```bash
cargo test -p rb-agents
cargo test -p rb-hooks
cargo test -p rb-install forty_turn_session_produces_at_most_five_memories
```

If test names differ after implementation, run the equivalent per-agent lifecycle and 40-turn memory-volume tests.

## Risks

- Terminal-looking events are per-turn. Mitigate with multi-turn fixtures.
- Checkpoint fallback clears scratch and loses early turns. Mitigate with retention tests.
- Checkpoint fallback creates too many archived rows. Measure archived write count and keep one live summary.
- OpenCode installer remains deferred. Test `rusty-brain-hooks --agent opencode` directly and leave installer work to the parity task.
- Claude behavior regresses. Preserve existing Claude fixture tests and keep `Stop` no-op.

## Implementation Checklist

- [ ] Record Gemini fixtures.
- [ ] Record Codex fixtures.
- [ ] Record OpenCode fixtures or document blocker.
- [ ] Add fixture README files.
- [ ] Add adapter parse tests.
- [ ] Update mappings only after fixture proof.
- [ ] Add checkpoint event/path if required.
- [ ] Update dispatch and daemon-connection logic if checkpoint exists.
- [ ] Generalize lifecycle test helpers by agent id.
- [ ] Add non-Claude fold tests.
- [ ] Add multi-turn fallback retention tests.
- [ ] Add or parameterize 40-turn memory-volume tests.
- [ ] Update follow-up documentation.
