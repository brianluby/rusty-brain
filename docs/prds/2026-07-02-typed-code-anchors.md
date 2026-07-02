# PRD: Typed Code Anchors (`--file`, `--commit`, `--symbol`)

## Status

Draft. From the 2026-07-02 senior-PM product review. Today memories attach
only to freeform `context`; the highest-value coding memories are decisions
about *code locations*, and there is no structured, queryable link from a
memory to a file/line/commit/symbol.

## Owner Area

Primary: schema, engine, and query filters.

Touchpoints:

- `crates/rb-types/src/memory.rs` (anchor type)
- `crates/rb-types/src/query.rs` (anchor filters)
- `crates/rb-store/src/store.rs` (anchor table + queries)
- `crates/rb-store/src/migrations.rs` (additive migration)
- `crates/rb-engine/src/engine.rs` (store/recall anchors)
- `crates/rb-proto/src/messages.rs` (additive fields, no bump)
- `crates/rusty-brain/src/cli.rs` (`--file`/`--commit`/`--symbol`)
- `crates/rb-mcp/src/tools.rs` (anchor params)
- `crates/rb-hooks/src/capture.rs` (auto-anchor from PostToolUse file edits)

## Problem

A memory like "we chose tokio here, not async-std" is only findable by
free-text recall. When an agent opens `server.rs`, nothing surfaces the
memory tied to that file - even though the hook already *sees* the file path
at PostToolUse time. Anchoring memories to code locations would make recall
actionable at the exact moment it matters, far beyond flat semantic search.

## Goals

- First-class, structured, queryable anchors: file path (+optional line
  range), commit SHA, symbol/identifier.
- Recall can filter/rank by anchor ("memories touching this file") and the
  hook can auto-anchor captures to the files a session touched.
- Additive schema, backward-compatible, no CONTRACT_VERSION bump.

## Non-Goals

- Do not build a code indexer or symbol resolver (symbols are caller-supplied
  strings, not resolved AST).
- Do not change ranking weights in v1 (anchor is a filter/boost signal; exact
  weighting is deferred to W4.1 evidence).
- Do not require anchors; freeform `context` remains valid.
- Do not couple to any specific VCS beyond a commit-SHA string.

## Functional Requirements

### ANC-1. Anchor data model

A new `memory_anchors` table (additive migration): `memory_id`, `kind`
(`file`|`commit`|`symbol`), `path` (normalized), `start_line`/`end_line`
(nullable), `ref` (commit SHA / symbol name), `namespace`. Multiple anchors
per memory. Decoded by name in `row_to_note` with serde defaults (the
`contested` precedent); no backfill UPDATE on existing rows.

### ANC-2. Capture anchors

- CLI: `remember ... --file src/foo.rs:12-40 --commit <sha> --symbol Foo::bar`
  (repeatable).
- MCP: `remember` gains additive `anchors` params.
- Hooks: PostToolUse scratch already records touched files; SessionEnd fold
  auto-anchors the session summary to the files touched (best-effort, gated
  to the hook source).

### ANC-3. Anchor-aware recall

- Recall filters: `--file <path>`, `--commit <sha>`, `--symbol <name>`
  (CLI + MCP), scoping candidates before ranking.
- When the hook performs prompt-time recall (W3.2 channel a), an anchor
  derived from the active file/context boosts anchor-matching memories (a
  documented, bounded boost; exact weight deferred to W4.1).

### ANC-4. Graph + provenance parity

- Anchors participate in the graph view (`graph <id>` lists anchors).
- Anchors are included in `export` and round-trip through `restore`.

## Acceptance Criteria

- A memory stored with `--file` is returned by recall filtered to that file,
  and absent when filtered to a different file.
- SessionEnd auto-anchor attaches the touched files to the summary memory
  (asserted by a hook e2e).
- Anchor filters compose with `--type`/`--tags`/`--min-importance`.
- Existing memories (no anchors) recall unchanged; migration is additive and
  reversible-tested against a populated prior-version fixture (W1.1 ground
  rule).
- Anchors survive export/restore.

## Verification

```bash
cargo test -p rb-store
cargo test -p rb-types
cargo test -p rb-engine
cargo test -p rb-mcp
cargo test -p rb-hooks
cargo test -p rusty-brain
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
```

Plus a migration-reproducibility test against a populated v-prior fixture.

## Risks

- Path drift across machines/renames orphans anchors. Mitigate: store a
  repo-relative normalized path + the namespace; document the limitation.
- Anchor boost degrades retrieval quality. Mitigate: v1 ships anchor as a
  filter only; boost weights deferred to measured W4.1 evidence.
- Schema churn. Mitigate: additive table, no ALTER on `memories`, serde
  defaults, no CONTRACT_VERSION bump.

## Implementation Checklist

- [ ] Add `memory_anchors` migration + type + decode.
- [ ] Add anchor params to CLI/MCP `remember`.
- [ ] Add anchor filters to recall (CLI/MCP).
- [ ] Auto-anchor SessionEnd summaries to touched files.
- [ ] Include anchors in `graph` and export/restore.
- [ ] Migration-reproducibility + recall filter e2e tests.

## Roadmap Fit

Strengthens retrieval (the `retrieval` dimension) and capture fidelity
(W3.1) without touching ranking weights, and is a clean differentiator vs
flat-recall competitors. Anchors compose with the timeline (PRD 5) and
search-parity (PRD 9) work.
