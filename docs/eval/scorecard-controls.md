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
| `memory-on` | The native extension forwards `session_start`, prompt-time recall, tool results, and shutdown to `rusty-brain-hooks --agent omp`. OMP does not inject the SessionStart hook response; the harness still records that response diagnostically so a scale claim cannot depend on startup-visible evidence. |
| `realistic-baseline` | Scenario-provided realistic `AGENTS.md` plus same-domain competitors; no extension. |
| `steelman-baseline` | Scenario-provided steelman `AGENTS.md` plus same-domain competitors; no extension. |
| `length-matched-placebo` | A native extension injects neutral punctuation through the same OMP `before_agent_start` custom-message channel. |
| `memory-off` | No extension or seeded `AGENTS.md`; fresh home and project. |

The runner reads the same thirteen legacy live scenarios: four freshness, three
retrieval-at-scale, three auto-capture, and three reach. Their historical
substring metric remains report-only and excludes assistant prose; it cannot
produce the hard result. Live agent-task correctness is explicitly unsupported
where no deterministic outcome assertion exists.

The four stale/freshness scenarios use exact delivered-artifact assertions:
`scorecard-answer.txt` must equal the scenario's current value (apart from one
optional trailing newline). Assistant prose is not part of that outcome. Thus a
response that names the current value but ships an obsolete artifact fails. The
remaining legacy dimensions still report their compatibility substring outcome
from the designated `scorecard-answer.txt` artifact, which their work prompts
explicitly request. Arbitrary workspace files, seeded `AGENTS.md`, and harness
diagnostics are not scoring inputs. Artifact reads are bounded, confined to
the project, and checked for changes during the read; symlinks are excluded.
The independent exact-ID assertion fixture remains a separate no-judge gate.

The safety result is a paired causal proxy. A pair is `memory_induced` only when
the memory-on task failed, the same scenario/run's memory-off task succeeded,
a stale-record-only marker appeared in the memory-on prompt-time receipt, and
the memory-off control is assessable and clean. The runner explicitly disables
extensions for the memory-off arm; its `0` / `extension_disabled` evidence
records that launch configuration, not a fabricated prompt-delivery receipt.
An unknown or contaminated control cannot support attribution.
Each freshness fixture requires the marker to be present in the predecessor
and absent from the current superseding record, so a current migration note
that merely names the old value is not stale evidence. A both-arms failure with
assessable controls is `not_memory_induced`. Missing or duplicate pairs, session
errors, malformed outcomes, and missing receipt evidence are `unassessable` and
fail a complete run closed.

`SAFE` means every required pair and injection receipt was assessable and none
satisfied that rule. `UNSAFE` means at least one pair satisfied the rule or was
unassessable. Neither label proves overall task correctness or strict causality:
model runs are nondeterministic controls, and the receipt limitation below
remains.
Before launching live arms, the runner executes the offline
`assertion-precision` binary and validates its schema-2 JSON independently. The
validator recomputes missing, extra, and duplicate IDs, atomic query/case
verdicts, aggregate totals, the fixed three required case IDs, and
`judge_used = false`. This exact-ID result is the always-on hard gate; a complete
live run must also pass the paired causal safety gate.

This validator trusts the local producer: the report must come from the
just-built `assertion-precision` execution in this job, never external input.
Internal consistency and a recorded SHA-256 do not authenticate an artifact or
prove execution. The checkout, binary, and artifact directory are trusted;
the hashes identify evidence for later comparison, not producer attestation.

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
`scorecard-controls.py validate-pair` to fail that scenario. Completed rows are
retained, remaining scenarios still run, and aggregation forces `RUN-FAIL`.
This prevents placebo fallback from leaking a memory-on recall message without
discarding other scenarios' evidence.

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

The TSV's first thirteen columns retain their layout and order, not necessarily
their scoring semantics. Columns 14-17 retain Class B capture diagnostics,
columns 18-22 retain injection estimates, and columns 23-24 carry answer-source evidence:

| Column | Meaning |
|---:|---|
| 18 | Session-start injection estimate: always `0` for OMP. |
| 19 | Sum of prompt-time custom-message estimates. |
| 20 | Seeded `AGENTS.md` estimate. |
| 21 | Sum of columns 18–20. |
| 22 | Estimator ID. |
| 23 | Expected answer present in the diagnostic SessionStart hook context (`0`/`1`; `na` outside memory-on). |
| 24 | Expected answer present in prompt-time/query recall (`0`/`1`; `na` outside memory-on). |

Class A reports SessionStart-present, prompt-recall-present, and query-only rows
separately. A response-level success supports the retrieval-at-scale claim only
when column 23 is `0` and column 24 is `1`. Historical rows lacking these fields
are not valid input to the current gate; analyze them with their original schema.

The runner emits columns 25-26 and the causal postprocessor appends columns
27-28. It can upgrade column 7 to `1`, but never erase an existing failure flag,
including when the pair is unassessable. Attribution status distinguishes a
causal finding from a retained legacy flag. Aggregation requires exactly 28
fields; attribution input requires exactly 26. Known unsupported-agent skip
records are separate from these session rows.

| Column | Meaning |
|---:|---|
| 25 | `injected_stale_evidence`: `0`, `1`, `unknown`, or `na`. |
| 26 | Injection evidence reason code. |
| 27 | `causal_attribution`: `memory_induced`, `not_memory_induced`, `unassessable`, or `na`. |
| 28 | Causal reason code, including `both_arms_failed`, `stale_not_injected`, `missing_memory_off_pair`, and receipt/session errors. |

No assertion data is squeezed into or appended to those row fields. The
schema-versioned exact evidence report is `RESULTS.tsv.assertions.json`; per-query
ID arrays remain structured separately from session rows.

This is a byte-based proxy, not provider token accounting. Usage is summed over
every assistant call in OMP's terminal `agent_end`, rather than taking only the
last call. Provider framing, user prompts, tool traffic, plant sessions, and the
model's own tokenization are outside placebo matching.

`--out RESULTS.tsv` also writes `RESULTS.tsv.metadata.json` and
`RESULTS.tsv.assertions.json`. Metadata schema 4 records the model selector, OMP
version, canonical extension SHA-256, estimator, assertion-report hash, exact
required/all-case counts, explicit no-judge status, exact causal rule,
SAFE/UNSAFE definitions, and residual limitations. Resolved provider/model
fields remain in retained `work.jsonl`; use `--log-dir` to retain those records
and receipts. Retained `outcome.json` files include the paired memory-off
outcome, attribution, reason, and causal rule.

`RESULTS.tsv.scenario-status.tsv` records each attempted scenario and its exit
status (`0` for completion). A scenario failure cannot produce a passing verdict,
even when the remaining live data is only directional. The workflow uploads
this status sidecar alongside the TSV, assertions, and metadata on every run.

The assertion sidecar is executed by each scorecard run. The hard assertion
result is `ASSERTION-PASS` only when all three fixed required exact-ID cases
pass. Non-required hard-negative/topic-drift cases remain visible through
`passed_cases`, `total_cases`, and `all_cases_passed`; they are never averaged
away or silently called green. A complete live run must also satisfy the paired
causal safety gate.

## Verification

```bash
cargo build --release -p rb-eval --bin assertion-precision
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
- The report-only live substring proxy is not exact-ID evidence or semantic task
  grading. Assistant prose alone cannot make it a hit, and it never controls
  `ASSERTION-PASS`.
- Capture is best-effort: a forced kill can bypass shutdown, and transcript/input
  caps can omit content. These probes certify neither crash recovery nor other
  OMP versions.
