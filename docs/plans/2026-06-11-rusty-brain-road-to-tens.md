# rusty-brain — Road to Tens: Uplevel Roadmap

- **Status:** Proposed (synthesized from the 2026-06-11 intensive review + adversarial plan critique)
- **Date:** 2026-06-11
- **Author:** Brian Luby (plan drafted with Claude, critiqued by a 6-agent panel against the 57-finding review digest)
- **Baseline scores (2026-06-11 review, adversarially verified):** architecture 6.5 · correctness 6 · security 6 · retrieval 4.5 · teamfit 4 · claudecode 3
- **References:** review findings digest (F01–F57, archived from the 2026-06-11 review run); `docs/specs/2026-06-02-rusty-brain-p7-apm-distribution.md`; `docs/specs/2026-06-02-rusty-brain-p6-llm-evolution.md`

---

## 1. Operating principles

1. **Stop unbackfillable debt first.** Every memory written before provenance columns exist is anonymous forever; every memory written under a heuristic namespace needs a rename later; every secret captured before redaction persists until purged. These land in Phase 0, before standing dogfood capture begins.
2. **Measure before changing.** The real-embedding eval corpus is captured *before* retrieval semantics change, so every fix lands with before/after numbers instead of assumptions.
3. **Validation-bound, not code-bound.** P0–P5 were built in ~4 days of agentic development (275 commits, May 31–Jun 3) — with zero end-to-end validation, which is why claudecode sits at 3. Code workstreams remain days-each at that velocity; **phase elapsed time is gated by validation**: CI stabilization, macOS runners, API budget for Voyage/Claude in CI, fresh-machine installs, and (in Phase 5) a multi-week human pilot. Expect 1–3 weeks per phase 0–4, and 6–10 weeks for Phase 5a–5c combined.
4. **Phases are gate checkpoints, not serial work batches.** See §12 for the parallel tracks and the short serial spine that actually constrains ordering.
5. **Every phase has a falsifiable gate.** A gate is a test or drill that can fail, not a review ritual.

---

## 2. What "10" means per dimension

Each clause is tagged with its owning workstream; clauses with no owner appear in the Phase 6 closure table (§10) as accepted gaps. Unowned criteria are the failure mode this section exists to prevent.

- **Architecture 10** — cross-crate contracts (paths, env allowlist, timeouts) defined once with agreement tests [W0.2]; config file replaces env-var sprawl [W0.2]; transport survives idle/restart [W0.1] and version skew per a written protocol-evolution policy [W5a.4]; perf budgeted in CI: recall p95 < 150 ms and remember p95 < 100 ms at 10k memories on the CI runner [W4.2]; ANN decision made from measured data [W4.2]; zero advertised-but-unreachable features [W2.2, W4.4]; observability: `status` reports writer health, WAL size, corpus stats, subscriber state [W1.6, W4.3]; signed release artifacts with a one-line install [W0.6]; documented platform policy [§15].
- **Correctness 10** — no known data-misroute or data-loss paths [W0.2, W0.3]; RAII transactions, writer recovers from Err-path COMMIT failures, daemon never zombies [W1.6]; degraded keyword-only recall when embeddings are down [W1.6]; fault-injection suite: kill mid-write, disk full, API outage [W4.3]; idle/reconnect e2e with injectable timeouts [W0.1]; every migration tested against populated prior-version DB fixtures [W1.1 ground rule]; macOS in the CI matrix [W0.2]; embedding-model identity is an open-time invariant [W0.2].
- **Security 10** — DB 0600/dir 0700 parity with the socket [W0.5]; capture-time secret redaction benchmarked against external leak corpora, plus retroactive `scrub` [W0.5 minimal, W2.4 full]; injected memory framed as untrusted data with an injection drill that must pass [W2.5]; provenance + trust tiers in schema and ranking [W0.5, W2.2, W5c.1]; peer-credential check; admin ops gated [W2.6]; written threat model [W2.6]; scheduled fuzzing of the frame decoder, FTS escaping, and hook JSON parsing [W4.3]; signed releases with provenance attestation [W0.6]; cargo-deny/audit stay green (already true — claimed credit).
- **Retrieval 10** — true cosine metric, declared in DDL, thresholds recalibrated in distance units [W1.1]; tokenized FTS (AND-of-terms, tokenizer chosen from eval evidence) [W1.2]; score floor with a model-legible empty state [W1.3]; queries embedded as queries [W1.4]; graph signal = real hop distance [W1.5]; vectors pruned on archive/supersede, KNN partitioned by namespace [W1.7]; recall issues zero writer ops [W1.8]; importance recalibration respects author intent [W1.9]; write-time near-dup suppression [W3.1]; RRF/confidence/contested all reachable [W2.2]; eval: ≥200-memory corpus, ≥50 graded golden queries, a held-out query set, and a ≥5k-distractor variant, gating CI at recall@5 ≥ 0.8 / MRR ≥ 0.7 on both [W1.0, W4.1]; hybrid demonstrably beats BM25-only and vector-only on the held-out set [W4.1].
- **Teamfit 10** — provenance on every memory [W0.5]; namespace = repo-committed declared identity [W0.3]; durable sequence-numbered oplog with cursor replay [W0.5 schema, W2.7 consumers, W5a.2 replication]; shipped team mode: authenticated hub, promote/curation workflow, per-author trust weighting, conflict surfacing [Phase 5]; data-lifecycle ops: backup/restore, hard-delete purge that cascades to vectors/FTS/oplog, retention policy [W5b.3]; one-command teammate onboarding (snapshot + oplog tail) [W5b.4]; validated by a 2+ user / 2+ machine pilot including restore, purge, mid-pilot onboarding, and poisoning drills [Phase 5c gate].
- **Claudecode 10** — out-of-the-box capture→idle→recall on clean macOS+Linux from a signed artifact [Phase 0 gate]; three-channel elicitation (deterministic UserPromptSubmit injection, CLAUDE.md policy fragment/skill, trigger-condition tool descriptions + MCP instructions) with a measured elicitation scorecard [W3.2]; capture produces decision-grade memories — one summary per session via SessionEnd, zero per-event memories [W3.1]; SessionStart injection source-aware and budgeted ≤ 600 tokens [W3.3]; recall rendered as compact markdown with age and contested markers [W3.3]; token-accounting CI test: tools/list ≤ 900 + instructions ≤ 150 + injection ≤ 600 tokens [W3.3]; native distribution: plugin, committed `.mcp.json`, project settings template, `permissions.allow` entries [W3.6]; A/B outcome eval (memory-on vs memory-off vs CLAUDE.md-only) wins on success/turns within token budget, with stale-memory harm cases enumerated [W3.5].

---

## 3. Phase 0 — Stop the bleeding (unbreak + unbackfillable debt + install story)

Target: claudecode 3→6, correctness +1, teamfit +1. Everything here either unblocks end-to-end use or stops debt that cannot be backfilled later.

- **W0.1 Transport resilience.** Reconnect-with-backoff in `ClientProxy::call` and the change-subscriber loop (or connection-per-call — decide by measuring reconnect cost); `REQUEST_IDLE_TIMEOUT` (`rb-daemon/src/server.rs:45`) becomes env-injectable so the idle test runs in seconds; subscriber staleness surfaced in `poll_changes` output. *(F01/F11/F48, F57)*
- **W0.2 One config truth + corpus integrity.** Single-source path defaults and FORWARD_ENV: keep `rb-daemon/src/paths.rs` as canonical (rb-hooks already transitively depends on rb-daemon) or extract `rb-config`; delete the copies in `rb-hooks/src/main.rs` (paths), `rb-agents/src/daemon.rs` and `rusty-brain/src/client.rs:90-100` (FORWARD_ENV — it exists in **three** places); **add `RB_EMBED_BACKEND`, `RB_LOCAL_MODEL`, `RB_JOBS_CONFIG`** to the allowlist; agreement test pinning hooks==CLI==daemon resolution; add a macOS CI runner. Introduce `~/.config/rusty-brain/config.toml` (+ repo-committed `.rusty-brain.toml`, which W0.3 needs anyway) so the env allowlist shrinks to secrets and XDG/HOME — retiring the F20 bug *class*, not just the instance. **Seed and verify `embedding_model` in `meta` alongside `embedding_dim`; fail closed on mismatch with a `reembed` remediation hint** — mixed vector spaces must be impossible before dogfood capture starts. *(F03/F12/F49, F20, F10)*
- **W0.3 Canonical namespace identity.** Resolution order becomes: repo-committed `.rusty-brain.toml` > `--namespace` flag / env > git-toplevel name > cwd; the CLAUDE.md ancestor walk is bounded at the git toplevel and **demoted to legacy compat**. The `project:` frontmatter override is no longer silently trusted: interactive use warns and pins the override on first use per directory (SSH known-hosts style); non-interactive hooks fall back to the git-root name and log the unconfirmed override — a malicious repo's own CLAUDE.md must not silently claim another project's namespace. Apply identically in `rusty-brain/src/namespace_detect.rs` and `rb-agents/src/namespace.rs` (or unify them); ship a one-time namespace-rename helper for existing rows. Test: the F22 clone-a-malicious-repo scenario, plus same repo cloned under two different directory names on two hosts resolves the identical namespace string. *(F04/F13/F54, F22, F40)*
- **W0.4 MCP `update` tool honesty.** Add a validation-class variant to `rb_types::Error` (none exists today — the engine returns `Error::Storage`, which `error_map` masks); the content-update rejection returns it message-verbatim with a leak-prevention test; the tool schema documents the limitation. (W3.1's update-as-supersede may later lift the limitation — wording should not preclude that.) *(F55)*
- **W0.5 Provenance + oplog migration (the unbackfillable one).** Additive migration: `origin_user`, `origin_host`, `origin_agent`, `origin_source` (hook|mcp|cli|job), `session_id` on memories; a durable `memory_oplog` table (monotonic seq, site_id) appended **in the same transaction** as every mutation from day one; optional serde-default identity field on the Handshake (one CONTRACT_VERSION bump, coordinated with any other Phase 0 protocol change); hook-written memories get `confidence < 1.0`. Constraints that keep it clean: **no backfill UPDATE on memories** (the `mem_au` trigger would rewrite FTS for every row); old rows keep NULL provenance; new columns decoded by name in `row_to_note` with serde defaults per the `contested` precedent. Also in this phase: **DB file 0600 / dir 0700 at creation** (two-line fix, parity with the socket), and a **minimal redaction pattern set** (bearer tokens, `key=value`, AWS-style keys, PEM blocks) in hook capture — full benchmarked redaction comes in W2.4. *(F38, F41, F43, F23, F24-minimal, F39-partial)*
- **W0.6 Distribution v0.** cargo-dist (or a release.yml) producing signed, checksummed macOS+Linux binaries on tag, with provenance attestation; one-line install documented in README. The Phase 0 gate installs from this artifact, not a source build. *(prereq for every later gate; claudecode/architecture 10 criteria)*
- **W0.7 Claude Code hook-form verification.** Record a real Claude Code `settings.json` + hook-event fixture set; assert the installer's emitted hook config round-trips against it; if the separate-`args` form is not honored by current Claude Code, switch to the shell-quoted single-string form already used for Gemini/Codex. *(F50 — contested between review verifier and critique panel; resolve empirically, the fixture is needed regardless)*

**Phase 0 gate (three tiers, replacing "fresh macOS machine" as a manual ritual):**
(a) **per-PR CI**: path-agreement tests + fixture-driven hook lifecycle e2e on ubuntu *and* macos runners, idle test with injected short timeout; (b) **nightly, allowed-to-fail with alerting**: real Claude Code headless (`claude -p`) smoke on macOS with an API-key secret — capture → idle → recall reaches **one** daemon and **one** 0600 DB; (c) **once per phase**: scripted fresh-machine checklist (`scripts/verify-fresh-install.sh`) using the W0.6 artifact. Plus: a hook-written memory carries author/source/seq/confidence; planted fake secret yields zero plaintext grep hits in the DB; two clones of the same repo resolve the same namespace.

**Phase-0 carryover debt (recorded deferrals — must land before/with early Phase 1):**

- **W0.2 config file.** `~/.config/rusty-brain/config.toml` was not introduced; the env allowlist grew to 13 entries (the three specified vars plus `RB_ACCEPT_MODEL_CHANGE` and `RUSTY_BRAIN_IDLE_TIMEOUT_SECS`) instead of shrinking to secrets + XDG/HOME. The F20 bug *class* (config reaching a foreground daemon but not auto-started ones) stays open — only known instances are fixed, and every new daemon knob must remember to extend `FORWARD_ENV` until the config file lands.
- **W0.3 namespace-rename helper.** The one-time wire op + `rusty-brain namespace rename` was deferred. §11's dogfood-data lifecycle depends on it: memories captured before a repo pins identity via `.rusty-brain.toml` land under the heuristic directory-name namespace and cannot be re-scoped until it ships — land it pre-dogfood.
- **W0.7 hook-event fixtures.** Only the real Claude Code `settings.json` was recorded; real hook-event payload fixtures (PostToolUse/SessionStart/Stop/PreCompact as Claude Code actually emits them) are still missing — rb-hooks integration tests run on hand-authored payloads. W3.4 and gate tier (a)'s "fixture-driven hook lifecycle e2e" assume the full set.
- **Gate tiers (b)/(c).** The nightly real-Claude-Code smoke workflow does not exist (blocked on the S1 spike: prove `claude -p` fires hooks headlessly), and the planted-secret check is asserted at the wire (Remember payload), not by grepping a DB file.

---

## 4. Phase 1 — Measure, then fix retrieval semantics + hard robustness

Target: retrieval 4.5→7, correctness →8. **Ground rule:** every retrieval-semantics workstream lands with a re-captured `baselines.json` in the same commit, with a one-line justification of which metric moved and why (the compile-time include at `rb-eval/src/runner.rs:52` hard-gates CI otherwise).

- **W1.0 Real-embedding eval corpus FIRST.** Extend rb-eval's existing real-model mode to record/replay real Voyage vectors as committed fixtures, keyed on `(model_id, input_kind, text)` (W1.4-proof); ≥200 memories, ≥50 graded golden queries + a held-out set; per-channel (FTS/vector/graph) hit-contribution counters in the daemon; capture the pre-Phase-1 baseline (measurement only — CI gating thresholds come in W4.1). Corpus authoring is content work that starts during Phase 0. *(F35)*
- **W1.1 Cosine metric — open-time code rebuild, not a SQL migration.** The vec0 table is created in code at open with a runtime dim, and vec0 implements no `xRename`, so the rebuild is: in `SqliteStore::init` after `run_migrations`, if `meta.vector_metric != 'cosine'`, then in one `BEGIN IMMEDIATE`: stash `memory_id, embedding` to a plain table via vec0 fullscan, `DROP` the virtual table, recreate with `distance_metric=cosine` (supported by vendored sqlite-vec 0.1.9; vectors are unchanged bytes — no re-embed), re-insert, set the meta marker. Sequence it in the **writer's** open path before the read pool spins up (single-flight by construction; avoids busy_timeout failures on large corpora). Recalibrate thresholds **in distance units** (linker 0.6 under L2 ≈ cos 0.82 → choose cosine-distance ≈ 0.18 for parity; consolidation 0.95 similarity re-derived); one-shot revalidation pass re-scores existing similarity-produced links and drops/flags those below the recalibrated threshold. Tests: an L2-vs-cosine distinguishing test using non-unit vectors inserted via the store API, and migration tests against **committed populated previous-schema DB fixtures**. *(F02/F14/F28)*
- **W1.2 FTS tokenization.** Per-token escaping, AND of quoted tokens (+ trailing prefix match where useful); tokenizer decision (unicode61 vs porter) made from W1.0 eval evidence and recorded; injection tests retained; the README quickstart query becomes a test. *(F09/F18/F29)*
- **W1.3 Score floor + empty state.** Recall may return fewer than limit or nothing; below-floor/empty results return a compact model-legible empty state (`{results: [], hint: "no stored memories match"}`); SessionStart injects zero tokens on an empty corpus (first-session scenario tested in the W3.4 harness). *(F30)*
- **W1.4 Query-kind embeddings.** `EmbedKind { Query, Document }` on the provider trait (all five impls update); Voyage uses `input_type="query"` for recall. **DeterministicProvider MUST ignore the kind** (test: `embed_query == embed_document` for identical text) to preserve persisted corpora and the eval baselines. If the composite-doc asymmetry is changed on the document side, bump `EMBEDDING_INPUT_VERSION` and converge via the existing stale-stamp reembed scan; a query-side-only change needs no migration. *(F36)*
- **W1.5 Real graph hops.** `graph_neighbors` returns `(MemoryId, hops)` via `SELECT node, MIN(d) ... GROUP BY node ORDER BY MIN(d)` (the CTE already computes depth; the UNION currently dedups `(node, d)` pairs, so MIN-GROUP-BY is required, not just exposing `d`); ripple through `MemoryBackend`, rb-eval's backend, and `build_signals`; re-capture baselines. *(F06/F15/F31)*
- **W1.6 Writer robustness, named mechanisms.** (a) rb-store: convert `insert_memory`/`supersede`/`update_vector` to RAII via `Transaction::new_unchecked(conn, Immediate)` with drop-rollback — covers failed COMMITs; (b) store_handle: after a completed-with-Err op, if `!is_autocommit` (exposed via SqliteStore), drop+reopen via the existing panic-path machinery; (c) writer death raises an internal shutdown signal raced in `Server::run`'s existing `select!` — no more zombie daemons that pong Ping while every write fails; (d) recall degrades to keyword+graph when the embedder errors, flagged in the response. *(F07/F16, F17, F19)*
- **W1.7 Vector hygiene + namespace partitioning.** Archive/supersede delete the `memory_vectors` row in the same transaction; one-shot cleanup migration prunes vectors for already-archived rows; recreate the vec0 table with a `namespace` **partition key** (supported by sqlite-vec 0.1.9) so KNN scopes per namespace and the 4096 cap applies to live, in-namespace candidates — fold into the W1.1 rebuild so the virtual table is rebuilt once. Test: a namespace whose live rows are <1% of all vectors still fills `limit`. *(F44, second half of F08)*
- **W1.8 Read-path write amplification.** Recall must issue zero writer-thread ops: change `mem_au` to `AFTER UPDATE OF content, summary, keywords, tags` (migration) **or** move `access_count`/`last_accessed_at` to a side table with batched bumps. Test: a recall of N results triggers zero FTS writes. *(first half of F08)*
- **W1.9 Importance recalibration respects author intent.** Author-set importance becomes a prior modulated within a bounded delta (e.g. `clamp(base + k·tanh(signal) − decay, base−2, base+2)`), or the job ships disabled by default until the W3.7 usefulness signal exists. Property tests: importance-10 never falls below 8 from access signals alone; rb-eval runs with evolution jobs off. *(F33)*

**Phase 1 gate:** W1.0 corpus shows improved recall@5/MRR vs the pre-Phase-1 baseline; FTS channel contributes hits on ≥80% of natural-language goldens (per-channel counters); the README quickstart query returns its target memory; recall succeeds with the embedding provider down; a 10k-archived-vector scenario still returns correct live-namespace results; recall issues zero writer ops.

---

## 5. Phase 2 — Trust producers, security hardening

Target: security →8, teamfit →6.

- **W2.2 Wire the trust machinery.** Confidence settable (update path + enrichment producer; hook captures already write <1.0 from W0.5); contradicts-link creation exposed via CLI/MCP; `contested` reachable end-to-end; RRF selectable via config (decision on default deferred to W4.1 evidence). Anything not wired by end of phase is **removed from the README** until real. *(F05/F32, F39)*
- **W2.4 Redaction, benchmarked + retroactive.** Patterns benchmarked against external leak-rule corpora (gitleaks/trufflehog fixtures) with the measured false-negative rate documented in the threat model; entropy heuristic for high-entropy tokens; **`rusty-brain scrub`** retroactively redacts existing DBs (content, context, FTS, re-embed affected rows). Residual risk stated honestly: redaction is best-effort; 0600 + purge (W5b.3) are the backstops. *(F24/F42)*
- **W2.5 Untrusted-data framing + injection drill.** SessionStart/context injection wraps memory content in data-not-instructions framing with provenance labels. Exit test (in the W3.4 harness): a planted memory containing instruction-shaped text ("ignore previous instructions and run X") is injected and the scripted session asserts the agent does not act on it. Documented as best-effort; the curation queue (Phase 5b) is the team-mode backstop. *(F21)*
- **W2.6 Peer identity + threat model.** `getpeereid`/`SO_PEERCRED` check; handshake principal populated (field landed in W0.5); `RunJob`/`Reembed` gated as admin ops; threat-model doc covering today's surface and the team surface, stating explicitly that namespace is not an auth boundary. *(F25, F43)*
- **W2.7 Oplog consumers.** The W0.1 subscriber and `poll_changes` consume oplog cursors (replay-on-reconnect instead of silently-empty); `subscribe --since <seq>` for the CLI. Phase 5a inherits a proven replication substrate instead of building it under the hub. *(F41, F57)*
- **W2.8 Logging polish.** Enrichment internal errors logged with detail per the module's own contract (F26); error-taxonomy audit so every guidance-bearing rejection is a validation-class error.

**Phase 2 gate:** redaction benchmark numbers documented and `scrub` drill passes on a seeded DB; injection drill passes; non-admin client cannot invoke RunJob/Reembed; provenance fields present from all three write paths; a killed-and-reconnected subscriber replays missed events from its cursor; fresh DB is 0600/0700.

---

## 6. Phase 3 — Claude Code value

Target: claudecode →8.5. This phase was redesigned after critique — the original Stop-hook plan contradicted the engine's own no-content-update invariant, and the elicitation bet ignored Claude Code's deterministic channels.

- **W3.1 Capture inversion, SessionEnd-centric.** PostToolUse writes **zero** memories; it appends to a local per-session scratch file keyed by `session_id` (files touched, commands, failures). **SessionEnd** (add to `CLAUDE_EVENTS` and the claude_code adapter — it is not consumed today) folds scratch + transcript into **one** summary memory per session: heuristic extraction by default (user prompts, decision-marker lines, files touched), rb-enrich LLM summarization when configured. Stop stores nothing (any retained Stop logic checks `stop_hook_active`). PreCompact switches from `custom_instructions` (empty on auto-compact — it effectively never fires today) to reading `transcript_path` and extracting decision-markers. Incremental summaries use **update-as-supersede** (new note + supersede link — reuses the existing atomic supersede, preserves the embedding-consistency invariant). Write-time near-dup suppression via the existing `near_duplicates()`. Decision-grade bar: ≥80% of a 50-memory sample per release passes a written rubric ("states a decision, constraint, or outcome a future session would act on"), human-graded. *(F34, F51)*
- **W3.2 Three-channel elicitation (ranked).** (a) **Deterministic recall**: a UserPromptSubmit hook (the event reaches the adapter today and is dropped as `Other`) runs recall on the user's prompt and injects top-k under a token budget via `additionalContext` — recall stops depending on the model electing to call a tool; (b) **policy**: the installer optionally appends a 4–6 line memory-policy block to project CLAUDE.md, and/or ships a skill; (c) **tool surface**: trigger-condition descriptions ("Use when the user states a decision, preference, or constraint…") + MCP `instructions` (cheap; currently omitted from initialize). Installer adds `permissions.allow` entries for `mcp__rusty-brain__*` — without allowlisting, every unprompted call stalls on an approval prompt. Channel ranking validated by the W3.5 transcript metrics. **Elicitation scorecard**: K≥10 scripted sessions where the user states a decision with no mention of memory; pass = remember fires in ≥70%, recall-before-work in ≥50%; tracked per release. *(F52)*
- **W3.3 Token economy.** Projection implemented **exclusively in rb-mcp's `response_to_content`** (the wire Response and CLI `--json` keep the full shape — no CONTRACT_VERSION bump; rb-eval is unaffected). Render recall/list/context as markdown, one line per result: `N. [type, imp 8, 3d ago] summary-or-first-200-chars (id 1a2b3c)` + `⚠ contested` marker; age is decision-critical and was missing from the draft's 7-field projection; score bucketed or omitted; full JSON only as optional structuredContent. SessionStart injection is **source-aware** (full digest on `startup`; nothing on `resume`; constraints-only after `compact`), budget ≤600 tokens / ≤10 items, preferring constraint + architecture_decision at importance ≥8, with the W2.5 framing and a "use recall for older decisions" pointer. Default MCP toolset shrinks to remember/recall/get/context (+update); poll_changes/graph/delete/list behind `RB_MCP_FULL_TOOLSET`. **Token-accounting CI test**: tokenize the actual tools/list + instructions + injection payloads; fail above 900/150/600. *(F53, F56)*
- **W3.4 Fixture harness.** Recorded real Claude Code settings.json + hook payloads (started in W0.7) expanded to full session-lifecycle coverage on macOS + Linux; the empty-corpus first-session scenario; the W2.5 injection drill lives here.
- **W3.5 A/B outcome eval (nightly, not per-PR).** ≥10 scenario pairs run headless via `claude -p` with hooks+MCP installed: session 1 plants a decision/constraint; session 2 (fresh context, same namespace) performs a task where it matters. Arms: memory-on, memory-off, **CLAUDE.md-only** (the native baseline — this is the value-over-native proof F56 demands). Judge: deterministic assertion on output/diff where possible, LLM judge otherwise; N runs per scenario for stochasticity; report task success, turns-to-completion, token cost, and **memory-induced errors (stale/wrong memory acted on) enumerated, not averaged away**. Memory-on must win on success/turns within token budget. *(F56)*
- **W3.6 Native distribution.** Claude Code plugin (hooks + MCP config + memory skill + `/rb:remember`, `/rb:recall`) publishable via a marketplace repo; committed `.mcp.json` + project `.claude/settings.json` template for zero-effort team rollout on clone; `rusty-brain install` becomes a thin wrapper preferring native channels; execute the existing **P7 APM spec** as the cross-harness channel; document enterprise managed-settings interactions (org policy can disable hooks/MCP — a Phase 5 rollout dependency).
- **W3.7 Usefulness signal.** `memory_feedback` MCP tool (helpful/wrong/stale) as the confidence producer W2.2 needs and the non-circular input W5c trust weighting needs — `access_count` counts "returned", not "useful". *(F37)*

**Phase 3 gate:** a 40-turn simulated session produces ≤5 memories (vs ~40 today); elicitation scorecard passes; token-accounting test green; W3.5 nightly eval: memory-on beats memory-off AND CLAUDE.md-only on the scenario set; decision-grade rubric ≥80% on the sampled corpus. **Begin recruiting Phase 5 pilot users now.**

---

## 7. Phase 4 — Prove it: eval gates, perf, fuzz

Target: retrieval →9, architecture →9, security →9.

- **W4.1 Eval gating.** Promote W1.0 measurements to CI gates: recall@5 ≥ 0.8 / MRR ≥ 0.7 on the authored corpus **and** the ≥5k-distractor variant (golden queries inside hook-capture-shaped noise); the held-out query set (never used during weight tuning) gates CI; baseline comparisons (BM25-only / vector-only / hybrid / RRF) reported on the held-out set; RRF default decision from this evidence.
- **W4.2 Perf budgets.** Criterion benches + e2e latency: recall p95 < 150 ms, remember p95 < 100 ms at 10k memories on the CI runner, gated; 100k-memory load test informational; empirical brute-force-KNN ceiling documented; ANN trigger point decided from data. (W1.7/W1.8 already removed the known pathologies — benches measure honest numbers, not artifacts.)
- **W4.3 Fault injection, soak, fuzz.** Concurrent-client soak; kill −9 mid-write; disk full; embedding-API outage (proves W1.6 degraded mode); cargo-fuzz targets for the frame decoder, FTS escaping, and hook JSON parsing — short per-PR, long nightly, **scheduled** (a posture, not an event).
- **W4.4 Docs truth pass.** Every README/ARCHITECTURE claim mapped to a reachable production path; `status` surface verified to report writer health, WAL size, corpus stats, subscriber state.

**Phase 4 gate:** all eval and perf gates green in CI; fault-injection suite green; fuzz corpus running on schedule with zero outstanding crashes; docs-truth audit clean.

---

## 8. Phase 5 — Team mode (split: 5a substrate, 5b hub, 5c trust & pilot)

Target: teamfit →9.5+, security →9.5+. Design first: **P8 spec** precedes 5a. Shape (endorsed independently by the review, the critique panel, and the clean-room plan): **curated central hub with explicit promote semantics over the oplog — not live multi-master sync.** The supersede/conflict semantics and writer guards assume a single coordinator; keep that assumption and team mode is an extension, not a rewrite.

### Phase 5a — Team substrate
- **W5a.1** Handshake identity finalized (consumes the W0.5 field; shape pinned by spike S2 so the contract bumps once); token auth (or mTLS/SSH-tunnel per S2).
- **W5a.2** Oplog replay API: `Pull since seq` / `Push batch`, idempotent apply over the existing UUID + tombstone + supersede primitives; per-site seq tracking.
- **W5a.3** Transport genericization: `Client<S>`, listener seam (codec/framing already stream-generic; sized at ~a day).
- **W5a.4** Protocol evolution policy doc: additive serde-default fields never bump CONTRACT_VERSION; breaking changes bump it and the hub supports N and N−1 for one release; CI handshakes an N−1 fixture client.
- **Gate:** two daemons on two machines exchange a promoted memory, with replay after restart; N−1 client fixture connects.

### Phase 5b — Hub + promote
- **W5b.1** Shared central daemon over the authenticated transport; local daemon remains the private store; explicit `promote` of curated memories to the team namespace, pull of team changes.
- **W5b.2** Curation queue: promoted memories reviewable before they enter teammates' injection paths (the team-mode injection backstop).
- **W5b.3** Data-lifecycle ops: `rusty-brain backup`/`restore` (SQLite online-backup API); `purge <id|--author|--pattern>` hard-delete cascading memories+vectors+FTS+oplog (oplog entry redacted to a content-free tombstone), propagated hub→spokes; retention policy config. A shared brain with no backup and no way to excise a secret or a departed teammate's data fails any review.
- **W5b.4** Onboarding bootstrap: initial snapshot fetch + oplog tail from the snapshot's seq; one-command `rusty-brain join <hub>`.
- **Gate:** 3-machine smoke with curation queue; restore drill; purge drill (planted secret provably absent from DB, FTS, vectors, and oplog afterward).

### Phase 5c — Trust & pilot
- **W5c.1** Per-author trust weighting in ranking (fed by W3.7 feedback + provenance); cross-author contradiction surfacing; provenance-aware curation CLI.
- **W5c.2** Pilot: 2+ real users, 2+ machines, 2 weeks; a third user onboards mid-pilot and reaches recall parity within minutes (measured); poisoning drill: a planted malicious memory is contained by framing + curation + trust.
- **Gate:** pilot completes with all drills passed and teammates' curated memories recalled with correct attribution; network kill mid-sync re-converges with zero duplicates and zero lost acked writes (DB diff after replay).

---

## 9. Spike register (time-boxed, before committing the dependent phase)

- **S1 (before the Phase 0 gate is finalized, 1 day):** prove `claude -p` fires SessionStart/PostToolUse/SessionEnd hooks headlessly with an API key in CI. If not, the nightly tier becomes fixture-only and real-CC checks stay a per-phase manual ritual — W3.5's design depends on this answer.
- **S2 (during Phase 2, 1–2 days):** hub auth design note — token vs mTLS vs SSH tunnel — pinning the handshake identity field shape so the wire contract bumps once.
- **S3 (retired):** vec0 `distance_metric=cosine` support confirmed in vendored sqlite-vec 0.1.9; rebuild mechanics folded into W1.1 as open-time code.

---

## 10. Phase 6 — Last mile to 10 (closure table)

Run the full review battery (see §13) and close the residue. Every "what 10 means" clause must by now map to a shipped workstream or appear below as an accepted, stated gap:

| Dimension | Residual clause | Owner / resolution |
|---|---|---|
| architecture | version-skew survival | W5a.4 (policy + N−1 CI fixture) |
| architecture | Windows | accepted gap, documented (§15): WSL2 path; revisit when W5a.3 TCP lands |
| correctness | upgrade-path coverage for *every* future migration | standing rule from W1.1: populated prior-version fixtures, enforced in review checklist |
| security | continuous fuzzing + signed releases | W4.3 + W0.6 (verify both still on schedule) |
| retrieval | multilingual | accepted gap (§15) |
| teamfit | GDPR-grade deletion semantics beyond purge | accepted gap unless pilot demands it; documented in threat model |
| claudecode | sustained elicitation + outcome wins across releases | W3.2 scorecard + W3.5 A/B tracked per release, regression = release blocker |

**Final gate:** all six dimensions ≥9.5 on a fresh full review **plus** external evidence: pilot metrics, the A/B outcome results, and at least one fresh-eyes install by someone who has never used the tool.

---

## 11. Dogfood-data lifecycle through the migrations

Pre-provenance, pre-redaction, pre-cosine memories accumulate during Phases 0–2. Policy: cosine rebuild migrates vectors automatically (W1.1); namespace rename helper re-scopes rows (W0.3); `scrub` (W2.4) is run before any DB is shared; **no local store is promoted or joined to a hub until scrubbed** — enforced as a check in `rusty-brain join`/`promote` (pre-redaction rows carry no `redaction_pass` stamp). Anonymous (NULL-provenance) rows are never auto-promoted.

---

## 12. Parallel tracks & the serial spine

Phases are gate checkpoints; with one developer orchestrating agents, run as tracks:

- **Track A — transport/robustness:** W0.1, W0.4, W1.6
- **Track B — retrieval:** W1.0 corpus authoring (starts during Phase 0) → W1.1 → W1.2–W1.5, W1.7–W1.9
- **Track C — capture/claudecode:** W0.7 → W2.4 → W3.1–W3.7
- **Track D — quick wins, landable any time:** DB perms (in W0.5), W2.5 framing, W2.6 threat-model doc, W2.8 logging

**The spine that must stay ordered:** W0.2 paths → everything e2e; W0.5 provenance/oplog → W2.2/W2.7 → Phase 5; W1.0 → W1.1 → threshold recalibration → W4.1 gating; W3.1 capture quality → W3.5 eval (a corpus worth searching) → Phase 5 sharing.

---

## 13. CI & cadence policy

- **Test tiers:** per-PR = unit + agreement + fixture e2e with injected short timeouts (minutes; any test >60 s wall-clock must justify per-PR placement); nightly = soak, fault injection, 10k/100k benches, real-Claude-Code smoke (allowed-to-fail with alerting), long fuzz, W3.5 A/B; per-phase = manual fresh-machine checklist.
- **Review cadence:** dimension-scoped re-reviews at each phase boundary (Phase 1 → retrieval+correctness agents only, etc.); the **full** intensive review at the Phase 2 and Phase 4 boundaries and pre-pilot. Phase 3+ scorecards must include external evidence (pilot metrics, A/B results, fresh-eyes installs) — self-grading against a known rubric overfits to the reviewer.
- **Baseline discipline:** retrieval changes land with re-captured baselines + justification in the same commit (§4 ground rule).
- **Contract-version policy:** additive serde-default fields: no bump. W0.5 (Handshake identity) and any Phase 0 protocol change share **one** bump. Breaking changes thereafter follow W5a.4.

---

## 14. Score trajectory (honest, sums to the title)

| Phase | arch | corr | sec | retr | team | cc |
|---|---|---|---|---|---|---|
| baseline | 6.5 | 6 | 6 | 4.5 | 4 | 3 |
| P0 | 7 | 7 | 6.5 | 4.5 | 5 | 6 |
| P1 | 7.5 | 8 | 6.5 | 7 | 5 | 6.5 |
| P2 | 8 | 8 | 8 | 7.5 | 6 | 7 |
| P3 | 8 | 8 | 8.5 | 8 | 6.5 | 8.5 |
| P4 | 9 | 9 | 9 | 9 | 6.5 | 9 |
| P5a–c | 9.5 | 9.5 | 9.5 | 9.5 | 9.5 | 9.5 |
| P6 | 10* | 10* | 10* | 10* | 10* | 10* |

\* "10" = every exit criterion in §2 demonstrably met or explicitly accepted-and-documented in §10's table. A 10 with stated non-goals is honest; a 10 by omission is not.

---

## 15. Deliberate non-goals (beyond 10 for this product)

- **Multilingual retrieval** — dev-team memories are English/code-dominant; unicode61 handles incidental non-English.
- **Windows tier-1** — out of scope for v1; WSL2 documented as the supported path; revisit when the team-mode TCP transport removes the UDS blocker.
- **At-rest encryption** — 0600 + OS full-disk encryption is the documented posture.
- **Live multi-master sync (CRDTs)** — hub-and-spoke with promote semantics chosen deliberately (§8); revisit only with pilot evidence that curation is the bottleneck.

---

## 16. Traceability appendix — finding → workstream

| Finding | Workstream(s) | | Finding | Workstream(s) |
|---|---|---|---|---|
| F01/F11/F48 idle-death | W0.1 | | F30 junk default recall | W1.3 (+W1.0 evidence) |
| F02/F14/F28 L2-vs-cosine | W1.1 | | F31 (=F06) | W1.5 |
| F03/F12/F49 path split-brain | W0.2 | | F32 (=F05) | W2.2 |
| F04/F13/F54 namespace hijack | W0.3 | | F33 importance job | W1.9 (+W3.7) |
| F05 unreachable features | W2.2, W4.4 | | F34 low-info capture | W3.1 |
| F06/F15 graph_hops | W1.5 | | F35 eval blindness | W1.0, W4.1 |
| F07/F16 COMMIT poison | W1.6 | | F36 query input_type | W1.4 |
| F08 recall-as-write + residue | W1.8, W1.7 | | F37 no usefulness signal | W3.7 |
| F09/F18/F29 phrase-only FTS | W1.2 | | F38 zero provenance | W0.5 |
| F10 model identity | W0.2 | | F39 no trust producers | W0.5, W2.2 |
| F17 zombie daemon | W1.6(c) | | F40 namespace not a team key | W0.3 |
| F19 recall hard-fail | W1.6(d) | | F41 no durable change log | W0.5, W2.7, W5a.2 |
| F20 env allowlist | W0.2 | | F42 capture leak engine | W0.5-min, W2.4 |
| F21 injection-via-memory | W2.5, W5b.2 | | F43 no identity slot | W0.5, W2.6, W5a.1 |
| F22 frontmatter attack | W0.3 | | F44 KNN cliff | W1.7 |
| F23 world-readable DB | W0.5 | | F45/F47 (positive/sequencing) | adopted in §8 shape |
| F24 verbatim secrets | W0.5-min, W2.4 | | F46 transport coupling | W5a.3 |
| F25 no peer-cred | W2.6 | | F50 installer hook form | W0.7 |
| F26 enrichment logging | W2.8 | | F51 noise-dominant capture | W3.1 |
| F27 (positive) | — | | F52 no elicitation | W3.2 |
| | | | F53 token economy | W3.3 |
| | | | F55 update tool lies | W0.4 (+W3.1 supersede) |
| | | | F56 value-over-native unproven | W3.5 (CLAUDE.md-only arm), W3.6 |
| | | | F57 dead subscriber | W0.1, W2.7 |

Every confirmed finding has an owner; F27/F45/F47 are positives that shaped the design rather than work items.
