# PRD: Portable Export and One-Command Backup

## Status

Delivered 2026-07-02 (PR #52, merge `24ac007`). The CLI ships deterministic
markdown/JSON/CSV export, timestamped portable backup with listing/retention,
and bounded JSON restore through the redacting, deduplicating normal remember
path (therefore vectors are recomputed under the current model). Unit and
binary e2e tests cover deterministic id order, retention pruning,
export/restore idempotency, recall after restore, and planted-secret absence.

Delivered v1 is narrower than the draft in several reviewable ways: export is
the current namespace to stdout (no `--all`, output-file flag, `--since`, or
archived-row mode); filters are type/tags/min-importance; CSV is metadata-only;
the read is bounded at 100,000 active memories and warns if it may truncate;
restore accepts JSON only; and no cron/launchd schedule is installed. The e2e
round trip restores into the same live namespace and proves dedup/continued
recall, not rank equivalence on a fresh database or semantic parity across two
production embedding models. Export serializes stored rows; it does not run a
new redaction pass. The planted-secret test writes through the redacting import
path, so operators with legacy or manually stored plaintext must run `scrub`
before exporting. Raw SQLite disaster recovery remains future team-mode work.

## Owner Area

Primary: read-only dump + restore over the existing store.

Touchpoints:

- `crates/rusty-brain/src/cli.rs` (`Export` / `Backup` / `Restore`)
- `crates/rusty-brain/src/run.rs`
- `crates/rusty-brain/src/output.rs`
- `crates/rusty-brain/src/export.rs`
- `crates/rb-proto/src/messages.rs`
- `crates/rb-eval/` (existing export tooling to productize)
- `docs/prds/2026-07-02-init-and-project-import.md` (round-trip pair)

## Problem

Memory is locked into rusty-brain's own SQLite store with no escape hatch:
no human-readable dump, no migration path, no `git`-diffable artifact, no
auto-backup. A memory product with no exit feels like a trap, and W5b.3's
raw-SQLite backup is weeks away and not portable across embedding-model
swaps.

## Goals

- A portable, human-readable, diffable export (memory + metadata + links).
- A one-command backup with a default schedule, and a restore that round-trips.
- Reuse the export logic the `rb-eval`/scorecard tooling already implements.
- Restore must be embedding-model-aware (re-embed on import, reusing
  `reembed`) so an export survives a model swap.

## Non-Goals

- Do not replace W5b.3 team-mode raw-SQLite online-backup (that stays for
  byte-exact disaster recovery).
- Do not build a hub sync protocol (that is Phase 5a).
- Do not export vectors in v1 (they are re-derivable from content; export is
  text + metadata only).
- Do not export redacted/scrubbed-away secrets (export is post-redaction).

## Functional Requirements

### EXP-1. `rusty-brain export`

Read-only dump of the current namespace (or `--all`) to stdout or a file:

- Formats: `markdown` (human/diffable), `json` (full-fidelity), `csv`
  (tabular). Reuse the `rb-eval` export helpers.
- Fields: id, type, importance, confidence, tags, summary, content, context,
  created/updated, links, `contested`, provenance.
- Filters: `--type`, `--tags`, `--min-importance`, `--since <seq>` (oplog),
  `--active` (exclude archived).
- Content reflects the stored post-scrub state; export itself does not mutate
  or newly redact rows. A secret removed by `scrub` is not re-emitted.

### EXP-2. `rusty-brain backup`

- One-command snapshot to `~/.local/share/rusty-brain/backups/<ts>.<ext>`
  (default `json`).
- Optional auto-schedule hook (documented cron/launchd example; not a daemon
  feature in v1).
- `--retention N` to prune old backups (keep last N).
- Idempotent and safe to run while the daemon is live (read pool / online
  snapshot; no writer contention).

### EXP-3. `rusty-brain restore`

- Ingest an export file into the current namespace via the import path
  (`init`/`import` PRD), reusing near-dup suppression so restore is
  idempotent.
- Vectors are re-embedded from content on restore (reusing `reembed`), so an
  export made under model A restores cleanly under model B.
- `--dry-run` to preview; `--namespace <ns>` to restore into a different
  namespace.

### EXP-4. Round-trip and integrity

- Export then restore reproduces the live, active set (asserted by a
  round-trip test comparing recall results before/after).
- Export is deterministic for a given corpus (sorted by id) so it is
  `git diff`-friendly and reviewable.

## Acceptance Criteria

- `export --format markdown` produces a diffable file a human can read and
  `git diff`.
- `backup` writes a timestamped file under the data dir and prunes to
  `--retention N`.
- `restore` of an export reproduces the original recall results (within a
  tolerance), re-embedding under the current model.
- A planted-then-scrubbed secret is absent from the export (post-redaction).
- Export/restore are safe to run with the daemon live (no writer contention).

## Verification

```bash
cargo test -p rusty-brain
cargo test -p rb-store
cargo test -p rb-redact
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
```

Plus an e2e asserting export -> restore -> recall parity and DB-bytes
redaction in the export.

## Risks

- Restore under a different model produces different vectors -> different
  ranking. Mitigate: re-embed on restore and document the caveat; the
  fail-closed model-identity contract still governs the live DB.
- Large-corpus export size. Mitigate: streaming output, format choice, and
  `--active` default.
- Leaking secrets via export. Mitigate: export is post-redaction only;
  document residual risk in the threat model.

## Implementation Checklist

- [x] Add `Export`/`Backup`/`Restore` subcommands.
- [x] Add dedicated deterministic export formatting in the CLI crate.
- [x] Wire JSON restore through the import/remember path so vectors are
      recomputed under the current provider.
- [x] Add backup listing and `--retention` pruning.
- [x] Add idempotent export/restore, recall, ordering, and redaction tests
      (with the fresh-DB/model-swap limitation recorded in Status).
- [x] Document backup/restore commands and the portable migration story in the
      README Quickstart.

## Roadmap Fit

Delivers the W5b.3 backup/restore *value* early and portably, without the
team-mode raw-SQLite machinery. Pairs with `init`/`import` as a full
round-trip and feeds version-controllable memory artifacts.
