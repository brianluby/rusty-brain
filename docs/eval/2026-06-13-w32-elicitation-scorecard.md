# Elicitation scorecard (W3.2)

- **Status:** Harness landed; measured ≥10-session run **DEFERRED** (needs
  `claude` + `ANTHROPIC_API_KEY` + a small spend budget — the same infra plan §6
  scopes to the W3.5 nightly eval). Tracked per release once that infra runs.
- **Date:** 2026-06-13
- **Gate clause (§6 / W3.2):** "K≥10 scripted sessions where the user states a
  decision with no mention of memory; pass = remember fires in ≥70%,
  recall-before-work in ≥50%; tracked per release."
- **Artifacts:** scenarios `crates/rb-eval/scorecard/w32_elicitation_scenarios.json`
  (11 scenarios); runner `scripts/w32-elicitation-scorecard.sh`.

## What W3.2 added (the three elicitation channels)

- **(a) Deterministic recall** — a canonical `UserPromptSubmit` event whose hook
  runs recall on the user's prompt and injects the top hits via
  `additionalContext` under a token budget. Recall no longer depends on the
  model electing to call a tool.
- **(b) Policy** — the installer appends a marker-delimited memory-policy block
  to the project (or global) `CLAUDE.md` and ships a `rusty-brain-memory` skill
  under `.claude/skills/`.
- **(c) Tool surface** — trigger-condition MCP tool descriptions ("Use when the
  user states a decision, preference, or constraint…"), MCP `instructions` in
  `initialize`, and `permissions.allow` entries (`mcp__rusty-brain__*`) so
  headless model-initiated calls never stall on an approval prompt (spike S1).

## What it measures

Each scenario is **two** real headless `claude -p` sessions in a fresh
namespace, with hooks + MCP + the W3.2 channels installed:

1. **PLANT** — the user states a decision / preference / constraint **without
   mentioning memory**. Metric `remember_fired` = the session made a
   model-initiated `mcp__rusty-brain__remember` call (channels **b** + **c**
   nudged the model to capture).
2. **WORK** — a fresh-context session of the **same namespace** performs a task
   where that decision matters. Metric `recall_before_work` = recalled context
   reached the model **before its first edit/command** — via the deterministic
   `UserPromptSubmit` injection (channel **a**) and/or a model-initiated recall.

Channel (a) makes `recall_before_work` near-deterministic (the hook recalls on
every prompt), so the **binding** signal is `remember_fired` — whether the
policy + tool-surface channels move the model to capture unprompted.

### Pass thresholds (per release)

| Metric | Threshold |
|---|---|
| `remember_fired` rate | ≥ 0.70 |
| `recall_before_work` rate | ≥ 0.50 |

## How to run

```bash
# Scoring-logic self-test (no API, CI-safe):
scripts/w32-elicitation-scorecard.sh --self-test

# Full measured run (needs claude + ANTHROPIC_API_KEY + spend):
cargo build --release -p rusty-brain -p rb-hooks -p rb-install
scripts/w32-elicitation-scorecard.sh --bin-dir target/release
```

The runner runs each scenario's PLANT and WORK sessions in **separate `HOME`s**
that share one socket/DB/namespace — so WORK recalls what PLANT stored while each
session's transcript tree stays separate, letting each metric be scoped to the
right session. It installs via the real `rusty-brain-install` (so it exercises the
channel-(c) `permissions.allow` write the prior nightly smoke script had to
hand-patch), drives the two sessions with a cheap model under a hard spend cap,
and detects the two metrics from the per-session transcripts under
`~/.claude/projects/`.

## Detection method (best-effort; refine on first measured run)

- `remember_fired`: the PLANT session transcript JSONL contains a `tool_use`
  naming `mcp__rusty-brain__remember`.
- `recall_before_work`: the WORK session transcript contains the deterministic
  injection block header ("Memories relevant to this prompt") **or** a
  `mcp__rusty-brain__recall` `tool_use`.

These greps are deliberately format-tolerant. The first real run should confirm
how Claude Code records UserPromptSubmit `additionalContext` in the transcript
and tighten the `recall_before_work` detection if needed.

## Notes / honesty

- This is a **behavioral** scorecard (does the model use memory?), complementary
  to the W3.1 decision-grade **content** rubric and the W3.5 A/B **outcome**
  eval. None alone is sufficient; §13/§10 require all three tracked per release.
- The measured run is **not** part of per-PR CI (it is nondeterministic and costs
  API spend). It belongs with the W3.5 nightly arm. Until it runs, the Phase-3
  gate's "elicitation scorecard passes" clause is **owed**, not met — recorded
  here and in §6 so it is not silently declared passed.
