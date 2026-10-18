# Coverage baseline and gate decision (Vikunja #82)

- **Date:** 2026-10-18, at `main` `cfc9b23`.
- **Tool:** `cargo-llvm-cov` 0.8.7, rustc 1.99.1 (Homebrew), LLVM 23.1.3.
- **Run:** the whole workspace with default features, 76 test binaries. All
  tests pass under instrumentation in 132 s on an Apple M4 Pro.

## Decision: no CI coverage gate

Coverage stays an on-demand local measurement, with this page as the reference
point. There is no CI job, report-only or blocking.

- **The baseline is already high.** Workspace line coverage is 93.2% and no
  crate is below 84%. A threshold would either be met trivially or need a
  ratchet nobody owns.
- **It costs every PR.** The instrumented build and test run adds a full
  separate build plus about 2 minutes locally, and more on CI runners.
- **The ticket's purpose is met without a job.** The aim is a reference before
  large refactors (facade and sibling splits, retrieval changes) and a check
  that a new capability is exercised. Both need a before/after diff at the
  moment of the change, not a number on every PR. A report-only job nobody
  reads adds cost without changing decisions.

**Revisit** if coverage starts to drift on a crate that is being refactored,
or if a hard requirement appears, such as a release checklist.

## Baseline (lines, regions, functions)

| Crate | Files | Lines | Line % | Region % | Function % |
|---|---:|---:|---:|---:|---:|
| `rusty-brain` | 13 | 7,673 | 84.9 | 84.2 | 84.7 |
| `rb-eval` | 26 | 5,700 | 85.2 | 84.3 | 81.8 |
| `rb-enrich` | 3 | 644 | 89.0 | 90.9 | 78.9 |
| `rb-daemon` | 13 | 8,075 | 91.4 | 92.2 | 88.6 |
| `rb-embed` | 3 | 668 | 92.4 | 93.0 | 86.4 |
| `rb-install` | 12 | 2,266 | 92.6 | 92.8 | 79.8 |
| `rb-hooks` | 9 | 4,321 | 95.5 | 96.0 | 96.6 |
| `rb-config` | 4 | 1,852 | 95.6 | 95.5 | 95.6 |
| `rb-mcp` | 6 | 3,445 | 95.8 | 94.8 | 94.2 |
| `rb-agents` | 13 | 1,863 | 96.3 | 96.9 | 98.0 |
| `rb-proto` | 5 | 3,365 | 96.6 | 94.1 | 97.1 |
| `rb-store` | 15 | 12,473 | 96.6 | 94.6 | 85.8 |
| `rb-engine` | 8 | 4,313 | 96.8 | 96.5 | 89.7 |
| `rb-tokens` | 2 | 267 | 97.0 | 97.3 | 100.0 |
| `rb-contract-guard` | 6 | 848 | 97.2 | 95.4 | 91.8 |
| `rb-search` | 4 | 1,001 | 99.3 | 99.6 | 100.0 |
| `rb-types` | 22 | 3,387 | 99.4 | 98.8 | 100.0 |
| `rb-redact` | 1 | 156 | 99.4 | 99.3 | 100.0 |
| **workspace** | 165 | 62,317 | 93.2 | 92.7 | 89.2 |

The table is sorted by line coverage. The lowest crates are the CLI
(`rusty-brain`, 84.9%) and the eval harness (`rb-eval`, 85.2%); both have
large manual or `#[ignore]` paths, such as real-model recording, external
benchmarks and interactive commands.

## Caveats

- **Default features only.** Code behind the `local` feature (the fastembed
  provider in `rb-embed` and the local paths in `rusty-brain`) is not compiled,
  so it is absent rather than counted as uncovered.
- **Inline tests count.** `#[cfg(test)]` modules inside `src/` files count as
  covered lines, which inflates line percentages somewhat. Compare crates over
  time, not against an external benchmark.
- **`#[ignore]` tests don't run.** That includes the manual model and
  benchmark tests, so their code counts as uncovered.

## Reproduce

```bash
cargo llvm-cov clean --workspace   # --no-report skips the automatic cleanup
cargo llvm-cov --workspace --no-report --no-fail-fast
cargo llvm-cov report --summary-only                  # per-file text table and TOTAL
cargo llvm-cov report --json --summary-only --output-path coverage.json
```

The per-crate table sums each file's covered and total counts from
`coverage.json` by `crates/<name>/`.

`cargo-llvm-cov` needs `llvm-cov` and `llvm-profdata` built from the same LLVM
major version as the active `rustc`. On this machine Homebrew's Rust comes
first on `PATH`, ahead of rustup, so the rustup `llvm-tools` component is not
found. Point the tool at Homebrew's LLVM instead (check the version with
`rustc -vV`):

```bash
export LLVM_COV=/opt/homebrew/opt/llvm/bin/llvm-cov
export LLVM_PROFDATA=/opt/homebrew/opt/llvm/bin/llvm-profdata
```

With a rustup toolchain, `rustup component add llvm-tools` is enough.
