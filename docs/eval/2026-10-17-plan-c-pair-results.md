# Implementer/reviewer pair (Plan C) — results (Vikunja #83)

- **Date:** 2026-10-17. Base `main` at `593be2a`.
- **Question:** in one git worktree, does a second-vendor CLI reviewing a
  budget-capped Claude's diff find real defects that the test suite does not?
- **Wrapper:** [`scripts/experiments/plan-c/run.sh`](../../scripts/experiments/plan-c/run.sh),
  with the task specs, reviewer prompt and hidden tests beside it. Per-run
  diffs, reviews, logs and cost summaries are in
  [`artifacts/2026-10-17-plan-c/`](artifacts/2026-10-17-plan-c/).
- **Budget:** owner-declared cap of $20 for the three runs.

## Decision: do not graduate (2 of 3)

The preregistered rule in #83 graduates the pair only if the reviewer finds
genuine defects in all three runs and Claude stays in budget. Claude stayed far
inside budget, but the reviewer found a genuine defect in two runs, not three.
So the wrapper stays an experiment script and is not promoted to a skill.

The result is still informative. In both successful runs, the reviewer found a
defect that the implementer's own tests, Clippy and an independent hidden test
all missed.

## Setup

Each run resets the experiment worktree to `origin/main`, then:

1. **Implement.** `claude -p --max-budget-usd 5` (Claude Opus 5.5) with
   `acceptEdits`, an allow-list of read/edit tools plus `cargo`,
   `git diff/status` and `ls`, hooks disabled (`disableAllHooks`) and no MCP
   servers (`--strict-mcp-config`), so nothing reaches the user's memory store.
2. **Review.** One `codex exec --json` call (gpt-6.1-sol, high reasoning) in
   an empty directory with a read-only sandbox. Its only input is the task
   text and the diff. A 10-minute timeout applies, with no retry.
3. **Gate.** `cargo nextest run` and strict Clippy on the touched crates, then
   a **hidden acceptance test**, written and checked against `main` before the
   run and never shown to the implementer.

The ticket names an "item-1 fixture" from earlier research that is not
available on this machine. The hidden tests replace it as the independent
oracle for judging reviewer findings.

| Run | Task | Hidden oracle |
|---|---|---|
| 1 | Batch link/anchor loading (the `TODO(P1)` N+1 in `rb-store`), with `get_many` accepting any number of ids | 2 tests. On `main` the 40,000-id case fails on SQLite's bind limit. |
| 2 | Offline `cached_pin_status` for the #100 model pin, plus a `doctor` check | 5 tests (missing, corrupt-wins, hf-hub symlinks, real cache) |
| 3 | Harden `prepare_socket_dir`: refuse symlinks, other owners and other modes; atomic private creation | 7 tests, including 20 rounds of 16 concurrent first starts |

## Results

| Run | Claude cost | Time | Own tests + Clippy | Hidden oracle | Reviewer verdict | Genuine? |
|---|---:|---:|:---:|:---:|---|:---:|
| 1 | $1.40 | 183 s | pass | 2/2 | Batched relations are read outside the row query's read snapshot | **yes** |
| 2 | $0.99 | 239 s | pass | 5/5 | Unix-only `symlink` in a test breaks Windows builds | no |
| 3 | $1.06 | 222 s | pass | 7/7 | Staging dir can be swapped for a symlink before chmod/rename; no post-publish check | **yes** (narrow) |

**Spend:**
- **Claude:** $3.45 in total, reported by `--max-budget-usd` accounting. No
  run came near its $5 cap.
- **Codex:** about 93k input tokens (40k cached) and 3.3k output tokens over
  three reviews. Codex has no dollar meter; at typical frontier API rates this
  is about $1.
- **Total:** well under the $20 cap.

### Adjudication

- **Run 1 (genuine, medium).** The store runs SQLite in WAL mode, with reader
  connections separate from the single writer. Before the change, each row's
  links and anchors were queried while the row cursor was still open, so a
  note came from one read snapshot. The batched version finishes the row query
  before loading relations, so a commit in between can return a note mixing old
  columns with new relations. The fix is an explicit read transaction around
  the batch. The hidden test is single-threaded and could not catch this. The
  finding follows from SQLite's snapshot semantics and was not reproduced as a
  failing test.
- **Run 2 (false positive).** CI is Linux and macOS only, and the daemon itself
  depends on Unix sockets, so a Unix-only test helper is not a defect here.
  The reviewer also missed a real one: with no cache directory configured,
  `doctor` and the daemon each resolve the relative `.fastembed_cache` against
  their own working directory, so `doctor` can inspect a different cache.
  That flaw predates this run.
- **Run 3 (genuine, low practical severity).** The implementer built each
  missing directory as a staging dir, chmodded it by path, renamed it with a
  no-replace syscall, and returned success without re-checking what was
  published. A user who can rename entries in the parent could swap in a
  symlink between steps. That requires a hostile, non-sticky, writable
  parent. The reviewer did not flag the larger issue: the implementer added
  `unsafe` libc rename calls that are unnecessary, since `mkdir` with mode
  0700 is already atomic, which is how `main` does it after #104.

## Observations

- Claude consistently solved the stated task: every hidden oracle passed on the
  first attempt, at about $1 to $1.40 per small change. The defects were in
  what the spec did not say (snapshot consistency, hostile filesystems), not
  in what it did.
- The reviewer is cheap (17 to 41 s per diff) and its true positives were
  concurrency and TOCTOU issues, which tests rarely catch. Its false positive
  was a portability claim it could not check from the diff alone.
  Diff-only input keeps cost bounded but removes repository context such as CI
  targets.
- Neither tool overspent, and no change left the experiment worktree.

## If revisited

Run it as an optional pre-PR step for concurrency- or security-sensitive
changes, not as a gate, and give the reviewer the CI platform list. A new
decision would need its own preregistered runs.
