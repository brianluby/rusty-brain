# Real Claude Code hook-event payload fixtures (W0.7 carryover / C3)

These are REAL hook-event payloads as Claude Code delivered them on a hook's
stdin — not hand-authored approximations. They are the ground truth the
`ClaudeCodeCli` adapter and the rb-hooks lifecycle tests parse, and the seed
set the W3.4 fixture harness expands on.

## Provenance

- **Claude Code version:** `2.1.175 (Claude Code)` (`claude --version`)
- **Captured:** 2026-06-12, macOS (Darwin 25.5.0)
- **Recording recipe:** a throwaway project under `/tmp/rb-w07-capture/proj`
  with a `.claude/settings.json` registering one `type: "command"` hook per
  event (`SessionStart`, `UserPromptSubmit`, `PostToolUse` matcher `*`,
  `Stop`, `SessionEnd`, `PreCompact`), each appending its raw stdin JSON plus
  a trailing newline to a per-event file (`cat >> $f; printf '\n' >> $f`).
  Then one short headless session from that directory:

  ```sh
  claude -p "Create a file named hello.txt containing exactly: hi" \
    --allowedTools "Write" --permission-mode acceptEdits \
    --setting-sources project --model haiku --max-budget-usd 1
  ```

  Each event fired exactly once; every file below is the verbatim single
  line captured for that event (plus the sanitization noted next).

## Sanitization (the ONLY edits)

| What | Recorded value | Committed value |
|---|---|---|
| Recording user's home dir inside `transcript_path` | `/Users/<real-username>` | `/Users/user` |

Everything else — field names, key order, incidental fields, the session
UUID, `tool_use_id`, `cwd` (a non-identifying `/tmp` path), timings — is
byte-faithful to what Claude Code emitted.

## Files

| File | Event | Notes |
|---|---|---|
| `session_start.json` | `SessionStart` | `source: "startup"`; no `permission_mode` field on this event |
| `user_prompt_submit.json` | `UserPromptSubmit` | carries `prompt`; unmodeled today (parses as `HookEvent::Other`) — W3.2 consumes it |
| `post_tool_use_write.json` | `PostToolUse` (Write) | `tool_response` is an OBJECT (`{"type":"create","filePath":...}`), not a string; also carries `tool_use_id`, `duration_ms` |
| `stop.json` | `Stop` | carries `stop_hook_active`, `last_assistant_message`, `background_tasks`, `session_crons` |
| `session_end.json` | `SessionEnd` | `reason: "other"` in `-p` (print) mode; unmodeled today (parses as `HookEvent::Other`) — W3.1 consumes it |

## Known absences

- **PreCompact:** did not fire — it needs a transcript long enough to
  trigger compaction, which a one-prompt headless session never reaches.
  Recording it stays open (W3.4); note the plan already records that
  auto-compact PreCompact arrives with empty `custom_instructions`.

## Fields present in real payloads that the adapter intentionally drops

`permission_mode`, `tool_use_id`, `duration_ms`, `background_tasks`,
`session_crons` have no consumer in the plan and are not mapped onto
`HookContext`/`HookEvent`. `transcript_path` (every event) and
`stop_hook_active` (Stop) ARE parsed since W0.7-carryover landed, for W3.1.
