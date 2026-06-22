# W3.5 prompt-caching study — measuring cached vs uncached input (dimension A) — RETIRED

> **RETIRED 2026-06-21.** The P4a trace/cache instrumentation this study describes
> (`scripts/w35-trace-tools.sh`, `scripts/w35-cache-trace.sh`, and their workflows)
> was **removed** in the W3.5 A/B gate cutover. The cache-economics methodology
> recorded here (ADR-3: score dimension A on `total_cost_usd` + accuracy, never raw
> input tokens) is kept as a record and folds into the dimension-A runner if/when it
> is built on the memory-value scorecard scaffold
> ([`2026-06-16-w35-criterion-redesign.md`](2026-06-16-w35-criterion-redesign.md)).
> File paths below no longer exist.

- **Status:** **RETIRED** (instrumentation removed in the gate cutover; ADR-3 methodology kept as a record). Originally: proposed (investigation + ADR-3; harness landed, measured run deferred).
- **Date:** 2026-06-19.
- **Scope:** resolves the open question that gates dimension **A — Retrieval at scale** in the criterion redesign (`docs/eval/2026-06-16-w35-criterion-redesign.md`, "A only after the caching question is resolved"). This is P4a of the redesign build sequence.
- **Companion harness:** `scripts/w35-trace-tools.sh` (extended: four `tok_*` columns + `aggregate_cache` + `--self-test`).

---

## Why

The criterion redesign flagged a methodological trap before dimension A could be
built. A large CLAUDE.md is loaded once and **cached**: its marginal per-turn
input cost is near-zero (cache read). Memory, by design, re-injects each turn —
the `UserPromptSubmit` hook injects a recall slice that depends on the user's
latest prompt, so it is **dynamic per turn** and is a cache write / uncached
input each time. A comparison that counts raw `input_tokens` would therefore
make memory look *more* expensive than the CLAUDE.md baseline even when its
*billable* cost is comparable. The redesign asked this to be resolved — by
**measuring cached vs uncached input** — before building the dimension.

## Investigation

The data needed to resolve this **already exists** in the output the trace
harness captures. `claude -p --output-format stream-json --verbose` emits one
`assistant` event per turn, and each carries the API `message.usage` object with
four buckets:

- `input_tokens` — uncached input,
- `cache_creation_input_tokens` — input written to the cache this call,
- `cache_read_input_tokens` — input served from the cache (cheap),
- `output_tokens`.

`scripts/w35-trace-tools.sh` already ran both arms (`memory-on`, `claude-md`)
under `stream-json --verbose`, but `emit_row` kept only `num_turns`,
`total_cost_usd`, and tool-call names — it **discarded `message.usage`**. This
study adds the four `tok_*` columns (session sums over assistant events) and a
per-arm `aggregate_cache` table.

### The key realization

**`total_cost_usd` is already cache-adjusted.** The API bills cache reads and
writes at their true rates, so `cost` is the honest, cache-correct cost axis —
it never had the bug. The bug is in *raw token counting*: comparing arms on
`input_tokens` (or any token sum that does not discount cache reads) overstates
memory's per-turn cost, because the CLAUDE.md baseline's input volume is
dominated by cheap cache reads while memory-on's injected slice is full-price
each turn. The four buckets are surfaced **diagnostically** (to explain *why*
costs differ and to show context-window pressure), not as a replacement for
`cost`.

Two derived figures in `aggregate_cache` make the distinction legible:

- **`mean_ctx_vol`** = `in + cc + cr` — total tokens the model consumed as input
  context. This is the context-window-pressure axis, and it is where a large
  CLAUDE.md *legitimately* looks big without being expensive.
- **`mean_eff_in`** = `in + 1.25·cc + 0.1·cr` — cache-weighted full-price input
  equivalents (1.25 ≈ Anthropic's 5-min cache-*write* premium; 0.1 ≈ cache-*read*
  fraction). Weighting `cc` at 1.25× matters because memory-on's hypothesis is high
  per-turn cache *writes*; leaving `cc` at parity would systematically understate
  its cost relative to claude-md (a directional bias in the metric whose purpose is
  to catch directional bias). This should track `total_cost_usd`; if it diverges,
  the 1.25 / 0.1 constants need recalibration for the run's model/cache-TTL.

---

## ADR-3: score dimension A on cost + accuracy, never raw input tokens; report cache buckets diagnostically

**Context.** Dimension A (retrieval at scale) must show memory-on beating the
realistic baseline and at least tying the steelman baseline on a *buried* fact
among a large corpus, with token cost as the secondary axis. The caching
asymmetry (above) means the choice of token metric is load-bearing.

**Decision.**

1. **The cost axis is `total_cost_usd`, not a raw token sum.** Memory-on is
   compared against the dual baselines on correctness (primary) and
   `total_cost_usd` (secondary, cache-adjusted by construction).
2. **Raw input tokens are reported only as the four buckets** (`tok_in` /
   `tok_cache_create` / `tok_cache_read` / `tok_out`) plus the two derived
   figures, **diagnostically**. They are never the basis of a pass/fail
   comparison between arms, because they double-count cheap cache reads.
3. **The corpus is cranked past comfortable context** (≥500 facts, per the
   redesign) so the *accuracy* edge on a buried fact actually bites — that is
   where memory's value over a single cached CLAUDE.md is real, and it is the
   primary axis for a reason.
4. **`mean_eff_in` is a sanity check on cost**, not a tiebreaker: if it and
   `total_cost_usd` disagree on which arm is cheaper, the cache constants (1.25
   write / 0.1 read) are wrong for the model/TTL and are recalibrated before
   reading the table. Note both multipliers are *systematic* — a wrong value
   biases all arms in the same direction and so does NOT surface as a
   disagreement; treat `mean_eff_in` as approximate and `total_cost_usd` as truth.

**Consequences.** The dimension-A runner that inherits the shared scaffold
(redesign build-sequence step 1) must emit the `tok_*` buckets; the scorecard's
"token cost" axis means `total_cost_usd`. The harness change lands now so A's
runner inherits it rather than retrofitting.

**Methodology re-homed in the scorecard, 2026-06-21.** Although the trace/cache
instrumentation above was retired in the gate cutover, the ADR-3 methodology it
recorded was not lost — it now lives in the unified memory-value scorecard.
`scripts/memory-scorecard.sh` runs every session under `--output-format
stream-json --verbose`, records `total_cost_usd` + the four `tok_*` buckets into a
13-field TSV, and for the `retrieval_scale` dimension prints the cache diagnostics
(`cache%`, `ctx_vol` = in+cc+cr, `eff_in` = in+1.25·cc+0.1·cr) plus a SINGLE
per-dimension RATIFY-Opt-3 / Opt-2 / descope verdict (memory-on vs steelman on
accuracy AND `total_cost_usd` within 20%; the verdict fails closed — it SKIPs on
session errors or zero/absent cost). The scorecard pools all `retrieval_scale`
scenarios into one dimension cell (no per-corpus ladder — the retired prototype's
corpus ladder did not carry over; cost is averaged across the corpus sizes). The
Class A scenarios (`corpus_size` 500/500/1000) live in
`crates/rb-eval/scorecard/memory_scorecard_scenarios.json`; their 500+ distractors
are bulk-planted via the new `rusty-brain remember --batch` (one process for the
whole corpus). All of this is exercised by `--self-test` (no API); the measured
run (real sessions at N≥5) stays deferred on a key + spend.

**Deferred to the measured run (this is the "low spend" boundary).**

- The `ANTHROPIC_API_KEY` repo secret + a small spend budget (the same one secret
  that unblocks the W3.5 nightly eval, the W3.2 measured scorecard, and the
  nightly claude-smoke).
- The ≥500-fact scale corpus so the retrieval edge bites (content authoring,
  no spend).
- **Verification of the `message.usage` JSON path against live output.** The
  parser is defensive (missing fields read `0` via `// 0`); the exact path
  (`select(.type=="assistant") | .message.usage`) is confirmed against the
  Anthropic Messages API usage shape but not yet against a real Claude Code
  stream in this environment. First live run is the verification step — a
  divergence shows up immediately (all-zero buckets), so it cannot silently
  mislead.
- Model/TTL-specific cache-pricing for the `0.1` constant (haiku placeholder).

## Harness change (landed — then removed in the 2026-06-21 cutover)

*(Historical: the harness below was deleted in the gate cutover; described in past sense.)*

- `emit_row` appends `tok_in`, `tok_cache_create`, `tok_cache_read`, `tok_out`
  (session sums from `.message.usage`), via a pure `sum_usage` helper.
- `aggregate_cache` prints per-arm mean buckets + `mean_ctx_vol` + `mean_eff_in`.
- `--self-test` (CI-safe, no API) validates `sum_usage`, the `emit_row` column
  wiring (incl. the missing-field `// 0` fallback), and the
  `eff_in`/`ctx_vol` arithmetic over a synthetic stream-json fixture.
- No protocol/contract change, no binary change. Output remains model-free-text
  free (token counts + tool names only), so it stays a safe CI artifact.

## Validation for this step (historical — the script below was removed in the 2026-06-21 cutover)

```
scripts/w35-trace-tools.sh --self-test   # PASS, no API (script since removed)
```

## Open questions for the measured run

1. Does memory-on's per-turn `UserPromptSubmit` injection actually bust the cache
   each turn (hypothesis: yes — it is per-prompt dynamic; the buckets will
   confirm by showing high `cache_creation` / low `cache_read` on memory-on vs
   the inverse on `claude-md`)?
2. Corpus size at which the retrieval-accuracy edge over a cached CLAUDE.md
   becomes statistically separable at N ≥ 5 runs.
3. Whether the `mean_eff_in` ↔ `total_cost_usd` ratio holds for haiku's cache
   pricing (calibrates the `1.25` write / `0.1` read constants; haiku vs a
   1-hour-cache TTL would shift the write multiplier to ~2.0).
