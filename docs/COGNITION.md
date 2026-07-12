# COGNITION.md — the rusty-brain cognitive contract

**Aligned to**: [cognition.md](https://github.com/arananet/cognition.md) spec **v0.2**
(six sections: Taxonomy, Consolidation, Retrieval, Degradation, Health, Security).
**Nature**: pure declaration. This file changes no behavior; it states the
memory-management policy the implementation already enforces and cites where each
clause is enforced (source file, migration, `W`-item from
[the Road-to-Tens plan](plans/2026-06-11-rusty-brain-road-to-tens.md), PRD, or merged PR).
Where rusty-brain does NOT implement an upstream clause, the gap is declared —
never papered over.

Section status legend:

- **Enforced** — the clause is implemented and test-covered; citations point at the code.
- **Partial** — a meaningful subset is enforced; the missing remainder is listed under *Gaps*.

| Section | Status |
|---|---|
| 1. Taxonomy | Partial |
| 2. Consolidation | Partial |
| 3. Retrieval | Enforced |
| 4. Degradation | Enforced (no per-stratum TTL — see §4 note) |
| 5. Health | Enforced (no per-memory redundancy index — see §5 note) |
| 6. Security | Enforced |

Key machine-checkable claims in this file are pinned to the code by
`crates/rb-agents/tests/cognition_docs.rs` (the
[capability-matrix doc-anchoring convention](../crates/rb-agents/tests/capability_docs.rs)):
the six section headings, the memory-type table, the CA6 recall-contract numbers,
the importance/confidence ranges, the retention floor default, and the
untrusted-data preamble fragment all fail CI if this file and the code drift.

---

## 1. Taxonomy

**Upstream asks**: four memory strata (working, episodic, semantic, procedural), each
with its own encoding depth, retention policy, and retrieval protocol.

**rusty-brain declares**: one durable store with a typed classification and global
(not per-stratum) policy dimensions. There is deliberately no four-strata model:

- **Working memory is the agent's context window**, not a rusty-brain stratum.
  The only working-memory analog rusty-brain keeps is the per-session scratch
  state (`crates/rb-hooks/src/scratch.rs`), which exists solely to be folded into
  one durable session summary at SessionEnd (`crates/rb-hooks/src/capture.rs`).
- **Everything durable lives in a single episodic/semantic store** (`memories`
  table, `crates/rb-store/migrations/001_initial_schema.sql`), classified by type
  rather than stratum.

Declared memory types (`MemoryType`, db strings):

| Type | Holds |
|---|---|
| `architecture_decision` | a decision and its rationale |
| `code_pattern` | a recurring implementation pattern |
| `bug_fix` | a defect and its resolution |
| `configuration` | settings, wiring, environment facts |
| `constraint` | a standing rule the work must respect |
| `entity` | a person, system, or component fact |
| `insight` | a derived observation |
| `reference` | a pointer to external material |
| `preference` | a user/team preference |

Enforced in `crates/rb-types/src/memory_type.rs` (fail-closed `parse`, db strings in
lockstep with the SQL `CHECK` constraint).

Per-memory policy dimensions, all schema-enforced:

- **`importance`** — integer `1..=10` (`validate_importance`,
  `crates/rb-types/src/validate.rs`; SQL `CHECK`), with the author-set prior kept
  immutable as `base_importance` (`crates/rb-store/migrations/007_base_importance.sql`) so
  automation may modulate but never re-author it (W1.9).
- **`confidence`** — finite `0.0..=1.0` (`validate_confidence`,
  `crates/rb-types/src/validate.rs`; SQL `CHECK`) — the single trust axis (§6).
- **Retention protection** — the `[retention]` policy (§4) declares an
  `importance_floor` (default **6**, `rb_types::DEFAULT_IMPORTANCE_FLOOR`) and
  `protected_tags` that exempt memories from forgetting entirely
  (`crates/rb-config/src/file.rs`, `RetentionFileConfig`;
  PRD [user-facing retention policy](prds/2026-07-02-user-facing-retention-policy.md), PR #60).
- **Type-aware retrieval preference** — the SessionStart digest surfaces standing
  `constraint` / `architecture_decision` memories at importance ≥ 8 first
  (`is_preferred`, `crates/rb-hooks/src/capture.rs`, W3.3) — the one place a type
  currently changes retrieval policy.

**Gaps (declared, not implemented here)**:

- No per-type (per-stratum) **retention** policy: `importance_floor`,
  `protected_tags`, and the age horizons apply uniformly across all nine types.
- No per-type **encoding** policy: enrichment and embedding treat all types the same.
- No per-type **retrieval** protocol beyond the digest preference above; ranking
  weights are global (`crates/rb-engine/src/engine.rs`).

Concrete follow-up candidates are listed at the end of this file.

## 2. Consolidation

**Upstream asks**: promotion criteria between strata, depth-of-processing levels,
and a spaced re-evaluation schedule.

**rusty-brain declares**: consolidation by *merge and supersede*, not by promotion
between strata (there are no strata, §1). What is enforced:

- **Update-as-supersede** — a correction stores the replacement first, then points
  the old row at it and archives it in one transaction
  (`Store::supersede`, `crates/rb-store/src/store/core.rs`; the W3.1
  update-as-supersede path in `crates/rb-daemon/src/server.rs`). The chain is
  auditable in both directions (§5, decision history).
- **Write-time near-duplicate suppression** — a hook-sourced write that is
  near-identical (cosine similarity bound) to an existing active memory is
  collapsed into a supersede instead of accreting a duplicate
  (`crates/rb-daemon/src/server.rs`, W3.1). Capture repeats across compactions
  collapse the same way (`pre_compact`, `crates/rb-hooks/src/capture.rs`).
- **SessionEnd fold** — many shallow per-session scratch observations are folded
  into one structured durable summary (`crates/rb-hooks/src/capture.rs`), the
  closest analog to a shallow→structural depth-of-processing step.
- **Consolidation job** — merges near-duplicate clusters into one
  deterministically-chosen survivor via supersede; bounded, idempotent,
  namespace-isolated (`crates/rb-daemon/src/jobs/consolidation.rs`).
- **Guided review merge** — the review sweep (§5) offers an atomic merge for
  near-duplicate pairs; both originals are superseded behind a pointer guard in
  one transaction (`crates/rb-daemon/src/store_handle.rs`;
  PRD [contradiction/dedup review](prds/2026-07-02-contradiction-dedup-review.md), PR #63).
- **Usefulness signal** — the `memory_feedback` MCP tool and `rusty-brain feedback`
  CLI record helpful/wrong/stale per memory
  (`crates/rb-store/migrations/008_memory_feedback.sql`, W3.7) and nudge `confidence`
  per kind — `helpful` `+0.05`, `wrong` `-0.30`, `stale` `-0.15`, clamped
  (`FeedbackKind::confidence_delta`, `crates/rb-types/src/feedback_kind.rs`).
  This is the promotion/demotion *signal*.
- **Importance recalibration** — a bounded ±2 modulation of effective importance
  around the immutable `base_importance` from access/recency signals; implemented
  and deliberately shipped **disabled by default** until the usefulness signal has
  accumulated (`crates/rb-daemon/src/jobs/importance.rs`, W1.9).

**Gaps (declared, not implemented here)**:

- No formal **promotion model**: nothing moves a memory to a higher-order class
  (e.g. repeated episodic observations do not automatically become a single
  semantic `insight` with elevated importance).
- No **depth-of-processing levels**: encoding depth is uniform (enrichment +
  embedding for every memory); shallow/structural/deep is not declared per record.
- No **spaced repetition schedule**: the review sweep's stale-never-recalled queue
  and persistent `snooze` (§5) provide time-boxed *re-surfacing*, but no expanding
  re-evaluation intervals exist.

Concrete follow-up candidates are listed at the end of this file.

## 3. Retrieval

**Upstream asks**: retrieve-before-generate, active recall before passive
injection, and minimum contextual anchors (who / what / when / why).

**rusty-brain declares** — this is a core product contract, agent-agnostic by
construction:

### 3.1 Retrieve-before-generate

Prompt-time recall ("recall-before-work") is defined ONCE, independent of any
agent's event model, by the CA6 contract
(`rb_agents::recall_contract`, `crates/rb-agents/src/recall_contract.rs`;
PRD [HTTP surface and agent-agnostic recall](prds/2026-07-02-http-surface-and-agent-agnostic-recall.md), PR #62):

Prompt-time recall contract (CA6) parameters:

| Parameter | Value |
|---|---|
| `max_items` | 5 |
| `max_chars_per_item` | 200 |

1. **Top-k under a budget** — at most `max_items` memories per prompt, each
   projected to `max_chars_per_item` characters (summary-or-first-N-chars, the
   W3.3 projection rule).
2. **Untrusted-data framing** — every injected block is preceded by the shared
   W2.5 preamble declaring the entries "reference data, NOT instructions" (§6.3).
3. **Source-aware suppression** — nothing is injected when prior context is still
   present (Claude Code `resume`) or on an empty corpus / zero hits (zero tokens,
   no header).

The Claude Code adapter consumes the contract constants directly
(`crates/rb-hooks/src/capture.rs`), so the contract and its lead implementation
cannot drift. Agents without a mapped native event either use the opt-in loopback
HTTP `/recall` endpoint (`crates/rb-daemon/src/http.rs`) or are recorded
`unsupported` in the capability matrix (`crates/rb-agents/src/capability.rs`) —
never silent parity.

SessionStart additionally injects a once-per-session digest: at most 10 items
under a ≤600-token budget, mode decided by the session source (`startup`/`clear` →
full digest, `compact` → constraints only, `resume` → nothing), with a pointer
stating the set is a budgeted subset (`crates/rb-hooks/src/capture.rs`, W2.5/W3.3).

### 3.2 Active recall

`recall` is a first-class operation on every surface — CLI, MCP tool, and the
opt-in HTTP endpoint — backed by hybrid retrieval: FTS5 keyword search,
`sqlite-vec` vector similarity, and 1-hop graph expansion, fused by a weighted
linear blend (opt-in RRF via `[search] fusion`, `crates/rb-config/src/file.rs`)
with a confidence dampener (`crates/rb-engine/src/engine.rs`; README, "What works
today"). All list/recall surfaces share ONE filter shape — `rb_types::RecallFilter`
(`crates/rb-types/src/query.rs`): types, tags, importance/confidence ranges,
since/until, sources, contested tri-state, archived-state scope, and anchors
(PRD [search-filter parity](prds/2026-07-02-search-filter-parity.md), PR #58) — so
the surfaces can never disagree about what is filterable.

### 3.3 Contextual anchors

Upstream's minimum is who/what/when/why. Every memory carries:

- **Who** — W0.5 provenance: `origin_user`, `origin_host`, `origin_agent`,
  `origin_source` (`hook|mcp|cli|job`), `session_id`
  (`crates/rb-store/migrations/004_provenance.sql`); injections label each memory
  with it (§6.1).
- **When** — `created_at` / `updated_at` (schema), both filterable.
- **What** — typed code anchors: structured file (+ optional 1-based line range),
  commit-SHA, and symbol links, multiple per memory
  (`crates/rb-store/migrations/009_memory_anchors.sql`;
  `crates/rb-types/src/anchor.rs`;
  PRD [typed code anchors](prds/2026-07-02-typed-code-anchors.md), PR #59).
  SessionEnd auto-anchors the session summary to the files it touched
  (`session_file_anchors`, `crates/rb-hooks/src/capture.rs`, ANC-2). Anchors are
  first-class recall filters (`AnchorFilter` in `RecallFilter`).
- **Why** — typed graph links (`contradicts`/`extends`/`references`,
  `crates/rb-types/src/link_type.rs`), the supersede chain, and the `MemoryType`
  classification itself.

## 4. Degradation

**Upstream asks**: TTL per stratum, pruning triggers (conflict, obsolescence,
redundancy), and graceful handling of uncertain memories.

**rusty-brain declares**:

### 4.1 Retention instead of TTL

There is no hard per-stratum TTL (no strata, §1). The default policy is that
**memories are permanent** until superseded, forgotten explicitly, or aged out by
the opt-in declared retention policy:

- `[retention]` config (`crates/rb-config/src/file.rs`): master `enabled` switch
  (absent = off), `archive_after_days` (soft stage), `max_age_days` (forget
  horizon), `importance_floor` (default 6), `protected_tags`, `batch_limit`.
  Unknown keys in this section **fail closed** (`deny_unknown_fields`) — a typo'd
  guard must never silently drop protection.
- The sweep plans and mutates from ONE candidate query (dry-run plan and
  `retention_sweep` share it, so preview and mutation cannot drift), with absolute
  eligibility guards (`crates/rb-store/src/store/retention.rs`; PRD
  [user-facing retention policy](prds/2026-07-02-user-facing-retention-policy.md), PR #60).
- `rusty-brain forget` is guarded: soft archive by default, confirmation gates,
  and hard purge (`--hard`) is a **peer-gated admin op** (§6.2) that permanently
  destroys rows (`Command::Forget`, `crates/rusty-brain/src/cli.rs`;
  `docs/THREAT_MODEL.md`).

### 4.2 Pruning triggers

- **Conflict** — a `contradicts` link marks BOTH endpoints `contested`, surfaced
  on every read (`crates/rb-engine/src/engine.rs`) and filterable
  (`RecallFilter.contested`). The review queue (§5) puts contradiction pairs
  first; resolution is keep / merge / archive / snooze, never silent deletion
  (PR #63). Contested is surfaced, not adjudicated — see §6.4.
- **Obsolescence** — supersede archives the old row behind a `superseded_by`
  pointer (§2); `stale` feedback erodes confidence (−0.15, §2); the review queue
  surfaces stale-never-recalled singles.
- **Redundancy** — write-time near-dup suppression, the consolidation job, and
  the review sweep's near-duplicate pairs with atomic merge (§2) — all three via
  the ONE similarity definition (`SqliteStore::near_duplicates`,
  `crates/rb-store/src/store/review.rs`).

### 4.3 Graceful degradation

- Low-confidence memories are **dampened in ranking, not hidden** — recall
  degrades their weight rather than pretending certainty either way
  (`crates/rb-engine/src/engine.rs`).
- Injected recall is explicitly labeled possibly-stale, weighted "context from
  the labeled source" (the CA6 preamble, §3.1) — uncertainty is stated to the
  consumer instead of laundered.
- Link `strength` decays exponentially by age from an immutable baseline
  (`base_strength`, migration 002), floored and idempotent
  (`crates/rb-daemon/src/jobs/link_decay.rs`) — old associations fade instead of
  flipping off.
- The capture surface **fails open**: when the daemon is unreachable, hooks
  abandon memory work and never block or degrade the host agent session
  (`crates/rb-hooks/src/main.rs`). Memory is an enhancement, not a dependency.

## 5. Health

**Upstream asks**: periodic coherence checks, a redundancy index per critical
memory, and consistency verification against recent evidence.

**rusty-brain declares** (all read-only by construction — observability issues
zero writer ops):

- **`rusty-brain doctor`** — diagnostics that never auto-start the daemon and
  never mutate: DB/WAL checkpoint health, socket and DB file permission modes,
  embedding model/provider coherence, daemon reachability and writer liveness
  (`crates/rusty-brain/src/doctor.rs`;
  PRD [doctor and stats observability](prds/2026-07-02-doctor-and-stats-observability.md), PR #56).
- **`rusty-brain stats` / `status`** — read-pool-only aggregates: counts, growth
  buckets, top-recalled, feedback totals — counts and ids only, never memory
  content (`crates/rb-store/src/store/stats.rs`, PR #56).
- **Review sweep** — the periodic coherence check: a priority-ordered queue of
  active contradiction pairs, near-duplicate pairs, and low-confidence /
  stale-never-recalled singles, with persistent snooze (migration 010) and an
  oplog audit row per resolution (REV-4)
  (`crates/rb-store/src/store/review.rs`;
  PRD [contradiction/dedup review](prds/2026-07-02-contradiction-dedup-review.md), PR #63).
- **Decision history** — `rusty-brain history` renders the supersede chain in both
  directions plus active links with current/superseded/contested markers, so a
  memory's evolution is auditable (`crates/rb-types/src/history.rs`;
  PRD [decision history timeline](prds/2026-07-02-decision-history-timeline.md), PR #61).
- **Contract health** — declared here as part of Health by judgment call: the
  wire surface and on-disk migrations are digested and compared against a
  checked-in snapshot on every CI run; ANY drift fails until the author records a
  deliberate additive-vs-breaking decision
  (`crates/rb-contract-guard/`; `.github/workflows/ci.yml`;
  spec: [contract drift guard](specs/2026-07-11-contract-drift-guard.md), PR #57).
- **Doc health** — user-facing claims are pinned to code by doc-anchoring tests:
  the README capability matrix (`crates/rb-agents/tests/capability_docs.rs`,
  PR #55) and this file (`crates/rb-agents/tests/cognition_docs.rs`).
- **Measured, not asserted** — retrieval quality is scored by the memory
  scorecard harness per agent and nightly live smoke runs
  (`scripts/memory-scorecard.sh`, `.github/workflows/memory-scorecard.yml`,
  `.github/workflows/nightly-claude-smoke.yml`, `crates/rb-eval/`).

**Gap (declared)**: no per-memory **redundancy index** — upstream asks that each
critical memory carry a redundancy score; the nearest analogs are the
near-duplicate similarity scan and the top-recalled/feedback aggregates, neither
of which is a per-record index.

## 6. Security

**Upstream asks**: write provenance and trust boundaries, access control across
the memory lifecycle, and poisoning detection on high-trust memories.

**rusty-brain declares** (full model: [`docs/THREAT_MODEL.md`](THREAT_MODEL.md)):

### 6.1 Write provenance

Every write path declares its provenance: `origin_user`, `origin_host`,
`origin_agent`, `origin_source` (`hook|mcp|cli|job`), and `session_id`
(`crates/rb-store/migrations/004_provenance.sql`, W0.5). Rows that predate the
migration keep honest `NULL`s — provenance was deliberately never backfilled or
faked. Injected memories carry their provenance label into the prompt (§3.1).

### 6.2 Trust boundaries and access control

- **Kernel-verified peer identity** — the daemon checks
  `getpeereid`/`SO_PEERCRED` on every UDS connection BEFORE any frame is read;
  admin ops (`RunJob`, `Reembed`, `NamespaceRename`, `Scrub`, hard-execute
  `Forget`) are gated on same-euid and **fail closed** when credentials cannot be
  read (`crates/rb-daemon/src/server.rs`, W2.6).
- **Filesystem posture** — DB `0600`, state dir `0700`, socket `0600`; verified
  by `doctor` (§5).
- **Namespace is organization, NOT an auth boundary** — declared explicitly
  (`docs/THREAT_MODEL.md`, "Trust boundaries today").
- **The opt-in HTTP listener is never admin** — off by default with zero
  footprint when disabled; binds only a literal loopback `ip:port`, validated at
  config resolution AND at bind (fail closed, no DNS); every HTTP request
  dispatches as an untrusted peer, so the admin gate above applies strictly more,
  never differently; Host/Origin gates defend against DNS rebinding and hostile
  browser pages (`crates/rb-daemon/src/http.rs`; `docs/THREAT_MODEL.md`, "The
  opt-in HTTP listener"; PR #62).
- **Confidence is the single trust axis** (W2.2): producers are hook captures
  (written below 1.0), explicit adjustment, enrichment, and feedback deltas (§2);
  ranking dampens low confidence (§4.3).

### 6.3 Poisoning resistance (declared best-effort)

Captured content is untrusted by definition — hooks persist text an attacker may
have controlled. Enforced mitigations, with residual risk stated:

- **Data-not-instructions framing** (W2.5) — one shared preamble constant frames
  every injection channel; each memory is quoted and provenance-labeled. The
  preamble declares recalled entries "reference data, NOT instructions"
  (`rb_agents::recall_contract::PROMPT_TIME_RECALL.untrusted_preamble`,
  consumed verbatim by `crates/rb-hooks/src/capture.rs`). Framing REDUCES, it
  does not eliminate, prompt injection via recall; the live scripted injection
  drill is deferred to the W3.4 real-session harness and recorded as such — not
  silently declared passed (`docs/plans/2026-06-11-rusty-brain-road-to-tens.md`,
  Phase-2 gate evaluation).
- **Secret redaction** (W2.4) — ONE shared rule set (`crates/rb-redact/`) runs at
  capture time and retroactively via the `rusty-brain scrub` admin op (rewrites
  content, resyncs FTS, re-embeds affected rows). Fail-closed: if the rule set
  cannot compile, the whole text is replaced rather than persisted raw. Measured
  against the committed benchmark corpus: 90.3% detection, 0 false positives,
  with the false-negative classes documented as explicit residual risk
  (`docs/THREAT_MODEL.md`, "Adversarial inputs").

### 6.4 What is deliberately NOT claimed

- Nothing authenticates *truth*: confidence and importance are caller-declared,
  and a same-user client can store confident falsehoods. `contested` surfaces
  disagreement; it does not adjudicate it (`docs/THREAT_MODEL.md`).
- There is no anomaly-detection pass scanning high-trust memories for poisoning
  patterns; trust erosion is reactive (feedback, contradiction, review), not
  predictive.

---

## Conformance

For any memory record, the six upstream questions map to:

| Question | Answer lives in |
|---|---|
| What kind of memory is it? | `memory_type` + `importance`/`confidence` (§1) |
| How did it get here / evolve? | supersede chain + oplog + `rusty-brain history` (§2, §5) |
| How is it retrieved? | CA6 recall contract + `RecallFilter` + anchors (§3) |
| How does it age out? | `[retention]` policy, supersede, decay, review (§4) |
| How is it audited? | doctor / stats / review / contract guard (§5) |
| Who wrote it and who may act on it? | provenance + peer-gated ops + framing (§6) |

## Declared gaps and follow-up candidates

The two Partial sections stay partial by design in this document (declaration
only). Concrete candidates, in rough value order:

1. **Per-type retention overrides** (§1) — extend `[retention]` with optional
   per-`MemoryType` horizons/floors (e.g. `insight` ages faster than
   `architecture_decision`), keeping the fail-closed unknown-key posture and the
   absolute `importance_floor`/`protected_tags` guards.
2. **Promotion job** (§2) — a bounded, idempotent job that promotes recurring
   corroborated episodic observations into a single higher-importance semantic
   memory (survivor logic exists in the consolidation job; provenance
   `origin_source = "job"` exists; the missing piece is the promotion criterion).
3. **Spaced re-evaluation** (§2) — schedule review re-surfacing at expanding
   intervals by generalizing the persisted `snooze_until` (migration 010) into a
   per-item re-review cadence driven by the existing job scheduler.
4. **Per-type retrieval weighting** (§1) — let ranking weight memory types
   differently per query class, generalizing the SessionStart
   constraint/decision preference into a declared policy.
5. **Per-memory redundancy index** (§5) — surface a near-duplicate count /
   corroboration score per high-importance memory in `stats` and the review
   queue.

None of these change this contract's claims; each would move a clause from
*declared gap* to *enforced* and should update this file in the same change.
