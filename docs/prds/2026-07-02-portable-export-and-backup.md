# PRD: Portable Export and One-Command Backup

## Status

Draft. From the 2026-07-02 senior-PM product review. The Road-to-Tens plan
scopes backup/restore (W5b.3) to raw SQLite online-backup, gated behind team
mode (Phase 5, 6-10 weeks out). Today there is no way to back up, migrate,
inspect, or version-control memory except grepping a DB - the #1 adoption
objection ("what if I want to leave / lose it").

## Owner Area

Primary: read-only dump + restore over the existing store.

Touchpoints:

- `crates/rusty-brain/src/cli.rs` (`Export` / `Backup` / `Restore`)
- `crates/rusty-brain/src/run.rs`
- `crates/rusty-brain/src/output.rs`
- `crates/rb-store/src/store.rs`
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
- Content is post-redaction; no secret that was scrubbed is re-emitted.

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

- [ ] Add `Export`/`Backup`/`Restore` subcommands.
- [ ] Productize the `rb-eval` export helpers into the CLI.
- [ ] Wire restore through the import path + `reembed`.
- [ ] Add `--retention` pruning for backups.
- [ ] Add round-trip + redaction e2e tests.
- [ ] Document the backup/restore + migration story in the README.

## Roadmap Fit

Delivers the W5b.3 backup/restore *value* early and portably, without the
team-mode raw-SQLite machinery. Pairs with `init`/`import` as a full
round-trip and feeds version-controllable memory artifacts.
