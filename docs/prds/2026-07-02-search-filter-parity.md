# PRD: Search and Filter Parity Across CLI and MCP

## Status

Draft. From the 2026-07-02 senior-PM product review. Discovery of what is
stored is weak: recall/list filters are inconsistent and incomplete across
the CLI and MCP surfaces. This is a hygiene/consistency PRD that makes the
existing surfaces first-class.

## Owner Area

Primary: query/filter plumbing across CLI, MCP, and proto.

Touchpoints:

- `crates/rusty-brain/src/cli.rs` (`recall`/`list` filters)
- `crates/rb-types/src/query.rs` (query filter model)
- `crates/rb-proto/src/messages.rs` (filter fields)
- `crates/rb-engine/src/engine.rs`
- `crates/rb-store/src/store.rs` (filter queries)
- `crates/rb-mcp/src/tools.rs` (`recall`/`list` params)
- `docs/prds/2026-07-02-typed-code-anchors.md` (anchor filters compose here)

## Problem

`recall` supports `--type` and `--tags`; `list` supports only
`--min-importance`. There is no date/age filter, no confidence filter, no
provenance/source filter, no anchor filter, and no way to query by
contested/archived state. The surfaces disagree, so the user falls back to
`--json | jq`, and the MCP agent cannot express precise queries.

## Goals

- One consistent filter model across CLI, MCP, and proto, expressed in
  `rb-types::query`.
- Add the missing filters: date/age range, confidence range, provenance
  source, contested/archived state, anchor (composing with PRD 4).
- Additive, backward-compatible, no CONTRACT_VERSION bump.

## Non-Goals

- Do not change ranking or fusion (RRF/linear).
- Do not build a full query language; composable flags only.
- Do not change the response shape (filters affect *which* results, not how
  they render).

## Functional Requirements

### SRH-1. Unified filter model

`rb-types::query::RecallFilter` (additive, serde-default) carries: `types`,
`tags`, `min_importance`/`max_importance`, `min_confidence`/`max_confidence`,
`since`/`until` (timestamps or seq), `sources` (`hook`/`mcp`/`cli`/`job`),
`contested` (tri-state), `state` (`active`/`archived`/`all`), `anchors`
(from PRD 4). All optional; absent = no constraint.

### SRH-2. CLI parity

`recall` and `list` accept the same filter flags (current flags are a
subset). Add: `--since`, `--until`, `--min-confidence`/`--max-confidence`,
`--source`, `--contested`, `--archived`, `--file`/`--commit`/`--symbol`
(from PRD 4).

### SRH-3. MCP parity

`recall`/`list` MCP tools expose the same filters as optional params. The
default advertised toolset stays within the token budget (filters are
optional params, not new tools); no new CONTRACT_VERSION.

### SRH-4. Store query support

The store filter queries honor every field with indexed/scoppable reads;
`contested` and `archived` reuse existing columns; no new schema for SRH
(anchor support depends on PRD 4's migration).

## Acceptance Criteria

- `recall` and `list` accept an identical set of filters in CLI and MCP.
- A `--since <seq>`/`--min-confidence`/`--contested`/`--source` filter
  returns the expected subset (asserted per filter).
- Filters compose (e.g. `--source hook --since <seq> --min-importance 7`).
- Default behavior is unchanged when no filters are given.
- Token budget for the default MCP toolset stays under the W3.3 limit.

## Verification

```bash
cargo test -p rb-types
cargo test -p rb-store
cargo test -p rb-engine
cargo test -p rb-mcp
cargo test -p rusty-brain
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
```

Plus a parametrized e2e asserting each filter and their composition.

## Risks

- Filter combinatorics explode query plans. Mitigate: scoped/indexed reads,
  bounded result sets, and the existing `LIMIT`.
- MCP tool description bloat breaches the token budget. Mitigate: optional
  params only, no new tools; assert with the W3.3 token-accounting test.
- Anchor filter depends on PRD 4. Mitigate: ship SRH-1..3 first; anchor
  filter lands with PRD 4.

## Implementation Checklist

- [ ] Define the unified `RecallFilter` model.
- [ ] Extend store filter queries for every field.
- [ ] Add CLI flags to `recall`/`list` (parity).
- [ ] Add MCP params to `recall`/`list` (parity).
- [ ] Assert token budget with the W3.3 accounting test.
- [ ] Parametrized filter + composition e2e.

## Roadmap Fit

Hygiene that raises the `claudecode` and `retrieval` dimensions' usability
without changing ranking, and is a prerequisite for anchor filters (PRD 4)
and useful stats/review surfaces (PRDs 2, 6).
