# Sprint PRDs - 2026-06-23

This directory contains implementation-ready product requirements for the remaining rusty-brain closeout work identified from roadmap docs, GitHub issues/PRs, and the sprint board on 2026-06-23.

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

## Notes

- The original six implementation tasks are now seven because the sprint needs a separate cross-agentic parity task.
- The parity task is broader than the non-Claude capture-regression task. Capture regression restores a broken lifecycle path; parity covers capture, retrieval, configuration, installer/docs, scorecard coverage, and Hermes discovery.
- Hermes support is explicitly discovery-gated. Do not invent Hermes hook event names or lifecycle semantics without recorded evidence.
- Existing unrelated untracked docs were left untouched.
