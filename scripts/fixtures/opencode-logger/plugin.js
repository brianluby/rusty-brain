// OpenCode fixture-recording plugin. Recording aid ONLY — not the production
// integration (rb-install opencode support stays deferred). Writes each hook
// event payload as one JSON line to RB_FIXTURE_LOG_DIR/<stem>.json so the
// recorder can sanitize and commit them. See
// docs/specs/2026-06-23-cross-agent-fixture-recording.md.
import { appendFileSync } from "node:fs";

const LOG_DIR = process.env.RB_FIXTURE_LOG_DIR || ".";
const STEMS = {
  "session.created": "session_created",
  "tool.execute.after": "tool_execute_after",
  "session.idle": "session_idle",
  "session.compacted": "session_compacted",
  "session.deleted": "session_deleted",
};

function log(type, payload) {
  const stem = STEMS[type];
  if (!stem) return;
  appendFileSync(`${LOG_DIR}/${stem}.json`, JSON.stringify(payload) + "\n");
}

export const FixtureLogger = async () => ({
  event: async ({ event }) => log(event?.type, event),
});
