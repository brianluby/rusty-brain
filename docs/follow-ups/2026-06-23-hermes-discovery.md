# Follow-up: Hermes agent discovery status

- **Date:** 2026-06-23
- **Area:** cross-agentic agent parity
- **Status:** Discovery-gated

## Known facts

- No Hermes adapter, installer, or scorecard target is implemented in this repo.
- No committed fixture proves Hermes hook event names, payload shape, lifecycle
  cadence, config path, transcript path, or prompt-time retrieval equivalent.
- No code should hard-code Hermes hook names or lifecycle semantics until those
  facts are recorded from a real current Hermes environment.

## Required discovery

Before implementation, record or link primary evidence for:

- CLI name/version and install source.
- Hook/config mechanism and scope.
- Session start, tool completion, per-turn boundary, true session terminus, and
  compaction payloads, if they exist.
- Prompt-time retrieval/injection equivalent, if any.
- Safe installer behavior and rollback path.

## Current mapping decision

Hermes remains absent from `AgentId` and unsupported by `rusty-brain-hooks`,
`rusty-brain-install`, and `scripts/memory-scorecard.sh` live runs. The
capability matrix lists Hermes as discovery-gated so users see a concrete status
instead of silent omission.
