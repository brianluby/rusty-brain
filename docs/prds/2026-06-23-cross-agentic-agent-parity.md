# PRD: Cross-Agentic Agent Parity for OpenCode, Codex, and Hermes

## Status

New sprint task, draft. This is the task requested on 2026-06-23 after confirming the current sprint is still Claude-heavy.

## Owner Area

Primary: agent adapters, installer/configuration surface, hook lifecycle, prompt-time retrieval, and scorecard coverage.

Touchpoints:

- `crates/rb-agents/src/cli.rs`
- `crates/rb-agents/src/opencode.rs`
- `crates/rb-agents/src/codex.rs`
- `crates/rb-agents/src/event.rs`
- `crates/rb-hooks/src/cli.rs`
- `crates/rb-hooks/src/capture.rs`
- `crates/rb-install/src/engine.rs`
- `crates/rb-install/src/installers/`
- `scripts/memory-scorecard.sh`
- `docs/plans/2026-06-02-rusty-brain-p4-broader-agent-surface.md`
- `docs/follow-ups/2026-06-13-cross-cli-capture-inversion.md`

## Problem

The repo has adapter support for multiple CLIs, but the highest-value workflows are still centered on Claude Code semantics:

- `SessionEnd` capture fold
- `UserPromptSubmit` prompt-time retrieval
- plugin/config assumptions
- Claude-only scorecard and smoke runners
- docs and validation commands that default to Claude paths

OpenCode and Codex need to become first-class targets. Hermes should be added as a discovery and integration target, but no Hermes hook names or lifecycle semantics should be invented until verified.

## Relationship to Other PRDs

This PRD is broader than the non-Claude capture regression PRD.

- Non-Claude capture regression: restores scratch flush behavior for Gemini/Codex/OpenCode.
- Codex `apply_patch`: captures one specific Codex tool event once upstream/local readiness is proven.
- Cross-agentic parity: makes capture, retrieval, config, docs, and scorecards agent-aware for OpenCode, Codex, and Hermes-first discovery.

## Goals

- Make OpenCode and Codex first-class rusty-brain memory workflow targets.
- Define an explicit adapter capability model for capture, retrieval, configuration, and validation.
- Audit W3.1, W3.2, and W3.5 flows for Claude-specific assumptions.
- Add scorecard target selection by agent.
- Add Hermes discovery scaffolding without speculative constants.
- Preserve existing Claude Code behavior and validation.
- Document current support levels and known limitations.

## Non-Goals

- Do not remove or reduce Claude Code support.
- Do not fully implement Hermes without verified integration details.
- Do not include Gemini as a primary sprint target unless shared changes make it cheap and safe.
- Do not redesign the memory schema or ranking model.
- Do not build a new plugin framework unless current abstractions block parity.
- Do not bundle unrelated memory quality improvements.

## Functional Requirements

### CA1. Capability Matrix

Introduce or document a capability matrix for each agent:

| Field | Meaning |
| --- | --- |
| `agent` | stable agent id, such as `claude-code`, `opencode`, `codex`, `hermes` |
| `adapter_status` | stable, experimental, discovery, unsupported |
| `capture` | supported, partial, unsupported, unknown |
| `retrieval` | supported, partial, unsupported, unknown |
| `config` | supported, partial, unsupported, unknown |
| `scorecard` | supported, partial, unsupported, unknown |
| `verified_lifecycle_source` | fixture path, docs link, or discovery note |
| `limitations` | short user-facing constraints |

Unsupported or unknown capabilities must produce actionable messages, not silent success.

### CA2. OpenCode Parity

OpenCode must have a clear path for:

- hook payload parsing through `rusty-brain-hooks --agent opencode`
- capture lifecycle support or documented fallback
- prompt-time retrieval support or documented equivalent
- installer/config story, including the known JS/TS plugin requirement
- scorecard coverage or explicit skip with reason

Because existing planning notes say the JSON-writing installer is inert for OpenCode, this task must either implement the plugin path or keep installer support explicitly deferred while still validating the adapter path.

### CA3. Codex Parity

Codex must have a clear path for:

- hook config through `.codex/hooks.json`
- capture lifecycle support or documented fallback
- prompt-time retrieval support or documented equivalent
- `apply_patch` readiness status linked to its own PRD
- scorecard coverage or explicit skip with reason

Codex parity must not assume `apply_patch` support until fixture-gated.

### CA4. Hermes Discovery

Hermes must be treated as discovery-gated.

Required output:

- a discovery note under `docs/follow-ups/` or `docs/prds/` stating known facts, unknowns, and next steps
- no new hard-coded Hermes event names without recorded evidence
- no installer path that writes speculative config
- optional `AgentId::Hermes` only if the code needs an explicit discovery/unsupported status and tests prove unsupported behavior is clear

If no reliable public spec or local fixture is available, the correct sprint outcome is documented discovery status, not fake support.

### CA5. Scorecard Target Selection

Extend scorecard tooling to support agent targets:

```bash
scripts/memory-scorecard.sh --agent claude-code
scripts/memory-scorecard.sh --agent codex
scripts/memory-scorecard.sh --agent opencode
scripts/memory-scorecard.sh --agent all
```

If the exact flag shape differs, it must still support:

- individual agent run
- all first-priority agents run
- clear skip output for unsupported capabilities
- per-agent failure reporting

Scorecard output must identify:

- agent id
- dimension
- scenario
- phase: capture, retrieval, config, or scoring
- failure reason

### CA6. Prompt-Time Retrieval

For each first-priority agent, define whether it can support an equivalent to Claude's `UserPromptSubmit` injection.

If supported:

- use the same retrieval/ranking path
- use the same untrusted-context framing
- preserve token budget constraints

If unsupported:

- document closest equivalent
- expose clear status in capability matrix
- avoid pretending parity exists

### CA7. Configuration and Docs

Docs must include:

- supported agents and capability matrix
- OpenCode setup path and current installer limitation
- Codex setup path
- Hermes discovery status
- validation commands per agent
- troubleshooting for missing hooks, unsupported lifecycle phases, and scorecard skips

## Acceptance Criteria

- Capability matrix exists in docs or generated status output.
- OpenCode capture and retrieval capabilities are verified or explicitly documented as partial/unsupported with next steps.
- Codex capture and retrieval capabilities are verified or explicitly documented as partial/unsupported with next steps.
- Hermes has a discovery record with no invented hook names.
- Scorecard can run Claude Code, OpenCode, and Codex targets separately or can explain an unsupported target in machine-readable output.
- Scorecard can run all first-priority targets together without hiding skips.
- Existing Claude Code scorecards and hook tests still pass.
- Non-Claude capture regression PRD is linked as a dependency or subtask.
- Codex `apply_patch` PRD is linked as a gated subtask.
- Documentation names the current sprint as cross-agentic rather than Claude-only.

## Verification

Run after implementation:

```bash
cargo test -p rb-agents
cargo test -p rb-hooks
cargo test -p rb-install
scripts/memory-scorecard.sh --self-test
```

Then run the available scorecard targets:

```bash
scripts/memory-scorecard.sh --agent claude-code --runs 1
scripts/memory-scorecard.sh --agent codex --runs 1
scripts/memory-scorecard.sh --agent opencode --runs 1
scripts/memory-scorecard.sh --agent all --runs 1
```

Unsupported targets must produce clear skip output rather than ambiguous success.

## Metrics

- 100 percent pass rate for existing Claude Code tests after changes.
- OpenCode has verified or explicitly unsupported status for capture, retrieval, config, and scorecard.
- Codex has verified or explicitly unsupported status for capture, retrieval, config, and scorecard.
- Hermes has documented discovery status with verified facts and unknowns.
- Zero hidden Claude-only assumptions in shared W3.1/W3.2/W3.5 runner paths.
- Scorecard failures include agent and phase.

## Risks

- OpenCode plugin integration is larger than expected. Mitigate by separating adapter validation from installer support.
- Codex lifecycle differs from Claude. Mitigate with fixture-gated mappings and clear capability status.
- Hermes details remain unavailable. Mitigate by treating discovery as a valid sprint deliverable.
- A generic abstraction becomes too broad. Keep the contract narrow: capture, retrieval, config, scorecard.
- Claude behavior regresses. Keep Claude tests and scorecards as release blockers.

## Implementation Checklist

- [ ] Audit W3.1, W3.2, and W3.5 for Claude-specific assumptions.
- [ ] Define capability matrix.
- [ ] Verify OpenCode capture path.
- [ ] Verify OpenCode retrieval path or document unsupported status.
- [ ] Verify Codex capture path.
- [ ] Verify Codex retrieval path or document unsupported status.
- [ ] Link Codex `apply_patch` readiness gate.
- [ ] Decide OpenCode installer/plugin scope.
- [ ] Extend scorecard agent target selection.
- [ ] Add per-agent scorecard output.
- [ ] Add or update tests for unsupported/skipped capabilities.
- [ ] Add Hermes discovery record.
- [ ] Update setup and troubleshooting docs.
- [ ] Run Claude, Codex, and OpenCode validation.
- [ ] Add board/status updates for remaining unsupported gaps.
