# Contract-drift guard (W5a.4, operationalized)

Vikunja #492. The Road-to-Tens W5a.4 protocol-evolution policy — additive
serde-default fields never bump `CONTRACT_VERSION`; breaking changes bump it —
used to live in prose only, enforced by reviewer diligence. The
`contract-drift` CI job now enforces it mechanically.

## What is guarded

- **Wire surface**: every shape-defining item in
  `crates/rb-proto/src/messages.rs` (`CONTRACT_VERSION`, `Handshake`,
  `HandshakeAck`, `ClientIdentity`, `Request`, `Response`,
  `RecallChannelTotals`) plus the rb-types payload shapes those frames embed
  (`MemoryNote`, `SearchResult`, `MemoryStats`, `MemoryChanged`, `Namespace`,
  enums, ...). rb-types is included deliberately: the v2 bump
  (`MemoryNote.contested`) happened in rb-types, so guarding rb-proto alone
  would have missed the only historical breaking change. The whole
  `crates/rb-types/src` directory is scanned (only `error.rs` is excluded), so
  a NEW payload file is guarded by default — the PR #56 lesson, where
  `MemoryStats` landed in a brand-new `stats.rs` that an explicit file list
  would have silently missed.
- **Persistence surface**: every `crates/rb-store/migrations/NNN_*.sql` file
  (name + sha256).

`crates/rb-contract-guard` parses the sources with `syn` and digests each
item's normalized tokens: doc comments, regular comments, and formatting are
stripped, `#[cfg(test)]` items are skipped, and serde attributes are kept
(they ARE the wire shape). Structs, enums, `type` aliases, explicit serde
impls, and `CONTRACT_VERSION` are digested; inline modules
(`pub mod v2 { ... }`) are recursed into with module-qualified keys. Only the
exact `#[cfg(test)]` predicate is treated as test-only — `cfg(not(test))`,
`cfg(feature = "...")`, `cfg(all(test, ...))` etc. stay in the digest
(conservative inclusion: the guard must never silently drop a real wire
item). Comment- or formatting-only edits never trip the guard. The digests
live in the checked-in `contract-snapshot.toml`.

## What happens on a PR

- **No contract change**: `check` matches the snapshot, the job is green.
- **Any shape/migration/version change without a snapshot update**: the job
  fails, naming exactly which items drifted.
- **Recording the decision** (this is the "explicit marker"):
  - additive, serde-default-compatible change — old frames still decode, no
    bump (the `Handshake.identity` / `Pong.recall_channels` precedent):

        cargo run -p rb-contract-guard -- update --intent additive --note "<what changed>"

  - breaking change — bump `CONTRACT_VERSION` in
    `crates/rb-proto/src/messages.rs` first, then:

        cargo run -p rb-contract-guard -- update --intent breaking --note "<what changed>"

  Commit the regenerated `contract-snapshot.toml`. The tool enforces the
  version rule for the declared intent (`additive` refuses a bumped version;
  `breaking` demands one), and the note lands in the snapshot's append-only
  `[[log]]`, so the decision is visible in the PR diff for reviewers.

The guard is also bound into `cargo test --workspace` (the
`rb-contract-guard` `real_repo` test), so drift fails locally before CI, and
`scripts/ci-local.sh` runs the same `check` step.

## Version-skew fixture

`rb-daemon`'s `n_minus_one_handshake_is_rejected_gracefully` e2e test pins the
N-1 handshake behavior: with no breaking bump in flight there is no N/N-1
dual-support window, so an N-1 client gets a graceful
`HandshakeAck { ok: false }` naming both versions, then a closed connection.
When a future bump opens a dual-support window (hub supports N and N-1 for one
release per W5a.4), that test is the seam to flip.

## Non-goals

- No general-purpose schema/OpenSpec adoption; the guard is scoped to
  rusty-brain's `CONTRACT_VERSION` + migration surfaces.
- No semantic additive-vs-breaking classification: the guard forces a
  deliberate, reviewable decision; whether a change is truly serde-default
  compatible remains a review judgment (the drift report and log entry make it
  impossible to slip one through silently).
- Framing (`rb-proto/src/frame.rs`, `codec.rs`) is not digested: its wire
  behavior lives in function bodies, which would make every refactor a false
  positive; it is covered by the round-trip tests instead.
- Non-doc attribute edits that cannot change the wire shape — e.g. rewording a
  `#[deprecated(note = "...")]` — DO trip the guard: attributes are digested
  whole because serde attrs must be, and special-casing "harmless" attrs is
  not worth the complexity. The resolution is the same one-command additive
  marker.
