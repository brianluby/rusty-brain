# Changelog

All notable changes to rusty-brain are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added — Codex `apply_patch` file-edit capture (follow-up 2026-06-02)

- **Codex `apply_patch` capture is live**: openai/codex#16732 shipped in Codex
  0.123.0 — Codex now emits `PostToolUse` for `apply_patch` with the raw V4A
  patch under `tool_input.command`, verified against a live codex-cli 0.144.1
  capture (committed as sanitized fixtures under
  `crates/rb-hooks/tests/fixtures/codex/`). Codex file edits now land in the
  per-session capture scratch; the scratch still awaits the fixture-gated
  `Stop` terminus mapping to fold (capture stays `partial` in the matrix).
- **Multi-file `apply_patch` patches record every touched path**: one
  `apply_patch` call can add/update/delete/rename several files in a single
  V4A patch (the live capture proves it); capture now records one observation
  per directive — `*** Add|Update|Delete File:` plus the `*** Move to:`
  rename destination (a rename records both source and destination) — in
  patch order, deduplicated, instead of only the first, for Codex and
  OpenCode alike. Malformed/non-V4A payloads still fail open to a single
  `unknown` file touch.
- **Hunk-aware, path-vetted V4A parsing**: directives are recognized at
  column 0 only and never inside `@@` hunk bodies, so patch content that
  merely looks like a directive (e.g. a context line reading
  `*** Add File: /etc/cron.d/evil`) can no longer register phantom touched
  files in session summaries and memory anchors. Directive paths are vetted
  per the capture PRD: leading `./` stripped (matching
  `rb_types::normalize_anchor_value`); empty, absolute, and `..`-traversal
  paths rejected (V4A paths are relative-only by spec); patch hunk content
  never reaches the scratch or folded summaries (leak-tested).
- **Batched scratch appends**: one `apply_patch` event persists all its
  observations in a single scratch read-modify-write round
  (`Scratch::append_many`) instead of one full write per touched file, and
  the fixture recorder's cleanup now preserves the hand-committed codex
  apply_patch fixtures it cannot regenerate (pinned by its `--self-test`).
- **Cross-adapter test uses the real Codex tool name**: the Codex PostToolUse
  fixture in `crates/rb-agents/tests/cross_adapter.rs` asserts `apply_patch`
  (mirroring the live-recorded payload) instead of the `Write` placeholder.

### Added — Guided contradiction/dedup review (PRD 2026-07-02)

- **`rusty-brain review`**: one guided sweep over the trust backlog. The
  queue surfaces, in priority order, active contradiction pairs (the
  pairwise expression of `active_contradicts`, held in lockstep by a drift
  test), near-duplicate pairs (reusing `near_duplicates()` — the one
  similarity definition, conservative 0.95 default / 0.80 floor), and
  low-confidence (< 0.4) plus stale never-recalled singles (the stats
  predicate + a 30-day age bound). Bare `review` is a DRY-RUN listing;
  `--interactive` walks the queue item by item (`keep` / `bump` / `merge` /
  `archive` / `demote` / `snooze`); `--apply --policy auto-merge-dups |
  demote-low-confidence` executes one bounded pass non-interactively.
  `--since <seq>` scopes to recently-touched memories; `--limit` and
  `--threshold` are server-clamped.
- **Safety posture (the forget precedent)**: apply requires an explicit
  policy (never auto-resolve without consent); on a TTY it previews the
  SAME plan the pass executes and asks with a default-NO prompt; `--yes`
  is required for `--json`/piped automation; `--dry-run` conflicts with
  `--apply` at parse time and wins over every mutating flag at runtime.
  Every action is reversible — merge is restricted to near-duplicate items
  and runs as ONE writer transaction (re-validate the pair still qualifies
  at the resolve-time threshold, insert the combined memory, copy the
  originals' external graph edges, supersede both originals behind a
  pointer guard, write the audit row — all-or-nothing, so concurrent
  resolutions of the same pair cannot split the chain: the loser gets the
  distinct `stale_plan` error); archive is a soft delete; demote/bump are
  bounded confidence nudges (`-0.15` / `+0.05`, the feedback magnitudes).
  Contradiction actions re-prove the pair is still an active contradiction
  at resolve time. Partial failures follow the `ForgetOutcome` shape:
  completed items stay committed, benign stale-plan collisions are skipped
  (the pass continues), real failures stop re-runnably, and the bulk
  `review_sweep` oplog row is written unconditionally.
- **Snooze persistence**: the additive `review_state` table (migration 010)
  keyed by the canonical item key (reason + sorted member ids), so a
  snoozed item stays hidden until its window elapses and acting on an item
  clears its snooze; `namespace rename` re-keys the rows in the same
  transaction so snoozes survive a rename. Every resolution records provenance + a
  `review_resolve` oplog row (REV-4), so merges surface in the history
  timeline and review activity feeds stats.
- **Wire surface**: additive `Review`/`Resolve` requests and
  `ReviewPlanned`/`ReviewDone`/`Resolved` responses (serde-default
  payloads, no `CONTRACT_VERSION` bump; an absent `dry_run` always
  previews). Queue generation runs entirely on the read pool (zero writer
  ops, pinned). Deliberately NOT an MCP tool (the forget precedent:
  destructive surface; tools/list budget at 897/900).
- **Stats**: new additive `low_confidence_live` gauge — with `contested`
  and `never_recalled_live`, the review-queue trend.

### Added — HTTP/REST surface and agent-agnostic prompt-time recall (PRD 2026-07-02)

- **Opt-in loopback HTTP listener** (`serve --http [bind]` / `[http]`
  config): REST endpoints (`GET /ping`, `GET /context`, `GET /memories/:id`,
  `POST /recall`, `POST /remember`, `POST /feedback`, generic `POST /ops`)
  that MIRROR the UDS wire ops — same `Request` decode, same `dispatch`,
  same `Response` JSON, no `CONTRACT_VERSION` change. Off by default at
  every layer (the `[retention]` precedent): no config → no TCP socket
  bound, no task spawned. Fail-closed security posture, threat-modeled in
  `docs/THREAT_MODEL.md` ("The opt-in HTTP listener"): literal-loopback-only
  bind validated at config resolve AND re-validated at `Daemon::bind`
  (hostnames never parse — no DNS decides where the daemon listens); admin
  ops (`RunJob`/`Reembed`/`NamespaceRename`/`Scrub`/hard `Forget`) are
  ALWAYS denied over HTTP (no kernel peer credential over TCP — the W2.6
  unreadable-peer-cred fail-closed path); loopback Host/Origin checks
  (DNS-rebinding and browser cross-origin defense) plus a required
  `application/json` content type (forces CORS preflight; no CORS headers
  are ever emitted); 1 MiB body cap (UDS `MAX_FRAME_BYTES` parity, refused
  from the declared length before buffering); header-read and per-request
  deadlines; a dedicated connection semaphore so HTTP load cannot starve
  the UDS path; graceful shutdown covers the listener. The
  `x-rusty-brain-namespace` header scopes requests (validated like the
  handshake namespace); HTTP writes are stamped `origin_source = "http"`.
- **`Client<S>` transport genericization (W5a.3, pulled forward)**:
  `rb_proto::Client` is now generic over any `AsyncRead + AsyncWrite`
  stream with `UnixStream` as the default; `Client::handshake(stream, ns,
  identity)` performs the same versioned handshake over any transport and
  `connect`/`connect_with_identity` delegate to it. Agreement tests pin the
  UDS path byte-for-byte against a second transport, so the wire contract
  stays single-sourced (no `CONTRACT_VERSION` bump; no snapshot change).
- **CA6 — recall-before-work as an agent capability**: the prompt-time
  recall injection contract (top-k=5 under budget, ≤200 chars/item, the
  W2.5 untrusted-data preamble, W3.3 source-aware suppression) is now
  defined agent-agnostically in `rb_agents::recall_contract`
  (`PROMPT_TIME_RECALL`); the Claude Code adapter (`rb-hooks`) consumes the
  contract constants directly so the lead implementation cannot drift.
  Per-adapter mapping truth stays in the capability matrix (Claude
  `UserPromptSubmit` supported; codex/opencode/gemini recorded
  `unsupported` — no invented events); agents without a mapped event get a
  documented HTTP `/recall` integration path (`docs/AGENTS.md`,
  "Agent-agnostic prompt-time recall (CA6)").

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
### Added — User-facing retention and forgetting policy

- **`[retention]` config block** (retention PRD RET-1): a declarative,
  OFF-BY-DEFAULT forgetting policy in the user config — `enabled`,
  `max_age_days` (forget horizon), `archive_after_days` (soft stage before
  forget), `importance_floor` (default 6), `protected_tags`, `batch_limit`.
  Deliberately stricter than every other section: unknown keys fail closed
  (`deny_unknown_fields`) and out-of-range/incoherent values abort resolution
  instead of warn-and-ignore — a typo in a policy that mutates memories must
  never silently no-op or broaden scope.
- **`rusty-brain forget`** (RET-2): bare invocation is a DRY-RUN listing
  exactly the set one pass would touch, with reasons (age, effective/author
  importance, last-recalled, archived state, matched rule; summaries pass
  through the shared redactor in BOTH output modes). `--apply` archives
  (soft, reversible, prunes vectors); `--hard` irreversibly purges
  row/FTS/vectors/feedback/history (one namespace-stamped `purge` oplog
  marker remains; freed bytes are zeroed via scoped `secure_delete` + WAL
  truncate, asserted by a raw-bytes drill), is peer-gated like `scrub`, and
  its EXECUTION additionally requires an interactive confirmation showing
  the plan — or an explicit `--yes` for automation; non-interactive
  invocations (`--json` or piped stdin) refuse without `--yes`. Bounded per
  pass and re-runnable; a mid-pass failure keeps the completed work (each
  memory commits in its own transaction), surfaces as a partial outcome
  ("N archived, M purged, then failed: ..."), exits non-zero, and every run
  — including zero-change and partial ones — writes the bulk
  `retention_sweep` oplog row so `last_forget_at` tracks runs, not
  mutations. Protected-tag matching is trimmed and ASCII-case-insensitive
  on both sides, so user-typed case/whitespace variance cannot defeat the
  guard.
  Eligibility guards are absolute: the importance floor gates BOTH the
  effective importance and the author prior (the W1.9 clamp productized —
  an authored importance-10 memory is never eligible), protected tags
  exempt entirely, and contested memories (per `active_contradicts`) are
  never swept. Dry-run and execute share one candidate query, recomputed on
  the single writer immediately before mutating.
- **Retention evolution job** (RET-3): `JobKind::Retention` runs a daily,
  apply-only (never purge) pass per namespace, spawned only when the policy
  is explicitly enabled; the `RunJob` path is a zero-work no-op without an
  enabled policy.
- **Visibility** (RET-4): `stats` reports `retention_eligible` (None =
  no policy, distinguishable from 0) and `last_forget_at`; `doctor` gains a
  static retention-policy lint (warns on a floor that would forget
  high-importance memories or an under-30-day horizon). Every sweep records
  per-memory oplog causes plus one bulk `retention_sweep` row; purge replays
  as `Archived` for subscribers.
- **Wire**: additive `Request::Forget` / `Response::ForgetPlanned` /
  `Response::ForgetDone` (no `CONTRACT_VERSION` bump); serde defaults are
  the safety contract — an absent `mode` decodes to apply (never hard) and
  an absent `dry_run` decodes to a preview (never an execute). Deliberately
  NO MCP tool: a destructive surface stays off the model-facing toolset.

### Added — Decision history and audit timeline (PRD 2026-07-02)

- **`rusty-brain history <id>` (+ `--depth`, `--json`)**: read-only timeline of
  a memory's evolution — the supersede chain in BOTH directions (prior and
  newer versions, oldest first) plus active `contradicts`/`extends`/
  `references` edges, each hop carrying summary, importance, confidence,
  created-at age, and `origin_*` provenance, with `current`/`superseded`/
  `contested` markers and a current-truth pointer. Derived entirely from
  existing rows (`memories.superseded_by` + `memory_links`): no schema change,
  no new persistence, and the whole path runs on the daemon's READ pool —
  zero writer ops (W1.8), asserted at the StoreHandle level and over a real
  socket. Each direction is one bounded recursive CTE (the `graph_neighbors`
  shape), walked independently so an ancestor entering a supersede cycle is
  still found; cycles in user-creatable data cannot hang the walk (SQL hop
  bound + `MIN(hop)` dedup; server depth clamp of 100 hops per direction,
  chain membership and edge list each capped at 200, with exact
  depth-truncation reporting), and traversal never crosses namespaces (both
  chain hops and both edge endpoints are namespace-scoped, the
  `active_contradicts` rigor). A corrupt out-of-band `importance` fails
  closed instead of decoding as 0.
- **Additive wire op `Request::History`/`Response::History`** with
  `rb_types::{MemoryHistory, HistoryEntry, HistoryEdge}` payloads — every
  top-level field serde-default, no `CONTRACT_VERSION` bump (recorded in the
  contract-drift snapshot as an additive decision).
- **MCP `history` tool (full-toolset-gated)**: exposed only under
  `RB_MCP_FULL_TOOLSET` per HIST-3, so the default `tools/list` token budget
  is unchanged; the tool remains routable when called directly.

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
