# Agent Support

rusty-brain is a cross-agentic memory system. OMP is the active scorecard
transport and can select configured models; OpenCode and Codex remain
first-priority adapter targets, while Hermes is discovery-gated (see
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

## Shared capture contract

- Hooks continue on handled errors/panics and exit zero. The five-second
  connect/capture timer is cooperative (connection timeout: two seconds), **not
  a whole-process deadline**. Waiting for stdin EOF, synchronous preprocessing,
  filesystem operations, or stdout can stall. Transcript parsing and scratch
  pruning run on blocking workers; shutdown does not join abandoned workers.
- Hook envelopes are capped at 1 MiB plus an oversize sentinel. Namespace
  pins/config/frontmatter files must be regular, whole-file inputs at most 1 MiB;
  oversized/special files are ignored, not prefix-interpreted. Committed `HEAD`
  configuration is capped while collecting Git stdout, including an overflow
  sentinel, and rejected whole if oversized. An invalid nearest `CLAUDE.md`
  does not expose an ancestor override.
- Regular transcript files retain an initial-goal window plus recent evidence:
  at most 1 MiB read including tail-boundary inspection (up to 256 KiB initial
  and 768 KiB tail for larger files), and 4000 selected records. Complete aligned
  tail records survive; only a fragmented first tail record is discarded.
  Omitted, partial, malformed or truncated evidence marks the digest incomplete.
  Skipped-byte counts are explicit; skipped-line counts cover known records only,
  not an unseen middle window.
  Adapter-side omitted bytes/records are unknown from the retained envelope;
  confirmed-fact context and summaries report them as unknown, never as local zeroes.
- PreCompact, SessionCheckpoint and SessionEnd share one fold: a superseding
  activity summary plus independently recallable user-confirmed decisions,
  preferences and constraints. Each digest retains at most 30 confirmed and
  30 other evidence entries. Assistant proposals, bare “yes”, quoted/planning/
  rejected statements and assistant success claims do not become confirmed policy.
  Fenced contents are excluded from confirmation and adoption. Scoped adoption
  requires one complete retained named assistant proposal (`Adopt proposal: subject`
  or `yes, use that for subject`), even when the wider window is incomplete.
  Assistant `Proposal: subject: value` supplies a named proposal, not confirmation.
  Statement-leading proposal rejection remains unconfirmed; labeled policies such as
  `Constraint: review: reject proposals that include secrets` and
  `Correction: auth: reject anonymous requests` remain confirmed.
  Statement-leading `propose`/`I propose`/`we propose` indicates planning;
  proposal terminology and generic planning words inside a labeled fact do not
  remove its confirmation (for example, preserving a `"could not connect"` error).
  Bare task imperatives and `must not`/`should not` substrings alone do not confirm
  policy; use labeled facts or explicit decision/preference/confirmation language.
  `Decision: subject: value` and `Correction: subject: value` permit ordered,
  explicit same-session correction: obsolete retained evidence is not admitted,
  and every known earlier matching-subject fact is retired after the correction
  is durable. Failed retirements retry even when the correction is already recorded.
  Stored corrections still filter obsolete evidence and finish pending retirement
  when their original transcript records leave the next bounded window.
  Matching uses persisted redacted subject/source identities, including replacement lineage.
  Ambiguous conflicts remain unresolved. Compaction custom instructions alone
  have no verified conversational role.
- Source context carries actual message IDs when supplied, record byte offsets
  or OMP branch positions, text-line order and window/status metadata; missing
  IDs are not invented. Attribution is not authority: hook-confirmed user facts
  retain daemon-derived `agent_attested` trust. Mixed activity summaries do not
  claim measured success merely because a command ran; tool outcomes distinguish
  observed success, failure and unknown status.
  Explicit failures retain the first nonblank error/stderr/message/content string;
  otherwise they record `tool reported an error`, including for nonmutation tools.
- A per-session 128-source identity ledger survives end/checkpoint resets.
  Repeated source facts keep their IDs; exhaustion is reported and stops new
  fact admission instead of evicting identities. Scratch remains best-effort
  with overlapping-hook last-writer-wins behavior. Redaction and instruction
  filtering precede persistence; directive-shaped legitimate constraints can
  still be dropped by the retained safety filter.


## Claude Code (stable)

The lead adapter; fully supported for capture, retrieval, and configuration.

```bash
rusty-brain-install install --agents claude-code            # project scope
rusty-brain-install install --agents claude-code --global   # per-user scope
rusty-brain-install status
```

- Events wired: `SessionStart`, `UserPromptSubmit`, `PostToolUse`, `Stop`,
  `SessionEnd`, `PreCompact`.
- Capture: the shared fold stores an activity summary and confirmed user facts.
- Retrieval: `SessionStart` injects session context; `UserPromptSubmit`
  performs prompt-time recall with untrusted-context framing and a token
  budget.
- Scorecard: retired; use `scripts/memory-scorecard.sh --agent omp --model <selector> --runs 1`.

## OMP (experimental)

Install from the directory in which you run OMP:

```bash
rusty-brain-install install --agents omp
rusty-brain-install status --agents omp
omp --model openai-codex/gpt-5.6-luna
rusty-brain-install uninstall --agents omp
```

The installer writes a self-contained `.omp/extensions/rusty-brain.ts`. OMP
discovers this directory in its current working directory, not in ancestors.
The asset embeds the absolute `rusty-brain-hooks` path; keep `rusty-brain` beside
it so session startup can start the daemon. Reinstall after moving the binaries.
The default runtime requires only OMP's Bun runtime and the installed Rust
binaries, not this checkout. `RB_OMP_HOOKS_BIN` overrides the embedded path for
explicit testing. Model selection stays in OMP, not in the extension.

**Ownership:** a SHA-256 comment covers the installed source. Intact old versions
can be upgraded; a changed or unowned file is never silently overwritten or
removed. `status` reports `drifted` and fails for ownership conflicts. Move your
customized file outside the discovery directory before reinstalling, keeping a
backup if needed. Symlinked asset paths are rejected. `--dry-run` validates without
writing. Explicit `--agents omp --global` fails: OMP supports project scope only.
Without `--agents`, global install/status/uninstall select only supported global
installers and skip OMP.

**Lifecycle:**

- `session_start` ensures the daemon is available; startup context is not injected.
- `before_agent_start` returns prompt-relevant memory in a hidden, agent-attributed
  custom message with the shared untrusted-data framing.
- `tool_result` records native `write.path`, hashline `edit.input`/`patch` paths
  (including moves/removals), and shell commands in redacted per-session scratch.
- Before switching or branching, a checkpoint folds the old session without
  clearing scratch. Cancelled transitions are safe; later folds supersede it.
- `session_shutdown` drains queued capture and folds bounded user/assistant
  branch text. The adapter retains the initial goal plus recent evidence under
  a reported 256 KiB JSONL cap, preserving actual entry IDs and branch positions.
  Each entry is capped at 64 KiB so a long final assistant response cannot consume
  the entire recent window before the preceding user turn is considered.
  Partial entries and omitted branch entries are reported; omitted byte count is
  unknown. Its view is conservatively incomplete, not equivalent to a full
  transcript. JSONL travels through stdin, not a temporary transcript file.
  Rust redacts and source-filters before persistence.
- Sessions/factories have independent state. Hooks have a 6.5-second subprocess
  deadline, bounded I/O, and fail open on missing binaries, errors or malformed
  output. A crash/forced kill can bypass shutdown; this is best-effort capture,
  not a durable event journal.

**Verified:** OMP 18.2.4, Bun 1.4.2, separate bounded
`openai-codex/gpt-5.6-luna` and `openai-codex/gpt-5.6-terra` live pairs. Native
project discovery, write capture, shutdown storage, subsequent recall after
archiving the original seed, and placebo isolation passed. See
[`eval/omp-extension-verification.json`](eval/omp-extension-verification.json).
This is transport/lifecycle evidence, not model-quality or scorecard efficacy.

The bounded native-callback fixture exercises the installed asset with real hook
processes, a disposable daemon/store, prompt injection and recall (no live model):

```bash
bun crates/rb-install/tests/omp_capture.ts target/debug/rusty-brain target/debug/rusty-brain-hooks
```

**Troubleshooting:** run `rusty-brain-install status --agents omp` and
`rusty-brain doctor` first. Verify the OMP working directory and binary locations.
`--no-extensions` disables native discovery; explicitly pass
`--extension .omp/extensions/rusty-brain.ts` when using it. If OMP ignores the
asset because of discovery settings or ignore rules, use that explicit path.
There is no MCP server or special memory tool to enable.

The scorecard explicitly loads `scripts/scorecard-omp-extension.ts`, a re-export
of the same shipped runtime, and replays thirteen shared scenario rows. Its
fresh profiles require provider environment credentials or configured broker
environment variables; local profile credentials are not copied. Use concrete
model selectors (or provide the role through `PI_SLOW_MODEL` for `@slow`).
Do not pool outcomes across models. See
[`eval/scorecard-controls.md`](eval/scorecard-controls.md) for receipts and controls.

## Codex (experimental)

```bash
rusty-brain-install install --agents codex            # writes <project>/.codex/hooks.json
rusty-brain-install install --agents codex --global   # writes ~/.codex/hooks.json
```

- Events wired: `SessionStart`, `PostToolUse`, `Stop`, `PreCompact`.
- Capture — **partial**: shell (`Bash`) tool use and `apply_patch` file edits
  are captured into the per-session scratch, but Codex's native `Stop` stays
  mapped to canonical `Stop` (a no-op boundary), so the scratch is never
  folded into a summary. The terminus mapping is fixture-gated: it will not
  be promoted until a recorded Codex lifecycle fixture proves whether `Stop`
  fires per-turn or per-session
  (`docs/plans/2026-06-26-cross-cli-terminus-mapping.md`).
  `apply_patch` capture is live-fixture-verified: Codex emits `PostToolUse`
  for it since 0.123.0 ([openai/codex#16732](https://github.com/openai/codex/issues/16732)),
  carrying the raw V4A patch under `tool_input.command`. Every directive path
  is recorded — `*** Add|Update|Delete File:` plus the `*** Move to:` rename
  destination (a rename records both source and destination), including
  multi-file patches. Parsing is hunk-aware (patch CONTENT inside `@@` hunks
  is never treated as a directive) and paths are vetted (leading `./`
  stripped; empty, absolute, and `..`-traversal paths rejected — V4A paths
  are relative-only by spec). Recorded from codex-cli 0.144.1, see
  `crates/rb-hooks/tests/fixtures/codex/` and
  `docs/follow-ups/2026-06-02-codex-apply-patch-capture.md`.
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

## Agent-agnostic prompt-time recall (CA6)

Recall-before-work is a product capability, not a Claude implementation
detail. The injection contract is defined once, independent of any agent's
event name, in `crates/rb-agents/src/recall_contract.rs`
(`PROMPT_TIME_RECALL`):

1. **Top-k under a budget** — at most 5 memories per prompt, each displayed
   at ≤200 chars (summary-or-first-N-chars, the W3.3 projection rule).
2. **Untrusted-data framing (W2.5)** — the shared data-not-instructions
   preamble precedes every injected block; each memory is quoted and labeled
   with provenance.
3. **Source-aware suppression (W3.3)** — inject nothing when prior context
   is still present (Claude `resume`) and nothing on zero hits.

Per-adapter mapping (the capability matrix records the truth — never silent
parity):

| Agent | Mapped event | `retrieval` |
|---|---|---|
| claude-code | `UserPromptSubmit` (rb-hooks consumes the contract constants directly) | supported |
| omp | `before_agent_start` native extension custom message | supported |
| codex | none — native `UserPromptSubmit` payloads are fixture-gated | unsupported |
| opencode | none — no prompt-submission event fixture exists | unsupported |
| gemini | none — descoped | unsupported |

Agents without a mapped event can still get recall-before-work from their
own tooling via the opt-in loopback HTTP surface (`serve --http`, see the
README "HTTP listener" section and `docs/THREAT_MODEL.md`): `POST /recall`
with the prompt as the query and the required `x-rusty-brain-namespace`
header, then wrap the results per the contract above
(cap the item count, apply the untrusted preamble) before injecting them
into the agent's context. This is a documented integration path, not a
claim of native support — the matrix rows above stay authoritative.

## Validation

```bash
cargo test -p rb-agents -p rb-hooks -p rb-install
scripts/memory-scorecard.sh --self-test

scripts/memory-scorecard.sh --agent omp --model @slow --runs 1
scripts/memory-scorecard.sh --agent codex --runs 1      # explicit skip
scripts/memory-scorecard.sh --agent opencode --runs 1   # explicit skip
scripts/memory-scorecard.sh --agent all --runs 1        # prints skips, runs OMP
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

**`[E_INSTALL_AGENT_DEFERRED]` when installing OpenCode.** Expected: OpenCode
needs a JS/TS plugin and has no persistent installer. The fixture under
`scripts/fixtures/opencode-logger/` demonstrates its plugin shape.

**Capture is `partial` for my agent — where did my session go?** `partial`
means a lifecycle boundary is not fully verified. For Codex, tool observations
reach the per-session scratch but no fold event is fixture-verified. For OMP,
the authorized live fixture verified `session_shutdown` folding.
Gemini remains deliberately descoped. Existing memories still recall and inject
normally.

**Scorecard prints one line and exits 0.** That is a machine-readable skip,
not a silent success. Fields: `agent`, `dimension`, `scenario`, `phase` (the
earliest blocked pipeline stage: `capture`, `config`, or `scoring`),
`status=skip`, `reason`, `detail`. A supported target that fails instead
reports per-run failures with agent and phase.

**No memories injected at prompt time.** Prompt-time retrieval is supported for
Claude Code and OMP (through `before_agent_start`). Codex and OpenCode lack a
verified native prompt channel. Any agent whose tooling can issue an HTTP request
can wire recall-before-work through the opt-in loopback listener; see
"Agent-agnostic prompt-time recall (CA6)" above.
