# Agent Support

rusty-brain is a cross-agentic memory system: the current sprint targets
OpenCode and Codex as first-priority agents alongside Claude Code, with Hermes
as a discovery-gated candidate (see
`docs/prds/2026-06-23-cross-agentic-agent-parity.md`).

The source of truth for per-agent support is the capability matrix in
`crates/rb-agents/src/capability.rs`. The README table is a rendered copy,
guarded against drift by `crates/rb-agents/tests/capability_docs.rs`. Every
agent is scored on four dimensions:

| Dimension | Meaning |
|---|---|
| capture | tool/session events are folded into memories automatically |
| retrieval | relevant memories are injected back into the agent's context |
| config | `rusty-brain-install` can wire the hooks into the agent's config |
| scorecard | `scripts/memory-scorecard.sh` can measure memory value for the agent |

Statuses are honest by design: `partial`/`unsupported`/`unknown` mean exactly
that, and unsupported paths fail with actionable messages rather than silently
succeeding.

## Claude Code (stable)

The lead adapter; fully supported on all four dimensions.

```bash
rusty-brain-install install --agents claude-code            # project scope
rusty-brain-install install --agents claude-code --global   # per-user scope
rusty-brain-install status
```

- Events wired: `SessionStart`, `UserPromptSubmit`, `PostToolUse`, `Stop`,
  `SessionEnd`, `PreCompact`.
- Capture: the per-session scratch folds into one summary at `SessionEnd`.
- Retrieval: `SessionStart` injects session context; `UserPromptSubmit`
  performs prompt-time recall with untrusted-context framing and a token
  budget.
- Scorecard: `scripts/memory-scorecard.sh --agent claude-code --runs 1`.

## Codex (experimental)

```bash
rusty-brain-install install --agents codex            # writes <project>/.codex/hooks.json
rusty-brain-install install --agents codex --global   # writes ~/.codex/hooks.json
```

- Events wired: `SessionStart`, `PostToolUse`, `Stop`, `PreCompact`.
- Capture — **partial**: shell (`Bash`) tool use is captured into the
  per-session scratch, but Codex's native `Stop` stays mapped to canonical
  `Stop` (a no-op boundary), so the scratch is never folded into a summary.
  The terminus mapping is fixture-gated: it will not be promoted until a
  recorded Codex lifecycle fixture proves whether `Stop` fires per-turn or
  per-session (`docs/plans/2026-06-26-cross-cli-terminus-mapping.md`).
  `apply_patch` file edits are additionally upstream-blocked — Codex does not
  emit `PostToolUse` for them
  ([openai/codex#16732](https://github.com/openai/codex/issues/16732), see
  `docs/follow-ups/2026-06-02-codex-apply-patch-capture.md`).
- Retrieval — **unsupported**: the closest equivalent to Claude's
  `UserPromptSubmit` injection is Codex's `SessionStart` context injection
  (returned via `hookSpecificOutput.additionalContext`), which is active.
  Native `UserPromptSubmit` payloads parse to `Other` and are not acted on
  until a recorded fixture verifies their shape. There is no prompt-time
  recall for Codex today; do not assume parity.
- Scorecard — **skipped** with `phase=capture`:
  `scripts/memory-scorecard.sh --agent codex --runs 1` prints a
  machine-readable skip line and exits 0.

## OpenCode (experimental)

- Capture — **supported** and fixture-backed: `session.created` maps to
  canonical `SessionStart`, `tool.execute.after` records tool use (including
  `apply_patch` file edits, with the edited path parsed from the V4A patch
  text), and `session.idle` maps to canonical `SessionCheckpoint`, which
  folds the scratch checkpoint-safely (`session.idle` can fire more than once
  per session).
- Config — **installer deferred, by decision**: OpenCode loads hooks through a
  JS/TS plugin in `.opencode/plugins/`, not a JSON hooks block, so
  rusty-brain's JSON-writing installer would be inert.
  `rusty-brain-install install --agents opencode` therefore fails closed with
  `[E_INSTALL_AGENT_DEFERRED]` instead of pretending to install. The adapter
  itself is fully functional: a plugin that pipes hook payloads to
  `rusty-brain-hooks --agent opencode` gets capture today. The
  fixture-recording plugin under `scripts/fixtures/opencode-logger/` shows the
  wiring; a maintained install path is a follow-on task.
- Retrieval — **unsupported**: no OpenCode prompt-submission event is mapped;
  there is no recorded fixture proving one exists with a stable shape. No
  injection channel is active for OpenCode.
- Scorecard — **skipped** with `phase=config` (blocked on the plugin/config
  path): `scripts/memory-scorecard.sh --agent opencode --runs 1`.

## Gemini (experimental, descoped)

The adapter and installer exist (`SessionStart`, `AfterTool`, `SessionEnd`,
`PreCompress`), but Gemini is descoped from the cross-CLI terminus track
(2026-06-27): its native `SessionEnd` stays on canonical `Stop`, so per-tool
observations are recorded but never folded. No fixture or mapping work is
planned. Scorecard target is skipped with `phase=scoring`.

## Hermes (discovery)

Discovery-gated: no hook names, config paths, or lifecycle semantics are
hard-coded anywhere in the codebase, and no installer path exists. Known
facts, unknowns, and next steps live in
`docs/follow-ups/2026-06-23-hermes-discovery.md`. The scorecard target is
skipped with `phase=config`.

## Validation

```bash
cargo test -p rb-agents -p rb-hooks -p rb-install
scripts/memory-scorecard.sh --self-test

scripts/memory-scorecard.sh --agent claude-code --runs 1
scripts/memory-scorecard.sh --agent codex --runs 1      # explicit skip
scripts/memory-scorecard.sh --agent opencode --runs 1   # explicit skip
scripts/memory-scorecard.sh --agent all --runs 1        # prints skips, runs Claude Code
sh scripts/memory-scorecard.test.sh                     # agent-targeting functions
```

## Troubleshooting

**Hooks don't seem to fire.** Run `rusty-brain-install status` — it reports,
per CLI, whether the CLI was detected and whether our sentinel-marked hook
block is present in its config. Reinstall with
`rusty-brain-install install --agents <id>` (add `--global` for the per-user
config). Hooks are fail-open: a broken hook degrades silently rather than
blocking the agent, so a misconfigured binary path looks like "nothing
happens".

**`[E_INSTALL_AGENT_DEFERRED]` when installing opencode.** Expected: the
OpenCode installer is deliberately deferred (see above). Wire a JS/TS plugin
that invokes `rusty-brain-hooks --agent opencode` instead;
`scripts/fixtures/opencode-logger/` demonstrates the plugin shape.

**Capture is `partial` for my agent — where did my session go?** `partial`
means per-tool observations reach the per-session scratch, but no fold event
fires, so no session summary is written; the scratch ages out after 24h
(`scratch::prune_stale`). This is the documented state for Codex and Gemini
until their terminus events are fixture-verified. Existing memories still
recall and inject normally.

**Scorecard prints one line and exits 0.** That is a machine-readable skip,
not a silent success. Fields: `agent`, `dimension`, `scenario`, `phase` (the
earliest blocked pipeline stage: `capture`, `config`, or `scoring`),
`status=skip`, `reason`, `detail`. A supported target that fails instead
reports per-run failures with agent and phase.

**No memories injected at prompt time.** Prompt-time retrieval is
Claude-Code-only today (see the matrix). For Codex, injection happens at
`SessionStart` only. For OpenCode, no injection channel is active.
