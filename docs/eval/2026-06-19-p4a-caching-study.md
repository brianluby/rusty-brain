# P4a — prompt-caching study (investigation + ADR-3)

- **Status:** investigation findings established + harness landed (zero spend).
  `scripts/w35-cache-trace.sh` + `--self-test` + `--aggregate` +
  `.github/workflows/w35-cache-trace.yml` are in place. **ADR-3 is PROVISIONAL**
  — the decision is deferred to a small measured run, with the thresholds that
  pick the option pre-registered below (per the redesign's §P3: "pre-register
  pass thresholds *before* running").
- **Date:** 2026-06-19.
- **Branch / worktree:** `feat/p4a-w35-caching-study` / `rusty-brain-p4a-caching-study`.
- **Blocks:** dimension **A (Retrieval at scale)** of the W3.5 scorecard
  (`docs/eval/2026-06-16-w35-criterion-redesign.md` §A), which names prompt
  caching as the open question that gates its build.
- **Carries over:** ADR-1 (single-fact parity with a fresh file is a non-goal)
  and ADR-2 (the A/B is a cheap proxy; the Phase-5 pilot is ground truth). This
  study does not re-litigate whether memory has value — it resolves the narrow
  cache-economics question that determines *how dimension A must be measured and
  how injection should be shaped*.

## Why

Redesign §A states the blocking question: a large CLAUDE.md is loaded once and
sits in Claude Code's **cached prefix**, so its marginal per-turn input cost is
near-zero after turn 1; memory, by contrast, re-injects every turn. A naive
token comparison therefore makes memory look *worse* than the native baseline
even when its retrieval accuracy is better. Dimension A cannot be built (nor its
scorecard judged) until this is resolved: we need **cached vs uncached input
tokens per arm**, at a corpus size where the retrieval edge actually bites.

This study establishes the measurement architecture, records what is already
known vs what must be measured, and locks the decision thresholds in ADR-3 so
the measured run cannot be re-interpreted to fit a preferred outcome.

## Injection mechanics (established from the code, zero spend)

Both rusty-brain injection channels ride `hookSpecificOutput.additionalContext`
(the only field Claude Code feeds back into the model — see
`crates/rb-agents/src/claude_code.rs:98` and `crates/rb-hooks/src/capture.rs`):

| channel | fires | content | varies per turn? |
|---|---|---|---|
| **SessionStart** (`capture::session_start`) | once per session | budgeted ≤600 tok / ≤10 items; source-aware (`startup`/`clear`→full, `compact`→constraints-only, `resume`→nothing) | **no** — stable across a session unless the store changes |
| **UserPromptSubmit** (`capture::user_prompt_submit`) | every turn | top-`RECALL_INJECT_LIMIT` (=5) recalls for the prompt, ≤200 chars/line | **yes** — different query ⇒ different hits every turn by design |

The cache asymmetry is therefore structural, not accidental:

- **CLAUDE.md** lives in the cached prefix. After turn 1 it is served from cache
  at ~10% of base input cost, regardless of how large it is (until the cache is
  evicted — see U3).
- **UserPromptSubmit recall** is inherently per-turn: its content changes each
  turn, so it can never sit in a cross-turn cached prefix. It is full-price
  input every turn.
- **SessionStart digest** is *potentially* cacheable across turns (stable
  content), **but only if** Claude Code places a cache breakpoint after it
  rather than before it. That placement is the single biggest unknown (U1).

## Investigation findings

- **F1 — stream-json already emits the cache breakdown.** A single-turn
  `claude -p --output-format stream-json --verbose` (probed 2026-06-19) emits the
  full per-turn and aggregate usage, including:
  `usage.input_tokens`, `usage.cache_creation_input_tokens`,
  `usage.cache_read_input_tokens`, `usage.output_tokens`, and
  `usage.cache_creation.ephemeral_{5m,1h}_input_tokens`. It appears in both the
  per-turn `assistant` record (`message.usage`) and the final `result` record.
  **Implication:** the measurement path is to extend the existing trace harness
  — no parallel direct-API rig is required.
- **F2 — the existing harness does not capture cache tokens.** `scripts/w35-trace-tools.sh`
  parses `num_turns`, `total_cost_usd`, and the tool-name sequence only (the one
  `cache_read` reference in the repo is an unrelated eval-corpus id
  `pk_cache_poisoning_incident`). `total_cost_usd` *reflects* cache benefit
  (cache reads are cheaper) but does not decompose it — two arms can post the
  same cost with very different cache dynamics, which is exactly what dimension A
  needs to tell apart.
- **F3 — the cost asymmetry is bounded by construction.** Per-turn recall is
  capped at 5 lines × ≤200 chars plus a fixed `UNTRUSTED_DATA_FRAME` preamble —
  a small, bounded payload (the W3.3 token-accounting test already asserts the
  ≤600-token SessionStart budget; UserPromptSubmit is tighter still). The
  per-turn uncached cost of memory is therefore a small constant, not an
  unbounded one.
- **F4 — measured run blocked, harness work unblocked.** `claude` in this
  environment returns `401 Invalid authentication credentials` for model calls
  (hooks still fire — they are local and need no API auth), and the worktree has
  no built binaries. So the investigation, ADR-3, the harness extension, and its
  `--self-test` are all zero-spend and unblocked; only the final measured
  dispatch waits on auth + a release build.

## Unknowns the measured run must resolve

- **U1 — injection placement vs cache breakpoints.** Does Claude Code place a
  cache breakpoint *after* the SessionStart `additionalContext` (so the digest
  is served from cache on turns 2..N) or *before* it (so it is re-sent at full
  price every turn)? This determines whether the SessionStart channel is cheap
  or expensive at scale, and is the first thing a 2-turn probe settles.
- **U2 — per-arm cache economics across a corpus ladder.** For
  `memory-on` / `realistic-baseline` / `steelman-baseline` / `memory-off` (the
  redesign §P1 four arms), measure `cache_read_input_tokens`,
  `cache_creation_input_tokens`, and `input_tokens` per turn at corpus sizes
  that cross comfortable context (redesign §A: 500+ facts).
- **U3 — the crossover.** At what corpus size does a steelman CLAUDE.md holding
  all facts (a) stop being maintainable, (b) exceed practical prefix size, or
  (c) get evicted from the cache by conversation growth — such that targeted
  recall is net-cheap *even when uncached*? This is the corpus size at which
  memory's retrieval edge is unambiguous.

## ADR-3 — injection shape under prompt caching

**Context:** redesign §A blocks dimension A on the caching question; F1–F4
establish that it is measurable via the existing harness extended with cache
fields; U1–U3 are what the measured run must settle.

**Options:**

- **Opt 1 — per-turn recall only (status quo for UserPromptSubmit).** Accept the
  uncached per-turn cost; rely on the scale crossover (U3) and the bounded
  payload (F3) to keep it net-neutral. Simplest; gives up nothing on retrieval
  quality.
- **Opt 2 — move injection into the cached prefix.** Make the injected set a
  stable digest so it caches across turns. Buys cache-friendliness at the cost
  of per-query targeting (recall no longer follows each prompt). Only sane if
  U1 shows the SessionStart digest is *not* already cached.
- **Opt 3 — hybrid: cache-stable SessionStart digest + tightly-bounded per-turn
  recall (current architecture).** Keep the source-aware SessionStart digest
  sized for cache stability and the per-turn recall at ≤5 items / ≤200 chars.
  Measure that the combined per-turn uncached cost is dominated by the
  retrieval-accuracy win at scale.

**Provisional decision: Opt 3** (the current architecture), ratified or revised
by the measured run. It is chosen *provisionally* because it costs nothing to
hold (the code already implements it) and because F3 shows its per-turn payload
is bounded.

**Pre-registered thresholds (the measured run picks the final option):**

1. If, at corpus ≥ 500 facts, `memory-on` retrieval accuracy ≥ `steelman-baseline`
   accuracy **and** `memory-on` mean `total_cost_usd` is within 20% of `steelman`'s
   → **ratify Opt 3.** `total_cost_usd` is the cache-adjusted cost axis; the
   cache buckets explain why costs differ but are not the pass/fail metric.
2. If `memory-on` accuracy wins but mean `total_cost_usd` is worse than 20%
   → **adopt Opt 2 for the SessionStart
   channel only**, keeping per-turn UserPromptSubmit recall only where U1 + the
   accuracy data show it changes outcomes.
3. If neither accuracy nor economics win at scale → memory's value is
   capture/freshness/reach (ADR-1), not retrieval@scale; **descope dimension A's
   token-cost axis to informational** and keep accuracy-at-scale as the sole
   primary metric.

**Status: PROVISIONAL.** Thresholds locked here before any spend, per redesign
§P3. The final ADR-3 is recorded by appending the measured numbers and the
option they selected to this same doc.

## Measurement architecture (LANDED; zero spend to stand up)

- **Harness:** `scripts/w35-cache-trace.sh` (sibling to `w35-trace-tools.sh` /
  `w35-ab-eval.sh`). Per WORK session it reads the session-AGGREGATE usage from
  the stream-json `result` record — `input_tokens`,
  `cache_creation_input_tokens`, `cache_read_input_tokens`, `output_tokens` —
  alongside `num_turns` / `total_cost_usd`. `--self-test` exercises the
  extraction + ADR-3 verdict math with no API (CI-safe); `--aggregate FILE`
  re-scores a saved TSV (e.g. an uploaded nightly artifact) without re-running.
- **Arms:** the redesign §P1 four — `memory-on`, `steelman-baseline`,
  `realistic-baseline`, `memory-off`. Reuses `w35-ab-eval.sh`'s hermeticity
  (isolated `HOME`, namespace, short `/tmp` socket, confound strip).
- **Planting:** redesign §P2 explicit-plant mode — `memory-on` loads the target
  (importance 8) + N distractors (importance 5) via direct `rusty-brain remember`
  CLI calls (bypasses the lossy auto-summary; isolates a clean retrieval + cache
  signal). Distractors are deterministic per scenario id (`gen_corpus`), off-topic
  (fictional port numbers) so they never collide with an `expect`/`stale` token.
- **Corpus ladder:** 0, 50, 200, 500, 1000 (distractors; +1 shared target, so
  corpus=0 is the single-fact ADR-1 baseline). `steelman-baseline` holds target +
  distractors inline in CLAUDE.md; `realistic-baseline` holds distractors only
  (target omitted — the common "nobody wrote it down" reality).
- **Variance:** N ≥ 5 runs per (scenario, arm, corpus-size) cell (redesign §P3);
  a single-run dispatch is directional only and never gates. (The harness reports
  per-cell means today; median + IQR aggregation is a follow-up — the gate-closing
  run needs N≥5 first, so means are not yet load-bearing.)
- **Verdicts:** `aggregate()` applies ADR-3's thresholds per corpus size
  (`memory-on` vs `steelman-baseline`) and prints RATIFY-Opt-3 / Opt-2-candidate /
  descope per cell, plus the hard gate (zero memory-induced errors).
- **Dispatch:** `.github/workflows/w35-cache-trace.yml` (manual only, never gates
  PRs). Default is a cheap slice (1 scenario × 3-point ladder × 1 run); close the
  ADR-3 gate with a manual `--runs 5 --corpus-sizes "0 50 200 500 1000"` dispatch.
- **Spend guard:** cheap model (`haiku`), hard `--max-budget-usd` per session,
  per-session wall-clock `timeout` (when available) — the existing posture.

## Build sequence

0. **This doc** — lock the findings + provisional ADR-3 + thresholds (no code, no spend). ✅ done.
1. **Harness + `--self-test` + `--aggregate` + dispatch workflow** (code, no spend): cache-token TSV columns, four-arm dual-baseline runner over a corpus ladder, pure-core aggregation, ADR-3 per-corpus verdicts. ✅ done (`scripts/w35-cache-trace.sh` `--self-test` passes; `.github/workflows/w35-cache-trace.yml`).
2. **Smoke** (first spend, tiny): `--self-test` → a 1-scenario × 1-corpus-size dispatch at N=1 to confirm `expect`/cache fields discriminate on real haiku output; recalibrate any noisy cell. ← blocked on `ANTHROPIC_API_KEY` + a release build.
3. **Measured run** (the spend): the full corpus ladder at N ≥ 5. Read the scorecard; apply the pre-registered thresholds.
4. **Finalize ADR-3:** append the numbers and the selected option to this doc; unblock dimension A.

## Notes / honesty

- **Not per-PR CI:** nondeterministic and costs API spend; nightly/dispatch posture with alerting (the `w35-ab-eval.yml` pattern).
- **The probe used to establish F1 401'd on auth** and so cost nothing; it still emitted the full usage schema, which is the only fact F1 depends on. The schema should be re-confirmed on the first authenticated run (step 2) in case a future claude build renames fields.
- **U1 (breakpoint placement) is asserted by no source I could read** — it is a Claude Code implementation detail that must be measured, not assumed. ADR-3's provisional decision explicitly does not depend on U1's outcome; the thresholds route around either result.
- Until the measured run is recorded, dimension A's "retrieval at scale" gate clause is **owed**, not met — recorded here so it is not silently declared passed.
