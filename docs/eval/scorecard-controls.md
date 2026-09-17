# Scorecard controls: memory-off and length-matched placebo

## Scope and gate

The live `scripts/memory-scorecard.sh` runner now has five arms:

| Arm | Injected treatment |
|---|---|
| `memory-on` | Existing memory hooks and memory configuration; actual SessionStart and UserPromptSubmit outputs are recorded. |
| `realistic-baseline` | Seeded realistic `CLAUDE.md`, including scenario distractors; no memory hooks. |
| `steelman-baseline` | Seeded steelman `CLAUDE.md`, including scenario distractors; no memory hooks. |
| `memory-off` | No seeded `CLAUDE.md`, no hooks, no memory MCP installation; fresh home and project. |
| `length-matched-placebo` | Neutral punctuation emitted through **both** corresponding hook channels; no memory store, memory MCP installation, or seeded `CLAUDE.md`. |

**This runner still uses the legacy substring proxy**, not assertion precision.
It scores final prose plus size-capped touched files. The existing success/MIE
rules and zero-MIE hard gate remain; `SAFE` is not correctness or causal safety
proof. Complete five-arm runs at the configured minimum sample count can gate;
incomplete/below-minimum runs remain directional. Historical four-arm TSVs lack
the new control and are now incomplete for a five-arm verdict.

The separate [assertion-precision fixture](assertion-precision.md) uses explicit
assertions and **no model judge**. It is **not executed by the live scorecard**,
and equivalent coverage of the old scenarios has **not** been established.
The fixture must not silently replace or retire the substring hard gate.

## How matching works

`scripts/scorecard-controls.py` instruments the installed memory-on command hooks.
The original command still runs on the actual agent event, with its actual input,
project, home, and namespace. Its JSON output is recorded outside the work
project and its stdout is forwarded unchanged. Preflight diagnostic probes and
planted-store size are **not** the source of the placebo budget.

After memory-on finishes, the placebo is built from these actual emissions:

1. Read each SessionStart and UserPromptSubmit output in channel order.
2. Keep `hookEventName`, `continue`, and `suppressOutput` routing/control fields.
3. Replace the entire `hookSpecificOutput.additionalContext` with inert
   punctuation and spaces at the same **estimated** token size. No source memory
   text, IDs, expected answers, stale answers, or instructions are copied.
4. Check generated padding against the scenario's expected, forbidden, and stale
   substrings. Choose an alternative punctuation character if needed; fail if
   none is leak-free.
5. Install replay hooks in the placebo's isolated project. They consume actual
   agent events and record their own outputs.
6. Compare the two arms' per-invocation sizes and event counts independently for
   both channels. Missing invocations, unequal sizes, extra failed invocations,
   or observable wrapper failures (including nonzero hook exits and invalid JSON)
   abort the run before any aggregate `SAFE` verdict.

Empty outputs remain empty; they are not filled to an arbitrary target budget.
The harness expects at least one invocation of each configured channel. A
user-facing `systemMessage` is **not** model context in Claude Code and is neither
counted nor replayed (see `rb-agents/src/claude_code.rs::render_output`).
Independent hook-error markers prevent an agent's fail-open handling of observable
wrapper failures from silently certifying an invalid pair. They cannot detect
errors already swallowed inside `rusty-brain-hooks`: a connection failure or
timeout that returns exit 0 with empty context is indistinguishable here from a
legitimate empty result. Matching such empty emissions is not proof of healthy
retrieval.

## Counting and reporting contract

Estimator ID: **`utf8-bytes-div4-ceil-v1`**.

```text
estimated_tokens(text) = ceil(len(text.encode("utf-8")) / 4)
```

The estimator is applied to each emitted `additionalContext` separately, then
summed by channel. A baseline's seeded `CLAUDE.md` is counted before the work
session, including its heading and distractors. `memory-off` is zero on all
three measured channels. The placebo matches the estimator at each actual hook
invocation, not just an aggregate total or a file length in a different channel.

This dependency-free estimator is deliberately documented rather than described
as a tokenizer. **It is not `rb-tokens`' `o200k_base` BPE, is not a token upper
bound, and does not establish exact Claude/model token equality.** In particular,
punctuation-heavy padding may tokenize differently from prose. `rb-tokens`
provides a Rust library rather than a scorecard counting CLI; this implementation
does not add a build dependency, download a tokenizer, or modify a production
crate. A future BPE counter should retain an explicit tokenizer ID and still
state that `o200k_base` is a proxy for Claude.

The stable first 17 TSV columns are preserved. Five columns are appended:

| Column (1-based) | Meaning |
|---|---|
| 18 | Estimated tokens emitted at SessionStart |
| 19 | Estimated tokens emitted at UserPromptSubmit |
| 20 | Estimated tokens in seeded `CLAUDE.md` |
| 21 | Sum of columns 18–20 |
| 22 | Estimator ID |

Each session result prints its sizes. Every reported success/capture-fidelity
rate and ADR-3 success/cache rate carries its arm's
`injected_est[start=…,prompt=…,file=…,total=…]` on the same line. Aggregated sizes
are **arithmetic means of per-run sizes**, using the same rows as their associated
rates. Capture-fidelity annotations describe the corresponding downstream **work
sessions**, not the earlier auto-capture plant session. Repeated accuracy rates
in the ADR-3 comparison also carry each compared arm's sizes. Historical rows
without measurements display `?`, never fabricated zeros.

`--out RESULTS.tsv` also writes `RESULTS.tsv.metadata.json`, naming the estimator,
measurement scope, legacy scorer, retained hard gate, and separate no-judge
fixture. The metadata explicitly records `exact_model_tokens: false`,
`executed_by_this_run: false`, and `coverage_equivalent: false` for the appropriate
fields. Without `--out`, both artifacts are temporary as before.

With `--log-dir DIR` (or existing `RB_SCORECARD_KEEP_LOGS=1`), the strict retention
allowlist additionally keeps:

```text
<scenario>-r<N>/on/injections/{session-start,prompt-time}.jsonl
<scenario>-r<N>/placebo/{work.jsonl,judge.txt}
<scenario>-r<N>/placebo/injections/{session-start,prompt-time}.jsonl
<scenario>-r<N>/placebo/injections/pair-check.json
<scenario>-r<N>/{on,placebo}/injections/error.json  # only on hook errors
```

Pair receipts contain per-invocation estimated sizes, not a model-token claim.
Replay templates, original commands, stores, and arbitrary project files are not
retained. Existing preflight diagnostic evidence remains separate from these
actual-emission receipts.

## Offline verification and TDD evidence

Implementation was developed on `vega.local`, starting at
`d98de4c4f093860ea16a827c83445fed25267748`, on the existing branch. No paid model
call or production-crate change was needed for these controls.

```bash
# Real bash runner + deterministic fake agent/daemon/hook subprocesses:
python3 -m unittest discover -s scripts/tests -p test_scorecard_controls.py -v

# Existing routing tests now also invoke that end-to-end suite:
sh scripts/memory-scorecard.test.sh

# Existing pure judge, variance, safety, capture, and retention tests:
bash scripts/memory-scorecard.sh --self-test
bash -n scripts/memory-scorecard.sh
git diff --check
```

Observed RED → GREEN cycles used the first command, with focused `-k` selections
where noted:

- Initial end-to-end test failed because `length-matched-placebo` was absent;
  the same test passed after actual-hook recording/replay was added.
- Rate/artifact test failed because rows lacked per-channel measurements;
  passed after TSV, metadata, and aggregation reporting were added.
- Missing placebo prompt invocation initially produced `SAFE`; the regression
  now requires a nonzero control-mismatch exit and no aggregate safety verdict.
- `-k receipts` failed because cleanup discarded placebo evidence; passed after
  extending the strict retention allowlist.
- `-k user_facing` caught incorrectly counting UI-only `systemMessage` (120/105
  rather than the fixture's model-context estimates 30/15); corrected counting
  passed. A separate routing assertion caught lost `continue`/`suppressOutput`.
- `-k fail_open` caught an extra failed hook being ignored by an otherwise
  successful agent; independent error markers now prevent certification and are
  retained for diagnosis.

The nine end-to-end tests also cover explicit and auto-capture planting,
identity-B/reach scoring, empty prompt output, Unicode payloads, preflight versus
actual-emission differences, two-run aggregation with different injected sizes,
no answer/stale leaks, a hook-free memory-off arm, and preservation of the legacy
stale-substring `UNSAFE` gate. They test executed subprocess behavior, not source
string presence. Synthetic transport outcomes/costs are **test data**, not model
performance evidence.

### Additional real-binary integration check

An isolated offline check also used the built **real installer, daemon, SQLite
store, CLI and hook binaries** with a temporary HOME, DB and socket. A seeded
HTTP-crate fact appeared in both wrapped injection channels; placebo output
contained neither the current nor stale token, and `validate-pair` passed.
Memory-on and placebo each emitted estimated sizes **225 SessionStart + 196
UserPromptSubmit = 421 total**. The temporary daemon was terminated afterward.

Claude Code was not on the test PATH: the first installer attempt reported
`not_found`; the successful check stubbed **only its executable-presence detector**
with `/usr/bin/true`. No Claude session or model ingestion was simulated as real.
The installed hook commands and their daemon/store responses were real.
Receipt summary: `crates/rb-eval/evidence/assertion-precision/real-hook-controls-check.json`.

## Remaining limitations

- No live Claude session was run for this implementation. Receipts prove what
  hook processes emitted, not what a particular Claude version ultimately
  ingested. Live integration remains to be verified with an authorized run.
- The control matches the two memory-hook injection channels under an estimator;
  it is **not** a whole-prompt or whole-session token match. Existing memory-on
  MCP configuration and voluntary tool/CLI retrieval remain available. Their
  schemas, instructions, and tool-result traffic are not counted or matched.
- Auto-capture planting costs/context, provider framing, user prompts, and agent
  built-in instructions are outside the injection measurement. Existing provider
  usage/cache metrics remain separate and are not interchangeable with it.
- Fixed arm ordering, model variability, semantic differences between punctuation
  and prose, and the rough estimator limit causal interpretation. A matched
  length is not proof that memory content alone caused an outcome.
- Existing MIE classification still uses its preflight diagnostic probes; these
  are not retroactively relabeled as actual emissions by the new receipt files.
- The locally authored assertion fixture does not imply complete scenario,
  assertion, generalization, or production coverage. The old hard gate remains.
