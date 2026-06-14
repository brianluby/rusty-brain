# Native distribution (W3.6a — Claude Code)

How rusty-brain reaches a Claude Code user/team. Three channels ship the SAME
agent context (MCP server + fail-open hooks + memory skill); **use exactly one
per project** — two active channels double-register the hooks (two captures per
event).

All channels wire CONTEXT, not binaries: `rusty-brain` and `rusty-brain-hooks`
must be on `PATH` (a checkout's `cargo install --path crates/rusty-brain --path
crates/rb-hooks`, or a signed release / Homebrew once published). Missing
binaries degrade silently — the MCP server doesn't start and the strictly
fail-open hooks no-op; nothing blocks.

## Channel 1 — plugin (`plugins/rusty-brain/`, via the marketplace)

The shareable artifact. This repo is itself a marketplace
(`.claude-plugin/marketplace.json`):

```text
/plugin marketplace add brianluby/rusty-brain
/plugin install rusty-brain@rusty-brain
```

Best for: individuals and teams who want versioned, one-command install +
updates, independent of any single project's committed config.

## Channel 2 — committed project config (`.mcp.json` + `.claude/settings.json`)

Zero-effort on clone: a team commits `.mcp.json` (registers the server) and
`.claude/settings.json` (hooks + `permissions.allow` for the rusty-brain MCP
tools) to their repo root, and every clone has memory active. **This repo commits
both** — it dogfoods rusty-brain on itself and is the reference template to copy.
`.claude/settings.local.json` stays personal (gitignored).

Best for: a team repo where memory should be on for everyone with no per-user step.

## Channel 3 — `rusty-brain install` (the W3.2 installer)

`rusty-brain install --agents claude-code` writes the same hooks +
`permissions.allow` (with an absolute, shell-quoted hooks path resolved for the
local machine) plus the CLAUDE.md policy block and the skill. Best for: a user who
wants a guided, machine-local setup without committing config or installing a
plugin. `rusty-brain uninstall` reverses it.

## Enterprise managed settings (Phase 5 rollout dependency)

Permission/hook/MCP policy is enforced by Claude Code, not the model, and
**managed settings cannot be overridden** by user/project config. An org can
therefore neutralize rusty-brain's native channels:

- **`permissions.deny`** of `mcp__rusty-brain__*` (or a managed deny of the whole
  server) blocks the memory tools regardless of our `permissions.allow`.
- **`allowManagedHooksOnly: true`** drops user/project/plugin hooks — capture +
  injection stop firing.
- **`allowManagedMcpServersOnly: true`** (or a managed MCP denylist) prevents the
  `rusty-brain` server from loading — no memory tools.
- **`strictPluginOnlyCustomization`** can force the plugin channel as the only
  permitted source of skills/hooks/MCP.

Consequence for a managed-settings org: rolling rusty-brain out is a **policy
decision**, not just an install. The org must explicitly allow the `rusty-brain`
MCP server and (if it restricts hooks) the capture hook, ideally via **managed
settings** so the allowance is centrally owned. The only channel that survives a
hooks/MCP lockdown is the **CLAUDE.md policy block** (plain context the model
reads) — but with the tools denied it has nothing to call. This is tracked as a
Phase 5 (team rollout) dependency, not a Phase 3 blocker.
