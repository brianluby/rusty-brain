# PRD: First-Run Cold-Start and Project Import (`rusty-brain init`)

## Status

Delivered 2026-07-02 (PR #51, merge `f783d1c`). `rusty-brain init` scans the
bounded project sources below, supports plan/confirmation and batch undo, and
`rusty-brain import` accepts a bounded file or stdin with dry-run. Both use the
normal remember path after client-side redaction; fixture-driven e2e coverage
proves non-empty recall, DB-bytes redaction, rerun idempotency, undo, and
dry-run no-write behavior.

Two implementation details differ from the draft without reducing the user
contract: dedup confirms exact redacted-content equality through a bounded
recall probe rather than calling `near_duplicates()` directly, and undo uses a
private per-database sidecar ledger of stored ids followed by idempotent
per-id archive calls rather than a bulk delete-by-tag operation. Import is a
bounded cold-start tool, not an ongoing sync mechanism; source-quality and
large-project activation remain unmeasured.

## Owner Area

Primary: CLI onboarding, ingestion, and the enrich seam.

Touchpoints:

- `crates/rusty-brain/src/cli.rs` (new `Init` / `Import` subcommands)
- `crates/rusty-brain/src/run.rs`
- `crates/rusty-brain/src/client.rs`
- `crates/rb-engine/src/engine.rs` (reuse enrich -> embed -> store)
- `crates/rb-engine/src/enricher.rs`
- `crates/rb-enrich/src/heuristic.rs`
- `crates/rb-enrich/src/openai_compat.rs` (when enrichment is configured)
- `crates/rb-hooks/src/transcript.rs` (bounded-reader precedent)
- `crates/rb-redact/src/lib.rs` (import MUST route through redaction)
- `docs/prds/2026-07-02-portable-export-and-backup.md` (round-trip pair)

## Problem

A fresh install recalls nothing. The first session injects an empty digest,
the agent gets no memory, and the human concludes the tool does nothing. The
W3.1 SessionEnd capture loop only starts paying back after a session is
already over, so the *first* session is structurally worthless. Every memory
product's #1 activation risk is the empty corpus, and rusty-brain has no
answer for it today.

Meanwhile the typical project already encodes its durable knowledge in files
that exist *before* rusty-brain is installed: `CLAUDE.md`, `AGENTS.md`,
`README.md`, `CHANGELOG.md`, ADR/`docs/` markdown, and the git log. None of
it is ingested.

## Goals

- A one-command first-run experience that seeds memory from existing project
  context, so the first recall is non-empty and immediately useful.
- Reuse existing primitives: batch `remember`, the enricher, namespace
  detection, and the redaction pass. No new storage schema for v1.
- Make the import safe (redacted, namespaced, reviewable) and reversible through
  the private per-database batch ledger.
- Produce a visible "imported N memories" aha-moment at install time.

## Non-Goals

- Do not change the storage schema or retrieval ranking.
- Do not invent a new enrichment model; use the configured enricher or
  heuristic fallback.
- Do not import binaries, secrets, or `.git/` internals.
- Do not auto-import on every daemon start; import is an explicit user action.
- Do not replace W3.1 capture; this seeds the cold start only.

## Functional Requirements

### INIT-1. `rusty-brain init`

A guided first-run command, safe to run idempotently. Behavior:

- Resolve the namespace via the existing resolution order.
- Detect candidate sources: `CLAUDE.md`, `AGENTS.md`, `README.md`,
  `CHANGELOG.md`, `docs/**/*.md` (bounded count/size), recent `git log`
  decision-ish commits (bounded).
- Print a plan (sources, estimated memory count) and prompt unless `--yes`.
- Route every candidate through the existing redaction pass before storing.
- Store via the engine enrich -> embed -> store path, tagging imported
  memories with `origin_source = "cli"` and a stable `import_batch` tag so
  they are reviewable; record stored ids in the private batch ledger for undo.

### INIT-2. `rusty-brain import <path|->`

A general importer (stdin or path) that ingests arbitrary text/markdown into
the current namespace. Reuses `remember --batch` framing but with:

- `--type`, `--importance`, `--tags` defaults applied uniformly.
- Per-document heuristic extraction (headings -> summary, body -> content),
  delegating to the configured enricher when available.
- A size/count cap with a documented default, overridable.
- `--dry-run` that prints the planned memories without storing.

### INIT-3. Source adapters

Pluggable, file-extension-based extractors (`md`, `txt`, plus a `git-log`
extractor). Each produces `(summary, content, type, importance-hint, tags)`.
Extraction is best-effort and never fails the whole run; a per-source error
is logged and skipped.

### INIT-4. Provenance and redaction

- Imported memories carry `origin_source = "cli"` and an `import_batch:<id>`
  tag for reviewability; the private sidecar ledger records their ids for undo.
- Every imported byte passes through `rb-redact` before store; a planted
  secret in a source file must yield zero plaintext in the DB (asserted by a
  test mirroring the W2.4 scrub drill).

### INIT-5. Idempotency and rollback

- Re-running `init` does not duplicate: a bounded recall probe confirms exact
  redacted-content equality; the importer reports
  `new`/`skipped-duplicate`/`failed` counts.
- `init --undo <batch>` reads the private per-database sidecar ledger and issues
  idempotent per-id archive calls for exactly the stored set; vector/FTS state
  follows the normal archive path.

## Acceptance Criteria

- A fresh project with a `README.md` + `CLAUDE.md` + 3 `docs/*.md`, after
  `rusty-brain init --yes`, yields a non-empty `recall` on a topic mentioned
  in those files.
- A planted fake AWS key in an imported file is absent from the raw DB bytes
  (redaction asserted at rest).
- `init` run twice produces zero new memories on the second run
  (dedup-asserted).
- `init --undo <batch>` removes exactly the imported set (vector/FTS clean).
- `import --dry-run` stores nothing and prints the plan.

## Verification

```bash
cargo test -p rusty-brain
cargo test -p rb-engine
cargo test -p rb-redact
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
```

Plus an e2e in `crates/rusty-brain/tests/` driving `init` against a fixture
project tree and asserting recall + DB-bytes redaction + dedup.

## Risks

- Noisy/low-grade imports pollute recall. Mitigate: default importance cap,
  import-batch tagging, the private ledger, and the `--undo` escape hatch.
- Enricher cost/time on large doc sets. Mitigate: bounded defaults,
  `--dry-run`, and reuse of the batched single-connection pattern.
- Secret leakage from imported files. Mitigate: mandatory redaction pass;
  redaction is best-effort, documented honestly (0600 + delete are the
  backstop, per the threat model).
- Over-extraction from git log. Mitigate: bounded commit count and a
  decision-ish filter.

## Implementation Checklist

- [x] Add `Init` and `Import` subcommands to `cli.rs`.
- [x] Implement bounded markdown/text/git-log source adapters in `import.rs`.
- [x] Wire adapters through redaction and the normal engine remember path.
- [x] Add `import_batch:<id>` tagging, a private batch ledger, and
      `init --undo` / `--list-batches`.
- [x] Implement idempotent exact redacted-content dedup through a bounded recall probe
      (deliberate deviation from the draft's `near_duplicates()` mechanism).
- [x] Add fixture-driven e2e coverage for recall, DB-bytes redaction, dedup,
      undo, and dry-run.
- [x] Document the first-run flow in the README Quickstart.

## Roadmap Fit

Fills the "activation & value-realization" dimension absent from the
Road-to-Tens plan. Pairs with `export` (round-trip) and feeds the W1.0 eval
corpus authoring (real project context, not hand-authored fixtures).
