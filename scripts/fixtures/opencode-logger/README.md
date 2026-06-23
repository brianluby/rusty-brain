# OpenCode fixture-recording plugin

A standalone OpenCode plugin used only to record real hook-event payloads for
rusty-brain's cross-agent fixtures. It is NOT the production integration —
`rb-install` opencode support is deferred (see the cross-agentic parity PRD).

`scripts/record-agent-fixtures.sh --agent opencode` copies this plugin into the
recorder project (a STABLE recorder home OUTSIDE the repo with the operator's
auth copied in, mode 0600), registers it via `opencode.json`'s `plugin` array,
sets `RB_FIXTURE_LOG_DIR`, runs `opencode run --format json` (with all XDG dirs
redirected to the recorder home so global opencode state is never touched), then
sanitizes and commits the captured payloads under
`crates/rb-hooks/tests/fixtures/opencode/`. opencode has no plugin-trust gate, so
no pre-trust step is needed.

The plugin taps both the generic `event` hook and the dedicated
`tool.execute.after` hook (where `args` is on the INPUT object and
`title`/`output`/`metadata` on the OUTPUT object), and exports `FixtureLogger`
under both a named and a default export so a single-plugin file loads
unambiguously. Verified against opencode 1.17.5 / `@opencode-ai/plugin` 1.2.15.

Pin the recorded OpenCode version in the generated fixture README; the plugin is
kept minimal to reduce API-drift surface.
