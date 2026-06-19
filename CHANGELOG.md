# Changelog

All notable changes to rusty-brain are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added — C1 user config file (W0.2 carryover)

- **`~/.config/rusty-brain/config.toml`** (or under `$XDG_CONFIG_HOME`):
  daemon knobs — `socket_path`, `db_path`, `idle_timeout_secs`, `jobs_config`,
  `[embed] backend`/`local_model`, `[enrich] base_url`/`model` — now live in a
  user config file with precedence **CLI flag > env var > config file >
  default**. Every binary (CLI, hooks, daemon) re-reads the file from disk
  itself, so a file-set knob reaches **auto-started** daemons with no env
  forwarding — retiring the F20 bug *class*. Unknown keys warn (forward
  compat); malformed TOML fails closed naming the file; secrets
  (`VOYAGE_API_KEY`, `RB_ENRICH_API_KEY`) stay env-only; `accept_model_change`
  is deliberately not a file knob (consent is per-change).
- **`FORWARD_ENV` shrunk to secrets + identity + XDG/HOME** (9 entries). The
  seven pre-existing knob env vars keep working (env wins over the file,
  including through auto-start) via a frozen `LEGACY_KNOB_ENV` compat list
  that must never grow. The repo-committed `.rusty-brain.toml` remains
  namespace-identity-ONLY and is never a configuration source.

### Added — P4 agent surface

- **`rusty-brain-hooks` binary** (crate `rb-hooks`): a fail-open, capture-only
  per-event hook for JSON-protocol CLIs. Selected with `--agent <id>` for
  `claude-code`, `opencode`, `gemini`, or `codex` (Copilot deferred). It reads a
  hook event on stdin, captures mutating-tool observations (`Edit`/`Write`/`Bash`,
  deduped) into the daemon, injects recent high-importance memories on
  `SessionStart`, and **always exits 0** — it never blocks, never tracks memory
  debt, and never returns a non-zero exit.
- **`rusty-brain-install` binary** (crate `rb-install`): agent-surface — capture
  hooks + installer for **Claude Code, Gemini, and Codex** (OpenCode deferred —
  needs a JS/TS plugin, not a JSON hooks block). Merges a sentinel-marked
  (`rusty-brain`) hook block into each CLI's config, using that CLI's real hook
  event names and command form (Claude Code: exec `command`+`args`; Gemini —
  `SessionStart`/`AfterTool`/`SessionEnd`/`PreCompress` — and Codex use an inline
  quoted command string; SessionStart context is injected via
  `hookSpecificOutput.additionalContext`). Claude Code project
  `.claude/settings.json` by default, `--global` supported, with a `.bak`
  backup and atomic temp+fsync+rename. `uninstall` removes only the sentinel
  block, preserving any other user hooks. Supports `status` and `--dry-run`,
  with JSON or human output (non-TTY auto-selects JSON). Explicitly requesting
  `--agents opencode` returns a clear "deferred" error rather than silently doing
  nothing.
- **`rb-agents` crate**: the shared, CLI-agnostic spine — canonical `HookEvent`,
  per-CLI JSON adapters (`AgentCli`), a fail-open best-effort `DaemonClient` over
  `rb-proto`, namespace detection, and the install-side `AgentInstaller`
  contract.
- **CI `build-agents` job**: builds, lints, and tests `rb-agents`/`rb-hooks`/
  `rb-install`, and asserts via `cargo tree -e no-dev -p rusty-brain` that none
  of them enter the default `rusty-brain` binary closure.
- **`scripts/install-agents.sh`**: places the `rusty-brain-hooks` and
  `rusty-brain-install` binaries alongside `rusty-brain` in `~/.local/bin`,
  `chmod +x`, with SHA-256 verification of each copy.

### Added — W3.5 cache-trace instrumentation (P4a)

- **`scripts/w35-trace-tools.sh`** now emits four `tok_*` columns
  (`tok_in`, `tok_cache_create`, `tok_cache_read`, `tok_out`) summed from each
  `assistant` event's `.message.usage`, plus a per-arm `aggregate_cache` table
  (mean buckets + `mean_ctx_vol` and cache-weighted `mean_eff_in`). This is
  the prompt-caching study for W3.5 dimension A — `total_cost_usd` is already
  cache-adjusted, so the buckets are surfaced **diagnostically** (raw token
  counts double-count cheap cache reads and must never be the cost axis).
  **ADR-3** and the investigation live in
  `docs/eval/2026-06-19-w35-cache-study.md`; the measured scale run stays
  deferred (no API key/spend). New `--self-test` validates the token math +
  aggregate with no API; `.github/workflows/w35-trace.yml` runs it on every
  dispatch before spending.

### Notes

- The three agent-surface crates are workspace members but are **never** in the
  default `cargo build`/`rusty-brain`-binary dependency closure — no core crate
  depends on them. This keeps the daemon/CLI lean and is enforced in CI.
