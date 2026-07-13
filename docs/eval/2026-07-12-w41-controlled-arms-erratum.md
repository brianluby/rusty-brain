# W4.1 controlled-arm decision erratum

Status: **decision correction; the original frozen artifact is retained
unchanged for auditability.**

The controlled-arm preregistration committed on 2026-07-12 transcribed the
wrong go/no-go thresholds. It must not be edited after results were observed.
Vikunja task #56 was updated at `2026-07-12T15:45:46-07:00`, before the primary
preregistration commit at 16:55 and the controlled preregistration commit at
17:10, and already contained the authoritative rules below. Therefore the
branch's original controlled decision is invalid; this erratum recomputes the
decision from pre-existing tracker criteria rather than tuning a threshold from
holdout results.

Authoritative rules copied from task #56:

- Exact-evidence treatment must improve exact-span coverage **and** exact-answer
  accuracy by at least 10 percentage points, with no material semantic-quality
  or safety regression.
- Surprise-aware selection must improve recall@5 by at least 5 points over
  current Linear selection and at least 10 points over recency-only, with NDCG
  loss no greater than 0.01 and no increase in stale, wrong, or poison exposure.
- Stop if gains require future query labels, extra prompt budget, production
  retention changes, or disproportionate retention of secrets, poison,
  duplicates, or low-value noise.

For mechanical evaluation, “material semantic-quality regression” and dedup
regression are bounded at 0.01, matching the original equal-budget artifact's
quality tolerance. The machine manifest locks these values and the hashes of
the original preregistration plus this erratum.

Recomputed outcome: **NO-GO for both exact evidence and surprise-aware
selection.** The dated results record the corrected metrics and failed rules.
