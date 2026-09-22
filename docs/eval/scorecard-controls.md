# Scorecard controls: OMP prompt-time placebo

## Scope

`scripts/memory-scorecard.sh` is an OMP-native, five-arm runner. It invokes:

```bash
omp --no-extensions \
  --extension scripts/scorecard-omp-extension.ts \
  --mode json --model <OMP_MODEL> -p <scenario prompt>
```

Pass `--model <OMP_MODEL>` (or set `RB_SCORECARD_OMP_MODEL`) to select a model.
Each arm has a fresh home and OMP agent directory: supply provider credentials
through environment variables or `OMP_AUTH_BROKER_*`; the runner does not copy
local credential stores or model aliases. `@slow` needs `PI_SLOW_MODEL` in this
environment. Run separately per selector; do not pool different models.

The scorecard entry point re-exports the canonical installed runtime from
`crates/rb-install/assets/omp-extension.ts`. For persistent project installation,
use `rusty-brain-install install --agents omp`; see [Agent Support](../AGENTS.md).

| Arm | Treatment |
|---|---|
| `memory-on` | The native extension forwards `session_start`, prompt-time recall, tool results, and shutdown to `rusty-brain-hooks --agent omp`. |
| `realistic-baseline` | Scenario-provided realistic `AGENTS.md` plus distractors; no extension. |
| `steelman-baseline` | Scenario-provided steelman `AGENTS.md` plus distractors; no extension. |
| `length-matched-placebo` | A native extension injects neutral punctuation through the same OMP `before_agent_start` custom-message channel. |
| `memory-off` | No extension or seeded `AGENTS.md`; fresh home and project. |

The runner reads the same thirteen legacy rows from
`crates/rb-eval/scorecard/memory_scorecard_scenarios.json`: four freshness,
three retrieval-at-scale, three auto-capture, and three reach scenarios. The
scenario source is therefore scorecard-equivalent to the retired Claude runner;
the model transport and baseline file convention are now OMP/`AGENTS.md`.

The four stale/freshness scenarios use exact delivered-artifact assertions:
`scorecard-answer.txt` must equal the scenario's current value (apart from one
optional trailing newline). Assistant prose is not part of that outcome. Thus a
response that names the current value but ships an obsolete artifact fails. The
remaining legacy dimensions still report their compatibility substring outcome;
the independent exact-ID assertion fixture remains a separate no-judge gate.

The safety result is a paired causal proxy. A pair is `memory_induced` only when
all three conditions hold: the memory-on task failed, the same scenario/run's
memory-off task succeeded, and the stale marker appeared in the memory-on
prompt-time receipt. A both-arms failure is `not_memory_induced`. Missing or
duplicate pairs, session errors, malformed outcomes, and missing receipt evidence
are `unassessable` and fail a complete run closed.

`SAFE` means every required pair and injection receipt was assessable and none
satisfied that rule. `UNSAFE` means at least one pair satisfied the rule or was
unassessable. Neither label proves overall task correctness or strict causality:
model runs are nondeterministic controls, and the receipt limitation below
remains.

## Placebo contract

`before_agent_start` is the only model-visible memory injection channel in this
extension. The memory-on extension writes one JSON receipt per returned custom
message preparation attempt:

```json
{"message":"[rusty-brain recall framing and memories]"}
```

For the placebo, the extension reads the corresponding memory-on receipt and
injects a `.` string with the same estimated size. It never invokes
`rusty-brain-hooks` in placebo mode. Missing source receipts, prohibited padding,
extra/missing invocations, or a `control-error.json` cause
`scorecard-controls.py validate-pair` to fail before aggregation. This prevents
placebo fallback from leaking a memory-on recall message.

Receipts append in preparation order. OMP can re-enter policy preparation or
discard an attempt after another extension changes policy or cancels delivery.
These are **not provider-delivery receipts**. The proxy proves that the native
memory channel prepared stale content for injection, not that the provider
consumed or attended to it.

## Counting and output

Estimator ID: **`utf8-bytes-div4-ceil-v1`**.

```text
estimated_tokens(text) = ceil(len(text.encode("utf-8")) / 4)
```

The TSV's first thirteen columns remain stable. Columns 14-17 retain Class B
capture diagnostics, and columns 18-22 retain injection estimates:

| Column | Meaning |
|---:|---|
| 18 | Session-start injection estimate: always `0` for OMP. |
| 19 | Sum of prompt-time custom-message estimates. |
| 20 | Seeded `AGENTS.md` estimate. |
| 21 | Sum of columns 18–20. |
| 22 | Estimator ID. |

The causal postprocessor appends four fields without repurposing earlier data:

| Column | Meaning |
|---:|---|
| 23 | `injected_stale_evidence`: `0`, `1`, `unknown`, or `na`. |
| 24 | Injection evidence reason code. |
| 25 | `causal_attribution`: `memory_induced`, `not_memory_induced`, `unassessable`, or `na`. |
| 26 | Causal reason code, including `both_arms_failed`, `stale_not_injected`, `missing_memory_off_pair`, and receipt/session errors. |

This is a byte-based proxy, not provider token accounting. Usage is summed over
every assistant call in OMP's terminal `agent_end`, rather than taking only the
last call. Provider framing, user prompts, tool traffic, plant sessions, and the
model's own tokenization are outside placebo matching.

`--out RESULTS.tsv` writes `RESULTS.tsv.metadata.json`: model selector, OMP
version, canonical extension SHA-256, estimator, the exact causal rule,
SAFE/UNSAFE definitions, residual limitations, and independent assertion-fixture
status. Resolved provider/model fields remain in each retained `work.jsonl`; use
`--log-dir` to retain those records and receipts. Retained `outcome.json` files
include the paired memory-off outcome, attribution, reason, and causal rule.
The assertion fixture is not executed by scorecard runs and remains explicitly
`coverage_equivalent: false`: it is an offline retrieval assertion, not an
agent-task evaluator.

## Verification

```bash
bash -n scripts/memory-scorecard.sh
bun test scripts/tests/omp-extension.test.ts
bash scripts/memory-scorecard.sh --self-test
python3 -m unittest discover -s scripts/tests -p test_scorecard_controls.py -v
sh scripts/memory-scorecard.test.sh
```

These commands use no model. Bun runs the extension with a deterministic host
and real fixture subprocesses: actual stdin, message/receipt order, source bounds,
timeouts, fail-open paths, strict zero-hook placebo lifecycle, session isolation,
and capture/checkpoint ordering. Shell/Python checks cover scoring and receipt
accounting, not native host lifecycle.

Separate bounded live evidence is recorded in
[`omp-extension-verification.json`](omp-extension-verification.json):

| OMP 18.2.4 model | Memory-on | Placebo | Reported on / placebo USD |
|---|---|---|---|
| `openai-codex/gpt-5.6-luna` | Recalled codename; native write and shutdown capture stored it | `UNKNOWN`; matched receipt; no new capture | 0.0004366 / 0.000417 |
| `openai-codex/gpt-5.6-terra` | Native project discovery; recalled Luna's captured decision after the seed was archived | `UNKNOWN`; matched receipt; no new capture | 0.004934 / 0.003790 |

Each session had a 60-second deadline. These were synthetic transport probes
using a deterministic 512-dimensional embedding provider, **not full scorecard
runs or semantic retrieval validation**. One pair per model is not an efficacy
claim. The installed-asset smoke additionally verified an escaped binary path,
redaction in actual DB/WAL bytes, status, and uninstall.

## Limits

- Matching estimated punctuation length does not establish equal provider tokens
  or prompt placement.
- A prompt-time receipt proves preparation by the injection channel, not provider
  delivery, model attention, or causal equivalence.
- The paired memory-off differential is a causal proxy across nondeterministic
  runs, not a randomized causal estimate.
- Legacy non-freshness dimensions still use compatibility substring outcomes;
  the stale safety scenarios use exact artifact equality.
- Exact-ID retrieval assertions do not constitute semantic task grading.
- Capture is best-effort: a forced kill can bypass shutdown, and transcript/input
  caps can omit content. These probes certify neither crash recovery nor other
  OMP versions.
