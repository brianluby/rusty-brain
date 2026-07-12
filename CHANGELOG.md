# Changelog

All notable changes to rusty-brain are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added — Typed code anchors (PRD 2026-07-02)

- **First-class, queryable code anchors**: a memory can now carry structured
  `file` (+ optional 1-based line range), `commit`, and `symbol` anchors,
  stored in the additive `memory_anchors` table (migration 009 — no ALTER on
  `memories`, no backfill, no `CONTRACT_VERSION` bump). Capture:
  `remember --file src/foo.rs:12-40 --commit <sha> --symbol Foo::bar`
  (repeatable), MCP `remember` `files`/`commits`/`symbols` params, and the
  SessionEnd hook fold now AUTO-ANCHORS the session summary to the touched
  files (fail-open, capped like the summary's file section). Recall/list
  filter by anchor — `--file <path>` / `--commit <sha>` / `--symbol <name>`
  (CLI + MCP; all-of composition with each other and with the metadata
  filters) — with normalization on both sides (`./src/a.rs` == `src/a.rs`;
  file filters match by path, line ranges are capture-only). Anchors ride
  `graph`/`get`/`list` output, survive `export`/`restore`, and follow a
  `namespace rename`. v1 ships anchors as a FILTER only; ranking boosts are
  deferred to W4.1 evidence per the PRD.
- **`anchors` daemon capability on the handshake ack**: the additive
  `HandshakeAck.capabilities` list lets clients distinguish an
  anchor-evaluating daemon from an older one that would silently drop
  `Remember.anchors` or ignore anchor filters. Typed clients (and the MCP
  adapter's raw path) fail fast with `InvalidArgument` when anchors are used
  against a daemon that did not advertise the capability; the hook path stays
  fail-open and stores the summary WITHOUT anchors instead. The default MCP
  `tools/list` stays under the W3.3 token budget (897/900) — existing
  descriptions were tightened to make room for the anchor params.

### Added — Contract-drift guard (W5a.4 operationalized)

- **`rb-contract-guard` + `contract-drift` CI job**: the wire surface
  (rb-proto messages plus the rb-types payload shapes they embed) and the
  rb-store migrations are digested (syn-normalized item tokens: doc comments,
  regular comments, and formatting never trip the guard) and compared against
  the checked-in `contract-snapshot.toml`. Any drift fails CI until the PR
  records a deliberate decision: `update --intent additive` (serde-default
  change, no `CONTRACT_VERSION` bump — the tool refuses a bumped version) or
  `update --intent breaking` (the tool demands the bump). The note lands in
  the snapshot's append-only `[[log]]` so reviewers see the decision in the
  diff. Also bound into `cargo test --workspace` and `scripts/ci-local.sh`.
  See `docs/specs/2026-07-11-contract-drift-guard.md`.
- **N-1 handshake fixture (W5a.4)**: rb-daemon e2e now pins version-skew
  behavior — with no dual-support window open, an N-1 client handshake gets a
  graceful `HandshakeAck { ok: false }` naming both versions, then a closed
  connection.

### Added — Cross-agent capability status

- **Agent capability matrix**: `rb-agents` now exposes the current support state
  for Claude Code, Codex, OpenCode, Gemini, and discovery-gated Hermes. The
  matrix makes partial/unsupported capture, retrieval, config, and scorecard
  capabilities explicit instead of implying parity.
- **Scorecard agent targeting**: `scripts/memory-scorecard.sh --agent` now
  accepts `claude-code`, `codex`, `opencode`, `gemini`, `hermes`, or `all`.
  Claude Code runs the existing live scorecard; unsupported targets emit a
  machine-readable skip line with agent, phase, status, reason, and detail.
- **Hermes discovery note**: `docs/follow-ups/2026-06-23-hermes-discovery.md`
  records the current known facts and blocks speculative hook/config constants.

### Added — W3.5 scorecard closeout

- **W3.5 closeout artifact**:
  `docs/eval/2026-06-23-w35-scorecard-closeout.md` now records the final
  scorecard status: Class C / Freshness is measured green with raw TSV evidence
  and 0 memory-induced errors; Classes A/B/R are landed but unmeasured and remain
  API/spend-backed follow-up reads. The artifact explicitly scopes W3.5 as proxy
  scorecard evidence, not Phase 5 pilot proof.

### Added — Class A retrieval@scale scorecard + bulk remember

- **`rusty-brain remember --batch`**: read one fact per line from stdin and store
  them all over a SINGLE daemon connection (the `--type`/`--importance`/`--tags`/
  `--context` flags apply uniformly; blank lines are skipped; incompatible with a
  positional content arg and with `--supersedes`). At 500+ facts a per-fact CLI
  call is dominated by process spawn + handshake (the embed is the cheap
  deterministic fallback), so reusing one connection is the win.
- **Retrieval@scale (Class A) in the memory-value scorecard**: new
  `retrieval_scale` scenarios in `crates/rb-eval/scorecard/memory_scorecard_scenarios.json`
  bury one explicitly-planted target (importance 8) under `corpus_size` (500/500/
  1000) deterministic off-topic distractors — bulk-planted into memory-on via
  `remember --batch`, and written into both baselines' CLAUDE.md (steelman:
  target + distractors; realistic: distractors only). Accuracy on the buried fact
  is primary.
- **ADR-3 token/cost reporting in the scorecard** (`scripts/memory-scorecard.sh`):
  every session now runs under `--output-format stream-json --verbose`; the TSV
  grew to 13 fields with `total_cost_usd` + the four cache buckets, the per-arm
  table gained a `mcost$` column, and the `retrieval_scale` dimension prints an
  ADR-3 block (cache% / ctx_vol / eff_in diagnostics + a RATIFY-Opt-3 / Opt-2 /
  descope verdict comparing memory-on vs steelman on accuracy AND `total_cost_usd`
  within 20%). All exercised by `--self-test` (no API); the measured run stays
  deferred on a key + spend. See docs/eval/2026-06-19-w35-cache-study.md.

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

### Notes

- The three agent-surface crates are workspace members but are **never** in the
  default `cargo build`/`rusty-brain`-binary dependency closure — no core crate
  depends on them. This keeps the daemon/CLI lean and is enforced in CI.
