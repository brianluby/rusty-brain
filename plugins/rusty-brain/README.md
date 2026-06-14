# rusty-brain — Claude Code plugin

Persistent, decision-grade memory for Claude Code. The plugin wires the agent
context; it does **not** ship binaries.

It bundles:

- **`.mcp.json`** — registers the stdio `rusty-brain` MCP server (`recall`,
  `remember`, `get`, `context`, …).
- **`hooks/hooks.json`** — the strictly fail-open capture/inject hooks
  (`SessionStart` recall injection, `UserPromptSubmit` deterministic recall,
  `PostToolUse`/`SessionEnd` capture, `PreCompact`, `Stop`).
- **`skills/rusty-brain-memory/`** — the always-available memory skill.
- **`commands/`** — `/rusty-brain:remember` and `/rusty-brain:recall`.

## Prerequisite (out of band)

The `rusty-brain` and `rusty-brain-hooks` binaries must be on `PATH`:

```bash
cargo install --path crates/rusty-brain --path crates/rb-hooks   # from a checkout
# or a signed release artifact / Homebrew once published
```

Without them the MCP server simply doesn't start and the fail-open hooks no-op —
nothing blocks.

## Install

```text
/plugin marketplace add brianluby/rusty-brain
/plugin install rusty-brain@rusty-brain
```

(or, for local development, `claude --plugin-dir ./plugins/rusty-brain`.)

## Do NOT also commit the project config

The plugin and the committed project config (`.mcp.json` +
`.claude/settings.json`, see [`docs/2026-06-14-native-distribution.md`](../../docs/2026-06-14-native-distribution.md))
are **alternative** channels — enabling both double-registers the hooks (two
captures per event). Pick one per project.
