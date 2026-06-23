# PRD: W3.5 Scorecard Final Closeout

## Status

Draft, ready after the remaining W3.5 scorecard evidence is generated or explicitly deferred.

## Owner Area

Primary: evaluation documentation and release closeout.

Touchpoints:

- `docs/eval/2026-06-16-w35-criterion-redesign.md`
- `docs/plans/2026-06-11-rusty-brain-road-to-tens.md`
- `CHANGELOG.md`
- new measured artifact under `docs/eval/`
- sprint board parent task

## Problem

W3.5 status is currently spread across redesign docs, measured Class C artifacts, open PRs, and board tasks. The project needs a single closeout artifact that states what is measured, what landed but is unmeasured, what is intentionally deferred, and what remains open.

Without this, "scorecard complete" can be misread as "all dimensions passed" or "Phase 5 pilot is proven", neither of which is necessarily true.

## Goals

- Produce one final W3.5 closeout artifact.
- Account for every scorecard dimension: C, A, B, R, and safety.
- Link raw artifacts and commands.
- Preserve clear distinction between proxy scorecard evidence and later Phase 5 pilot evidence.
- Update roadmap, changelog, and board status consistently.

## Non-Goals

- Do not implement missing scorecard dimensions in this closeout task.
- Do not re-run expensive scorecards unless required to produce missing evidence.
- Do not claim real team adoption or two-machine pilot proof.
- Do not rewrite the W3.5 scorecard design.

## Functional Requirements

### C1. Closeout Artifact

Create:

```text
docs/eval/2026-06-23-w35-scorecard-closeout.md
```

The artifact must include:

- Date: 2026-06-23.
- Commit SHA or branch reference.
- Scorecard command(s).
- Scenario file path and scenario count.
- Raw TSV artifact link/path where available.
- Safety gate result.
- Dimension table covering C, A, B, R, and safety.
- For each deferral: owner, reason, and follow-up task.
- Explicit statement that W3.5 scorecard evidence is not Phase 5 pilot evidence.

Dimension states should use a small controlled vocabulary:

- `measured`
- `landed, unmeasured`
- `intentionally deferred`
- `not landed`

### C2. Existing Docs

Update `docs/eval/2026-06-16-w35-criterion-redesign.md` to point at the closeout artifact and current final status.

Update `docs/plans/2026-06-11-rusty-brain-road-to-tens.md` so the W3.5 section reflects final status and remaining follow-ups.

Update `CHANGELOG.md` under Unreleased with a concise scorecard closeout note.

### C3. Board State

Update the sprint board parent task with:

- closeout artifact path
- measured dimensions
- deferred dimensions
- raw artifact path or run URL
- remaining follow-up task IDs

Do not close the parent until the closeout artifact exists and links to the final task list.

## Acceptance Criteria

- Closeout artifact exists and has a dimension table for C, A, B, R, and safety.
- Safety result is explicitly stated.
- Deferred dimensions link to PRDs or board tasks.
- Redesign doc points to the closeout artifact.
- Road-to-tens plan reflects the final W3.5 state.
- Changelog has an Unreleased entry.
- Sprint board parent task has a comment or description update linking the closeout.
- No secrets, raw logs, or excessive model output are committed.
- The closeout does not claim Phase 5 pilot validation.

## Verification

Run:

```bash
scripts/memory-scorecard.sh --self-test
cargo test --workspace
```

If the closeout includes a new measured scorecard run, also record:

```bash
scripts/memory-scorecard.sh --runs 5
```

or the equivalent GitHub Actions run URL and artifact path.

## Risks

- Closeout overstates scorecard meaning. Mitigate with explicit proxy-vs-pilot language.
- Missing dimensions become invisible. Mitigate by requiring every dimension in the table.
- Artifact churn hides real status. Mitigate with one final closeout artifact and links out.
- Expensive reruns block closeout. Mitigate by marking dimensions as deferred when evidence is intentionally absent.

## Implementation Checklist

- [ ] Gather current scorecard artifacts and run links.
- [ ] Determine final state for C, A, B, R, and safety.
- [ ] Create closeout artifact.
- [ ] Update W3.5 redesign doc.
- [ ] Update road-to-tens plan.
- [ ] Update changelog.
- [ ] Update sprint board parent.
- [ ] Run self-test.
- [ ] Run workspace tests or document why they were skipped.
