# W3.5 memory-value scorecard — implementation plan (full C + A + B + R)

- **Status:** Active execution plan (covers the remainder of the redesign in
  `docs/eval/2026-06-16-w35-criterion-redesign.md`; supersedes the gate criterion
  in `docs/eval/2026-06-14-w35-ab-eval.md`, which is retired by this work).
- **Date:** 2026-06-19
- **Tracking:** Vikunja project `rusty-brain` → Task #2 (with phase subtasks
  P0–P7). Closes GH #19.
- **Scope decision:** build the full scorecard (C + A + B + R) and **replace +
  retire** the old 3-arm `w35-ab-eval` gate.

## Current state (why the plan is shaped this way)

Steps 0–2 of the redesign's build sequence are **already done and committed**
(PR #25 + follow-ups `6ca053d`→`da22f0e`; working tree clean):

- **Step 0–1 (scaffold): done.** `scripts/memory-scorecard.sh` (358 lines)
  ships the pure `judge_text`, `aggregate_scorecard` (median turns + the
  dual-baseline verdict — beats-realistic-AND-ties-steelman — and the
  safety-only hard gate: zero memory-induced errors), and a `--self-test`
  with a re-rig guard.
- **Step 2 (Class C / freshness): done.** 4 scenarios in
  `crates/rb-eval/scorecard/memory_scorecard_scenarios.json`. The `remember
  --supersedes` blocker once flagged in the `plant_explicit` comment is
  **already resolved** — `rusty-brain remember --supersedes` exists
  (`crates/rusty-brain/src/cli.rs:105`, `crates/rusty-brain/src/run.rs:110`);
  that comment is stale and gets cleaned up in P0.

So the genuine remainder is: finish the P3 variance protocol, cutover CI,
validate C with real spend, then build A / B / R.

## Sequencing at a glance

```
P1 variance (no spend) ──┐
P2 cutover workflow ─────┼─→ P3 validate C (spend) ──→ P5 Class B ──→ P7 closeout
P4a caching study (low spend) ──────────────→ P4 Class A ─────────┘
                                                              P6 Class R (stretch) ──→
```

**Hard rule:** do NOT enable the nightly `schedule:` cron until P3 validates C
green — otherwise the new harness flaps open/closed exactly like the old gate
(GH #19).

---

## Phase 0 — Verify committed baseline (no code, ~10 min)  · subtask P0

- Run `scripts/memory-scorecard.sh --self-test` → must print `self-test PASS`.
- One-off local check that `rusty-brain --json remember "x" --type insight`
  emits `{"id": ...}` — the `plant_explicit` supersede chain at
  `scripts/memory-scorecard.sh:288` depends on it. If absent, that's a real
  blocker to fix first.
- Fix the stale comment at `scripts/memory-scorecard.sh:268-273` ("the CLI has
  no `remember --supersedes`") — it does.

## Phase 1 — Finish P3 variance (code, no spend)  · subtask P1

The redesign mandates "median + spread (IQR or bootstrap CI), never a bare
mean". Today `aggregate_scorecard` has median turns but no spread, and success
is a bare rate.

- Extend `aggregate_scorecard` (`scripts/memory-scorecard.sh:62`): add **IQR**
  for turns and a **Wilson/bootstrap interval** for success rate; print
  `median [Q1–Q3]` / `rate [lo–hi]`.
- Add `--min-runs` (default 5) and emit `DIRECTIONAL ONLY — N < min_runs` when
  under; a sub-threshold run must not print PASS/SAFE verdicts (P3: single runs
  never gate).
- Pre-register thresholds in `memory_scorecard_scenarios.json` `config`
  (`min_runs`, `tie_margin`, `steelman_tie`).
- Extend `--self-test` with IQR + Wilson cases.

## Phase 2 — Cutover nightly (replace + retire old; no spend)  · subtask P2

- **Add** `.github/workflows/memory-scorecard.yml`, cloned from
  `.github/workflows/w35-ab-eval.yml`: `schedule` + `workflow_dispatch`, macOS,
  secret preflight, build `-p rusty-brain -p rb-hooks -p rb-install`, install
  claude, run `scripts/memory-scorecard.sh --bin-dir target/release --runs
  "$RUNS_INPUT"`, upload TSV. **Two differences:** (a) alert-issue title
  `"nightly memory-scorecard failing"` (distinct from old #19); (b) leave the
  `schedule:` cron **disabled/commented** until P3.
- **Delete:** `.github/workflows/w35-ab-eval.yml`, `scripts/w35-ab-eval.sh`,
  `crates/rb-eval/scorecard/w35_ab_scenarios.json`.
- **Docs:** mark `docs/eval/2026-06-14-w35-ab-eval.md` RETIRED with a pointer to
  the redesign doc; flip `docs/eval/2026-06-16-w35-criterion-redesign.md`
  `Status: proposed → active`; check off build-sequence steps 0–2.
- **GH #19:** comment "old 3-arm gate retired; successor live as
  memory-scorecard; tracked in Vikunja rusty-brain #2" and close.

## Phase 3 — Validate Class C (first real spend; human-dispatch)  · subtask P3

1. **Local smoke** (no workflow): `--runs 1` on one scenario → four arms each
   emit a row, scorecard renders, safety gate evaluates.
2. **Dispatch** `memory-scorecard.yml` at `--runs 1` (smoke), then `--runs 5`
   (real read per P3).
3. Read the scorecard. Safety gate must hold (zero MIE) and freshness must
   beat-realistic-AND-tie-steelman. If a scenario is noisy, **calibrate against
   real haiku output — never by rigging** (P1: a steelman win is non-negotiable).
4. **Enable the `schedule:` cron** only after green. Save the measured
   scorecard to `docs/eval/<date>-w35-scorecard-c-run.json`.

**Gate to proceed to B:** C green. (A may proceed in parallel from P4a.)

## Phase 4 — Class A: retrieval@scale (gated on the caching study)  · subtask P4

- **P4a study (low spend, decision record):** borrow the
  `--output-format stream-json --verbose` parsing from
  `scripts/w35-trace-tools.sh` to extract per-turn `usage` (`input_tokens`,
  `cache_creation_input_tokens`, `cache_read_input_tokens`) for memory-on vs
  steelman with a large CLAUDE.md. **Decision point:** if a cached CLAUDE.md
  drives marginal token cost ~0 while memory re-injects each turn, then token
  cost is informational only and **accuracy-at-scale is the primary metric**.
  Record as **ADR-3** in the redesign doc before running.
- **P4b scenarios:** add Class A rows to `memory_scorecard_scenarios.json` —
  `dimension: "retrieval-scale"`, `plant_mode: "explicit"`, a **500+ fact**
  explicit plant (note: `plant_explicit` loops one `remember` per fact — accept
  the latency, or add a bulk-load subcommand as a sub-task).
  `steelman_claude_md` = large file containing the target fact;
  `realistic_claude_md` = large/stale file that buries or omits it; `expect` =
  the one buried fact.
- **P4c aggregation:** add per-arm token reporting (informational) + the
  caching-adjusted comparison per ADR-3.

## Phase 5 — Class B: capture fidelity (expected to FAIL today; known lossy capture)  · subtask P5

- **P5a runner:** implement `plant_mode: "auto-capture"` at
  `scripts/memory-scorecard.sh:326` (replace the stub) — run a real PLANT
  session whose SessionEnd fold writes the summary, then the WORK session
  recalls. Requires the daemon + SessionEnd fold to fire (the real capture
  path).
- **P5b new metric:** a `capture_fidelity()` helper that inspects the store
  **directly** after the plant session (not via the downstream work session) and
  checks the folded summary actually contains the decision — the doc's
  first-class "capture-fidelity rate". New measurement path beside `judge_text`.
- **P5c scenarios:** `dimension: "capture"`, `plant_mode: "auto-capture"`; the
  plant prompt steers the model to state a decision; `expect` = that fact.
- **P5d expectation:** B will likely fail (lossy capture). Surface it — it is
  **tracked, not gating** (only safety gates). **File a follow-up task**
  (capture-quality fix) and keep B in the scorecard as a red-but-honest progress
  metric.

## Phase 6 — Class R: reach / team (stretch; report-only)  · subtask P6

- **Design decision to confirm at start:** two identities = two isolated HOMEs
  sharing **one store/namespace** (if namespaces differ, B can't see A's memory
  — namespaces are the isolation boundary). Simulate "A on another machine" via
  a second HOME against the shared DB.
- Realistic baseline: B's checkout never got A's CLAUDE.md edit (uncommitted).
  Steelman: it did.
- Add 2–3 `dimension: "reach"` scenarios. Metric: correctness for B.
  Report-only.

## Phase 7 — Closeout  · subtask P7

- Redesign doc: check off steps 3–4; record first full scorecard artifact under
  `docs/eval/`.
- Update the plan's §6 W3.5 progress note + Vikunja #2.
- `cargo test --workspace` + the relevant `--self-test`s green; `CHANGELOG.md`
  entry.

---

## Open decisions baked in (flag if you disagree)

- **Token cost for A is secondary** unless the caching study (P4a) says
  otherwise — accuracy-at-scale is primary. This is the doc's stated open
  question; it is the gate of P4.
- **Class B ships red** rather than being hidden — it is an honest progress
  metric and the trigger for a capture-quality follow-up.
- **Bulk `remember` for A's 500 facts**: tolerated as a slow loop first;
  promoted to a real bulk-load subcommand only if it materially slows the run.

## Risks

- Enabling the cron too early → new harness flaps like #19. Mitigated by the P3
  gate.
- `claude`'s `stream-json` usage fields vary across versions (the workflows are
  deliberately unpinned) → P4a must re-derive the field names per run, not
  hard-code.
- Capture-fidelity (P5b) scoring "did the summary contain the fact" can itself
  be noisy → use the same deterministic substring judge where possible,
  LLM-judge fallback documented.
