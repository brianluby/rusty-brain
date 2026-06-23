# Codex Fixture Status

## Provenance

- **CLI:** Codex CLI `0.142.0` (`codex --version`)
- **OS:** macOS 26.5.1, arm64
- **Capture date:** 2026-06-23

## Recording Recipe

Planned recipe:

1. Create a throwaway project with `.codex/hooks.json`.
2. Register command hooks for `SessionStart`, `PostToolUse`, `Stop`, and
   `PreCompact` that append raw stdin JSON to per-event files.
3. Run a multi-turn `codex exec` session that performs one Bash command.
4. Separately run a current Codex session that uses `apply_patch` and verify
   whether it emits `PostToolUse`, the exact `tool_name`, and the raw patch
   payload location.
5. Sanitize only local paths and secrets before committing the JSON fixtures.

This recording is **blocked in this worktree**. `codex doctor --json` reports
ChatGPT auth is configured, but provider reachability fails under the restricted
network environment. Running an authenticated session would also update
user/global Codex state outside the owned worktree. Hand-authored JSON is not
committed as a fixture.

## Sanitization Table

| What | Recorded value | Committed value |
|---|---|---|
| Real hook payloads | Not recorded | Not committed |

## Multi-Turn Event Cadence

Not fixture-verified in this worktree. The installer registers
`SessionStart`, `PostToolUse`, `Stop`, and `PreCompact`, but no committed
multi-turn capture proves whether native `Stop` is a true terminus or a
per-turn boundary.

## Mapping / Fallback

Codex native `Stop` remains canonical `Stop` until a real fixture proves a
better checkpoint or true terminus boundary. The `SessionCheckpoint`
infrastructure exists in `rb-hooks`, but Codex does not emit it yet.

Codex `apply_patch` capture remains blocked. The capture layer intentionally
does not recognize `tool_name: "apply_patch"` until a real current Codex
`PostToolUse` fixture proves the event shape and raw patch payload location.

## Known Absences

- No real transcript path fixture.
- No verified true session terminus.
- No committed raw payload JSON.
- No `apply_patch` `PostToolUse` fixture.
