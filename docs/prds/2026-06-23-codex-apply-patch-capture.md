# PRD: Capture Codex `apply_patch` PostToolUse Events

## Status

Draft, readiness-gated. Do not implement capture mapping until a current local Codex build proves that `apply_patch` emits the expected hook event shape.

## Owner Area

Primary: Codex adapter and hook capture observation extraction.

Touchpoints:

- `crates/rb-agents/src/codex.rs`
- `crates/rb-agents/tests/cross_adapter.rs`
- `crates/rb-hooks/src/capture.rs`
- `crates/rb-hooks/tests/integration.rs`
- `docs/follow-ups/2026-06-02-codex-apply-patch-capture.md`

## Problem

Codex `apply_patch` edits have historically not produced the same `PostToolUse` capture path as shell commands. The existing capture layer intentionally avoids a bare `apply_patch` mapping because it would record `unknown` or miss all changed paths.

Once Codex emits real `PostToolUse` events for `apply_patch`, rusty-brain must capture file paths from the raw patch safely and avoid storing patch contents.

## Goals

- Capture Codex `apply_patch` file paths once the local Codex hook shape is verified.
- Extract changed file paths from the raw patch payload.
- Append useful observations to session scratch.
- Preserve existing Bash command capture behavior.
- Avoid storing raw patch body, code content, or secrets.
- Add fixture-backed tests before enabling the mapping.

## Non-Goals

- Do not infer support from GitHub issue status alone.
- Do not parse arbitrary patch hunks for content.
- Do not record full patch text in memories or scratch.
- Do not change canonical `Stop` behavior.
- Do not solve all Codex lifecycle capture; that belongs to the non-Claude capture regression task.

## Functional Requirements

### AP1. Readiness Trigger

Before code changes are accepted, record a real current Codex fixture showing:

- Codex version/date.
- `apply_patch` emits `PostToolUse`.
- tool name is exactly known, such as `apply_patch`, not guessed.
- raw patch is present in `tool_input.command` or a verified equivalent field.
- Bash tool events still parse as Bash or command observations.

If the fixture does not exist, leave the implementation blocked and document the blocker.

### AP2. Adapter Payload Preservation

The Codex adapter must preserve enough `tool_input` payload for the capture layer to extract paths.

Tests must prove:

- `apply_patch` tool name round-trips.
- raw patch payload is available to capture.
- missing or malformed payloads do not panic.
- Bash mapping remains unchanged.

### AP3. Path Extraction

Add an extractor that reads only the patch command payload and returns changed paths.

Recognize:

- `*** Add File: <path>`
- `*** Update File: <path>`
- `*** Delete File: <path>`
- `*** Move to: <path>`

For moves, include both source and destination when the source is recoverable from the surrounding update block. Preserve first-seen order and dedupe.

Rules:

- Do not return hunk content.
- Do not return raw lines other than file paths.
- Do not return `unknown` when no path is found.
- Normalize only enough to avoid obvious empty or unsafe paths.
- Fail open by returning no observations on malformed patches.

### AP4. Capture Observations

Refactor singular tool observation handling if necessary so one `apply_patch` event can append multiple touched files.

Desired scratch entries:

```text
file touched: crates/rb-hooks/src/capture.rs
file touched: crates/rb-agents/src/codex.rs
```

Do not include the patch body.

### AP5. Safety

Add tests that patch content containing token-like strings, code, or comments does not appear in scratch or memory.

If the event shape is unknown, capture must do nothing rather than write misleading observations.

## Acceptance Criteria

- A real Codex `apply_patch` fixture is committed with provenance and sanitization notes, or the PRD remains blocked.
- Codex adapter tests prove the event shape.
- Capture extractor handles add, update, delete, and move.
- Multi-file patches append multiple file observations.
- Patch hunk text never appears in scratch or folded summaries.
- Bash command capture remains green.
- Missing or malformed patch payloads fail open.
- Follow-up documentation is updated with final support status.

## Verification

Run:

```bash
cargo test -p rb-agents codex
cargo test -p rb-agents --test cross_adapter
cargo test -p rb-hooks apply_patch
cargo test -p rb-hooks post_tool_use
```

If integration tests are added, include a Codex `apply_patch` fixture sequence that proves the paths survive into the folded session summary without leaking patch content.

## Risks

- Upstream Codex event shape differs from assumptions. Mitigate with local fixture gating.
- Parser captures code content. Mitigate with path-only extraction and leak tests.
- Multi-file patches lose paths. Mitigate with ordered dedupe tests.
- A bare mapping records `unknown`. Mitigate by returning no observation when paths cannot be extracted.
- This is mistaken for full Codex capture support. Link to the broader non-Claude capture regression and parity PRDs.

## Implementation Checklist

- [ ] Record current Codex `apply_patch` fixture.
- [ ] Document Codex version and event shape.
- [ ] Add adapter fixture test.
- [ ] Implement path-only extractor.
- [ ] Add add/update/delete/move tests.
- [ ] Add malformed payload tests.
- [ ] Add no-raw-patch leak tests.
- [ ] Append multiple observations.
- [ ] Verify Bash capture unchanged.
- [ ] Update follow-up doc.
