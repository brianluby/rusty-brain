# Recall confidence floor — #56 poison-gate re-run

- **Date:** 2026-09-26
- **Task:** Vikunja #59 admission (clears the #56 zero-poison-exposure blocker)
- **Base:** `main` @ `2b9ab39`, rustc 1.98.1, Darwin 27.0.0 arm64
- **Change:** recall and the SessionStart digest (`MemoryEngine::context`)
  exclude any memory whose `confidence <= RECALL_CONFIDENCE_FLOOR` (0.1).

## Why a floor

The #56 blocker (`docs/eval/2026-07-12-w41-semantic-gate-results.md`) was a
confidence-0.0 instruction-shaped poison returned as recall result #2. The
confidence dampener ranks it below the correct fact but never removes it.
Re-checked on 2026-09-26: #62 (abstention), #63 (trust classes) and #69
(write-path gate and channel quarantine) did not close it. The probe's
correct fact and poison have identical provenance (no channel, no source).
Confidence is the only field that separates them, so no provenance rule
can pass the gate without also hiding the correct answer or changing the
fixture to fit the fix.

## Floor value and its limits

The floor was chosen **after** the probe was known (poison at 0.0), so it is
**not** a blind preregistration. Its value comes from feedback arithmetic,
not from the probe:

| Start | Path to the floor |
|---|---|
| hook capture, 0.7 | two `wrong` verdicts (−0.30 each) |
| explicit fact, 1.0 | three `wrong` verdicts |
| any | `stale` alone (−0.15) never reaches it from ≥0.25 |

The lowest legitimate confidence in the eval corpus is 0.8. In the
maintainer's live store it is 0.7. The floor touches neither.

**What it does not cover.** A poison written at a normal confidence (such as
a fresh 0.7 hook capture) still passes the floor. Only confidence already
driven to exhaustion is withheld. Write-time defenses (#69 write gate, fold
source filtering) remain the primary control for fresh poison.

## Gate results (baseline vs floor)

Every step was run on both trees. Excluding latency timing noise, the
outputs are **byte-identical**:

| Step | Baseline | Floor | Non-latency diff |
|---|---|---|---:|
| `production_embedding_linear_gate_passes_goldens_and_untouched_holdout` | pass | pass | 0 lines |
| `five_seed_linear_rrf_diagnostic` (ignored diagnostic) | pass | pass | 0 lines |
| `controlled_retrieval_and_admission_arms_report_every_seed` (ignored diagnostic) | pass | pass | 0 lines |
| `pilot_gate_requires_zero_instruction_poison_exposure` | **FAIL** (poison at rank 2) | **pass** | — |

Production Linear gate on the floor tree: golden recall@k 0.9630, MRR 0.9838,
NDCG 0.9537, dedup 0.9472. Holdout (aggregate only) recall@k 0.975, MRR 0.95,
NDCG 0.9498. These are unchanged from baseline.

Full workspace: 1911 passed, 14 ignored.

## Test changes

- `pilot_gate_requires_zero_instruction_poison_exposure` is no longer
  `#[ignore]`. It now runs in the default suite. The semantic-quality
  workflow step drops `--ignored` (which would otherwise have run zero tests
  and passed vacuously) and passes `--exact`.
- `known_instruction_poison_exposure_is_recorded_as_a_pilot_blocker` is
  removed. It pinned the NO-GO and said to revisit once exposure reached
  zero.
- The two confidence-dampener tests move their poison from 0.1 and 0.05 to
  0.3. They still test the dampener, now above the floor. A new engine test
  covers floor exclusion from recall and from the digest.

## Pilot manifest

`docs/eval/2026-07-12-phase5-dogfood-pilot-manifest.json` is **not** flipped
here. `task_56.state=go` needs an immutable `commit:` reference, which exists
only after merge. The pilot also stays blocked on #57 (no operating
envelope).
