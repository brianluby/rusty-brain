# Session replay test fixtures

Every fixture below is miniature, invented, and source-shape-specific. No line
was copied from a real transcript or OpenCode database.

- `claude/invented.jsonl` exercises user/assistant text, tool call/result
  separation, private-reasoning rejection, ordering, provenance, and sensitive
  value replacement.
- `claude/unidentified.jsonl` verifies per-record rejection accounting when a
  file has no session or project identity.
- `opencode/invented.sql` creates a disposable in-test `session`/`message`/`part`
  database with the same invented behaviors.

These fixtures are reviewed test inputs, not semantic evaluation ground truth.
