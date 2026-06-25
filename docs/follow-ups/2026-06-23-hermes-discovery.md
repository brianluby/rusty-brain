# Follow-up: Hermes agent discovery status

- **Date:** 2026-06-23
- **Area:** cross-agentic agent parity
- **Status:** Discovery-gated

## Known facts

- No Hermes adapter or installer is implemented in this repo; the only scorecard
  presence is an unsupported `--agent hermes` target in `scripts/memory-scorecard.sh`
  that emits a discovery-gated skip row (`scorecard_unsupported_hermes_discovery_gated`).
  There is no live Hermes scorecard run.
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
