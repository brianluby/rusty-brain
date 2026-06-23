# Gemini Fixture Status

## Provenance

- **CLI:** Gemini CLI `0.46.0` (`gemini --version`)
- **OS:** macOS 26.5.1, arm64
- **Capture date:** 2026-06-23

## Recording Recipe

Planned recipe:

1. Create a throwaway project with `.gemini/settings.json`.
2. Register command hooks for `SessionStart`, `AfterTool`, `SessionEnd`, and
   `PreCompress` that append raw stdin JSON to per-event files.
3. Run a multi-turn headless Gemini session that performs one file write and one
   shell command.
4. Sanitize only local paths and secrets before committing the JSON fixtures.

This recording is **blocked in this worktree**. A real run would require model
auth/network access and may read or update user/global Gemini state outside the
owned worktree. Hand-authored JSON is not committed as a fixture.

## Sanitization Table

| What | Recorded value | Committed value |
|---|---|---|
| Real hook payloads | Not recorded | Not committed |

## Multi-Turn Event Cadence

Not fixture-verified in this worktree. The installer registers
`SessionStart`, `AfterTool`, `SessionEnd`, and `PreCompress`, but no committed
multi-turn capture proves whether native `SessionEnd` is a true terminus or a
per-turn boundary.

## Mapping / Fallback

Gemini native `SessionEnd` remains canonical `Stop` until a real fixture proves
a better checkpoint or true terminus boundary. The `SessionCheckpoint`
infrastructure exists in `rb-hooks`, but Gemini does not emit it yet.

## Known Absences

- No real transcript path fixture.
- No verified true session terminus.
- No committed raw payload JSON.
