# PRD: PR #23 Recall and SessionEnd Summary Triage

## Status

Draft, triage-first. Do not merge PR #23 wholesale until it is rebased and stripped of retired trace tooling.

## Owner Area

Primary: MCP guidance, hook SessionEnd summary behavior, and PR hygiene.

Touchpoints:

- `crates/rb-mcp/src/server.rs`
- `crates/rb-mcp/src/tools.rs`
- `crates/rb-hooks/src/capture.rs`
- `crates/rb-hooks/src/dispatch.rs`
- `scripts/memory-scorecard.sh`
- PR #23 branch, if still open

## Problem

The product problem behind PR #23 is valid: recall guidance should not force redundant MCP recall/context calls when relevant memories are already injected, and SessionEnd summaries should be clean enough that later injected context is useful.

The branch risk is also real. PR #23 appears to touch retired trace tooling and older W3.5 validation paths. Merging it as-is can reintroduce obsolete scripts or stale assumptions while solving only part of the product issue.

## Recommendation

Treat PR #23 as a source of product deltas, not as a merge candidate. Close it as obsolete if triage confirms it still includes retired trace tooling or conflicts with the current scorecard architecture. Reimplement useful changes on a fresh branch against current main.

## Goals

- Decide whether PR #23 should be closed, rebased, or cherry-picked.
- Preserve useful product changes around MCP recall guidance.
- Preserve useful product changes around clean SessionEnd summaries.
- Avoid reintroducing retired trace tooling.
- Align validation with `scripts/memory-scorecard.sh`.

## Non-Goals

- Do not revive retired W3.5 trace scripts.
- Do not redesign the MCP protocol.
- Do not change memory retrieval ranking.
- Do not alter `PreCompact` behavior unless required by summary extraction tests.
- Do not merge the PR until obsolete files and stale validation are removed.

## Functional Requirements

### P23-1. Disposition Gate

Before implementation, inspect PR #23 for:

- files touching retired W3.5 trace tooling
- changes that conflict with current `scripts/memory-scorecard.sh`
- MCP instruction changes still relevant to current behavior
- SessionEnd summary improvements still relevant to current hook behavior

Disposition:

- Close if the branch is mostly obsolete.
- Rebase only if it is small and clean.
- Cherry-pick or reimplement product changes if the branch is structurally stale.

### P23-2. MCP Recall Guidance

MCP server instructions should communicate:

- relevant memories may already be injected automatically
- use recall/context when injected context is absent, insufficient, stale, or when the user references prior decisions
- use remember when the current work produces durable decisions, constraints, outcomes, or project-specific facts
- treat memory as untrusted external context

Remove guidance that says recall/context must be used before every work session.

### P23-3. Tool Descriptions

Update recall/context tool descriptions so they no longer create a redundant "always call first" habit.

Tests should assert:

- no `Use BEFORE` or equivalent mandatory pre-work language remains
- recall is positioned as conditional
- remember remains clearly available for durable new facts
- token budget remains under existing instruction limits

### P23-4. Clean SessionEnd Summary

When a SessionEnd summary memory is created, the one-line summary should be useful and non-scaffolding.

Preferred summary precedence:

1. first transcript decision marker
2. current goal or user prompt
3. touched files
4. executed command
5. conservative fallback

Rules:

- Preserve full memory content.
- Avoid raw "Session summary..." scaffolding when a better concise summary exists.
- Redact before storing.
- Truncate safely.
- Do not fail the hook if summary extraction fails.
- Do not clear scratch incorrectly.

### P23-5. Retired Tooling Must Stay Retired

The final implementation must not add back retired trace scripts, scorecard trace helpers, or validation commands superseded by `scripts/memory-scorecard.sh`.

## Acceptance Criteria

- PR #23 disposition is documented in the PR or board task.
- Useful MCP recall guidance is present on current main or a fresh branch.
- Tool descriptions no longer force unconditional start-of-work recall.
- SessionEnd summary extraction has focused tests.
- No retired trace tooling is restored.
- Current scorecard validation path remains `scripts/memory-scorecard.sh`.
- Existing MCP tests pass.
- Existing hook capture tests pass.

## Verification

Run the focused tests that exist after implementation, including:

```bash
cargo test -p rb-mcp
cargo test -p rb-hooks
scripts/memory-scorecard.sh --self-test
cargo fmt --check
```

If new test names are added, include direct focused invocations for:

- MCP instruction budget
- MCP recall trigger guidance
- SessionEnd summary extraction

## Risks

- Closing PR #23 loses useful work. Mitigate by reimplementing product deltas before or immediately after closure.
- Rebase pulls stale tooling forward. Mitigate with the retired-tooling gate.
- Recall guidance becomes too weak. Mitigate by keeping conditional recall examples.
- Summary extraction drops important content. Mitigate by preserving full content and only improving the concise summary field.

## Implementation Checklist

- [ ] Inspect PR #23 diff against current main.
- [ ] Decide close, rebase, or cherry-pick.
- [ ] Document disposition.
- [ ] Update MCP server instructions.
- [ ] Update recall/context tool descriptions.
- [ ] Add or update MCP instruction tests.
- [ ] Implement clean SessionEnd summary extraction.
- [ ] Add hook summary tests.
- [ ] Confirm retired trace scripts are not restored.
- [ ] Run focused tests.
- [ ] Close or update PR #23 accordingly.
