# OpenCode Fixture Status

## Provenance

- **CLI:** OpenCode `1.17.5` (`opencode --version`)
- **OS:** macOS 26.5.1, arm64
- **Capture date:** 2026-06-23

## Recording Recipe

Planned recipe:

1. Create a throwaway OpenCode project with a local plugin that logs hook event
   payloads.
2. Register capture for `session.created`, `tool.execute.after`,
   `session.idle`, `session.compacted`, and `session.deleted`.
3. Run a multi-turn `opencode run` session that performs one file write and one
   shell command.
4. Sanitize only local paths and secrets before committing the JSON fixtures.

This recording is **blocked in this worktree**. OpenCode hook installation is
not implemented in `rb-install` on this branch, and a real run would require
model auth/network access plus plugin/config writes outside the owned worktree.
Hand-authored JSON is not committed as a fixture.

## Sanitization Table

| What | Recorded value | Committed value |
|---|---|---|
| Real hook payloads | Not recorded | Not committed |

## Multi-Turn Event Cadence

Not fixture-verified in this worktree. The adapter recognizes
`session.created`, `tool.execute.after`, `session.idle`, and
`session.compacted`, but no committed multi-turn capture proves whether
`session.idle` or `session.deleted` is a true terminus.

## Mapping / Fallback

OpenCode native `session.idle` remains canonical `Stop` until a real fixture
proves a better checkpoint or true terminus boundary. The `SessionCheckpoint`
infrastructure exists in `rb-hooks`, but OpenCode does not emit it yet. Native
`session.deleted` remains `Other` until a real fixture proves terminal behavior.

## Known Absences

- No `rb-install` OpenCode plugin support in this branch.
- No real transcript path fixture.
- No verified true session terminus.
- No committed raw payload JSON.
