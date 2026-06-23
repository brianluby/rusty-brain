// OpenCode fixture-recording plugin. Recording aid ONLY — not the production
// integration (rb-install opencode support stays deferred). Writes each hook
// event payload as one JSON line to RB_FIXTURE_LOG_DIR/<stem>.json so the
// recorder can sanitize and commit them. See
// docs/specs/2026-06-23-cross-agent-fixture-recording.md.
//
// Two distinct hook surfaces are tapped:
//   * the generic `event` handler — receives SDK Event objects whose payload
//     lives under `event.properties` (e.g. session.idle/created/deleted); we
//     flatten the relevant fields to the root so the recorded fixture matches
//     what the `OpenCodeCli` adapter parses (sessionID/directory at the top).
//   * the dedicated `tool.execute.after` hook — NOT part of the Event union; it
//     is a separate slot invoked as (input, output) and is the PostToolUse
//     source the adapter cares most about, so it must be registered explicitly.
import { appendFileSync } from "node:fs";

const LOG_DIR = process.env.RB_FIXTURE_LOG_DIR || ".";
const STEMS = {
  "session.created": "session_created",
  "tool.execute.after": "tool_execute_after",
  "session.idle": "session_idle",
  "session.compacted": "session_compacted",
  "session.deleted": "session_deleted",
};

function write(type, payload) {
  const stem = STEMS[type];
  if (!stem) return;
  appendFileSync(`${LOG_DIR}/${stem}.json`, JSON.stringify(payload) + "\n");
}

// Flatten an SDK Event ({ type, properties: {...} }) into the root-level shape
// the adapter reads (sessionID/directory at top level), keeping the nested
// `properties` for fidelity.
function flattenEvent(event) {
  const props = event?.properties ?? {};
  const info = props.info ?? {};
  return {
    type: event?.type,
    sessionID: props.sessionID ?? info.id,
    directory: props.directory ?? info.directory,
    properties: props,
  };
}

export const FixtureLogger = async () => ({
  event: async ({ event }) => write(event?.type, flattenEvent(event)),
  // tool.execute.after is a separate Hooks slot with an (input, output)
  // signature; merge both into one payload matching the adapter's fields
  // (tool, args -> tool_input, output -> tool_response).
  "tool.execute.after": async (input, output) =>
    write("tool.execute.after", {
      type: "tool.execute.after",
      tool: input?.tool,
      sessionID: input?.sessionID,
      callID: input?.callID,
      args: input?.args,
      output: output?.output,
      title: output?.title,
      metadata: output?.metadata,
    }),
});
