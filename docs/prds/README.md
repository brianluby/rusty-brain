# PRDs

This directory contains implementation-ready product requirements.

The 2026-06-23 set covers the remaining closeout work identified from roadmap
docs, GitHub issues/PRs, and the sprint board. The 2026-07-02 set comes from a
senior-PM product review and targets the activation/value-realization gap the
engineering roadmap does not measure.

## Final Sprint Task List

| Task | PRD | Status |
| --- | --- | --- |
| W3.5 Class B capture fidelity scorecard | [2026-06-23-w35-class-b-capture-fidelity.md](2026-06-23-w35-class-b-capture-fidelity.md) | Ready to implement |
| W3.5 Class R reach/team scorecard simulation | [2026-06-23-w35-class-r-reach-team.md](2026-06-23-w35-class-r-reach-team.md) | Ready to implement, report-only |
| W3.5 scorecard final closeout | [2026-06-23-w35-scorecard-closeout.md](2026-06-23-w35-scorecard-closeout.md) | Ready after scorecard artifacts exist |
| PR #23 recall and SessionEnd summary triage | [2026-06-23-pr23-recall-summary-triage.md](2026-06-23-pr23-recall-summary-triage.md) | Triage first, then reimplement if needed |
| Non-Claude CLI capture regression after W3.1 | [2026-06-23-cross-cli-capture-inversion.md](2026-06-23-cross-cli-capture-inversion.md) | Fixture-gated |
| Codex `apply_patch` capture | [2026-06-23-codex-apply-patch-capture.md](2026-06-23-codex-apply-patch-capture.md) | Upstream/readiness-gated |
| Cross-agentic agent parity for OpenCode, Codex, and Hermes | [2026-06-23-cross-agentic-agent-parity.md](2026-06-23-cross-agentic-agent-parity.md) | New sprint task |

## Sequencing

- [Next-work sequencing and parallelization](2026-06-23-next-work-sequencing.md) — ordering and parallel-track guidance across the W3.5 closeout and cross-agent parity work.

## 2026-07-02 — Product-review feature PRDs

From the senior-PM product review. Ordered by leverage (Tier 1 = activation/retention loop, Tier 2 = differentiation, Tier 3 = hygiene/scale).

| # | Tier | PRD | Current status |
| --- | --- | --- | --- |
| 1 | 1 | [First-run cold-start and project import](2026-07-02-init-and-project-import.md) | Delivered (PR #51) |
| 2 | 1 | ["Memory is working" observability](2026-07-02-doctor-and-stats-observability.md) | Delivered (PR #56) |
| 3 | 1 | [Portable export and one-command backup](2026-07-02-portable-export-and-backup.md) | Delivered (PR #52) |
| 4 | 2 | [Typed code anchors](2026-07-02-typed-code-anchors.md) | Delivered |
| 5 | 2 | [Decision history and audit timeline](2026-07-02-decision-history-timeline.md) | Delivered |
| 6 | 2 | [Guided contradiction/dedup resolution](2026-07-02-contradiction-dedup-review.md) | Delivered |
| 7 | 2 | [HTTP/REST surface and agent-agnostic recall](2026-07-02-http-surface-and-agent-agnostic-recall.md) | Delivered (PR #62) |
| 8 | 3 | [User-facing retention and forgetting policy](2026-07-02-user-facing-retention-policy.md) | Delivered |
| 9 | 3 | [Search and filter parity](2026-07-02-search-filter-parity.md) | Delivered |
| 10 | 3 | [Native Windows support](2026-07-02-native-windows-support.md) | Deferred; WSL2 remains the documented path |

## Notes

- The original six implementation tasks are now seven because the sprint needs a separate cross-agentic parity task.
- The parity task is broader than the non-Claude capture-regression task. Capture regression restores a broken lifecycle path; parity covers capture, retrieval, configuration, installer/docs, scorecard coverage, and Hermes discovery.
- Hermes support is explicitly discovery-gated. Do not invent Hermes hook event names or lifecycle semantics without recorded evidence.
- Existing unrelated untracked docs were left untouched.
