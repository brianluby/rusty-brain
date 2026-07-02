# PRD: User-Facing Retention and Forgetting Policy

## Status

Draft. From the 2026-07-02 senior-PM product review. Decay and
importance-recalibration exist internally (W1.9, the jobs subsystem) but the
user has no knob or visibility into *what gets forgotten and when*. Today you
cannot choose to forget.

## Owner Area

Primary: retention config, a bounded forget job, and surfacing it to the user.

Touchpoints:

- `crates/rb-config/src/file.rs` (retention policy config)
- `crates/rb-daemon/src/jobs/config.rs`
- `crates/rb-daemon/src/jobs/importance.rs`
- `crates/rb-daemon/src/jobs/link_decay.rs`
- `crates/rb-daemon/src/store_handle.rs`
- `crates/rb-store/src/store.rs` (archive/purge primitives)
- `crates/rusty-brain/src/cli.rs` (`forget` / retention commands)
- `docs/prds/2026-07-02-doctor-and-stats-observability.md` (forgetting stats)

## Problem

Memory accumulates forever unless the user manually archives. The
importance-recalibration job (W1.9) ships disabled, and there is no
user-facing retention policy, no "forget memories older than X with
importance < Y," and no view into what would be forgotten. The corpus drifts
toward noise, recall quality degrades, and the user has no control.

## Goals

- A declarative retention policy the user configures and can preview.
- A bounded, conservative `forget` job that archives (not hard-deletes) by
  default, respecting author intent (W1.9's clamp rule).
- Visibility: `stats`/`doctor` show what is eligible and what was forgotten.

## Non-Goals

- Do not hard-delete by default (archive is reversible; hard purge stays the
  W5b.3 admin op, never automatic).
- Do not forget memories the author marked high-importance (W1.9 invariant:
  importance-10 never falls below 8 from signals alone).
- Do not change ranking.
- Do not build team-mode retention/GDPR semantics (Phase 5).

## Functional Requirements

### RET-1. Retention policy config

A `[retention]` block in the user config (precedence per existing rules):

- `max_age_days` (forget older than N days AND below an importance floor).
- `importance_floor` (never forget at/above this; default 6).
- `archive_after_days` (soft archive before forget).
- `enabled` (default `false`; forgetting is opt-in).
- A `protected_tags` list (e.g. `architecture_decision`) never forgotten.

### RET-2. `rusty-brain forget`

- `forget --dry-run` lists candidates with reasons (age, importance,
  last-recalled, contested flag - contested memories are never auto-forgotten).
- `forget --apply` archives eligible memories (soft delete; reversible).
- `forget --hard` performs a hard purge cascading to vectors/FTS/oplog (admin
  op, requires the peer-cred admin gate like `scrub`).
- Bounded per pass; re-runnable.

### RET-3. Job integration

- The retention rule runs as an evolution job (reuse the jobs subsystem),
  disabled by default, enabled by config.
- It composes with importance-recalibration (W1.9) but never overrides the
  author-intent clamp: a memory's *effective* importance gates eligibility,
  and the floor + protected tags are absolute.

### RET-4. Visibility

- `stats` reports eligible-forgetting count and last-forget run; `doctor`
  warns on a policy that would forget protected/high-importance items.
- Every forget action records provenance + an oplog entry (so history PRD 5
  explains why a memory disappeared).

## Acceptance Criteria

- `forget --dry-run` lists exactly the memories matching the policy and
  respects the floor + protected tags + contested exclusion.
- `forget --apply` archives (soft) the listed set; `forget --hard` purges
  them from DB/FTS/vectors/oplog (asserted by raw-bytes checks).
- An importance-10 memory is never eligible regardless of age.
- A contested memory is never auto-forgotten.
- Forgetting is off by default; enabling requires explicit config.

## Verification

```bash
cargo test -p rb-daemon
cargo test -p rb-store
cargo test -p rb-config
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
```

Plus a job e2e seeding aged + high-importance + contested memories and
asserting the dry-run/apply/hard eligibility.

## Risks

- Forgetting something valuable. Mitigate: archive-by-default, floor +
  protected tags, contested exclusion, dry-run, off-by-default.
- Silent corpus shrinkage. Mitigate: oplog + stats visibility; never
  auto-hard-delete.
- Policy misconfiguration. Mitigate: `doctor` warns on dangerous policies.

## Implementation Checklist

- [ ] Add `[retention]` config block.
- [ ] Implement `forget` (dry-run/apply/hard) over archive/purge primitives.
- [ ] Wire the retention job into the jobs subsystem (disabled by default).
- [ ] Enforce floor + protected tags + contested exclusion + W1.9 clamp.
- [ ] Surface eligibility in `stats`/`doctor`; record oplog entries.
- [ ] E2e for eligibility and archive/purge.

## Roadmap Fit

Productizes the W1.9 importance-recalibration and W5b.3 data-lifecycle
intent for the single user, raising corpus quality (retrieval) and user
control (teamfit) ahead of Phase 5.
