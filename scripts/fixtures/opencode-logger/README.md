# OpenCode fixture-recording plugin

A standalone OpenCode plugin used only to record real hook-event payloads for
rusty-brain's cross-agent fixtures. It is NOT the production integration —
`rb-install` opencode support is deferred (see the cross-agentic parity PRD).

`scripts/record-agent-fixtures.sh --agent opencode` copies this plugin into a
throwaway project, sets `RB_FIXTURE_LOG_DIR`, runs `opencode run`, then
sanitizes and commits the captured payloads under
`crates/rb-hooks/tests/fixtures/opencode/`.

Pin the recorded OpenCode version in the generated fixture README; the plugin is
kept minimal to reduce API-drift surface.
