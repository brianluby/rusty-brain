#!/usr/bin/env bash
# Run the same gates as .github/workflows/ci.yml locally, in the same order,
# with the same RUSTFLAGS. Mirrors each job 1:1 so a green run here predicts a
# green CI. Pass --fast to skip the heavy full-workspace + local-feature builds.
#
#   scripts/ci-local.sh         # everything CI runs
#   scripts/ci-local.sh --fast  # skip --all-features / --features local builds
set -uo pipefail

export RUSTFLAGS="${RUSTFLAGS:--D warnings}"   # CI sets this globally (ci.yml env)
FAST=0; [ "${1:-}" = "--fast" ] && FAST=1
ROOT="$(cd "$(dirname "$0")/.." && pwd)"; cd "$ROOT" || exit 1

rc=0
step() { # label cmd...
  local label="$1"; shift
  printf '\n=== %s ===\n%s\n' "$label" "$*"
  if "$@"; then printf 'PASS: %s\n' "$label"; else printf 'FAIL: %s\n' "$label"; rc=1; fi
}

# --- rustfmt job ---
step "rustfmt"            cargo fmt --all --check

# --- agent crates job ---
step "agent: build"      cargo build -p rb-agents -p rb-hooks -p rb-install
step "agent: clippy"     cargo clippy -p rb-agents -p rb-hooks -p rb-install --all-targets -- -D warnings
step "agent: test"       cargo test -p rb-agents -p rb-hooks -p rb-install

# --- contract drift guard job (W5a.4) ---
step "contract-drift"    cargo run -p rb-contract-guard -- check

# --- cargo-deny / cargo-audit jobs ---
step "cargo-deny"        cargo deny check
step "cargo-audit"       cargo audit

# --- bash 3.2 portability (macOS CI runs the recorder self-test under the
#     system bash; locally `bash` is 5.x, so also exercise /bin/bash 3.2). ---
if [ -x /bin/bash ]; then
  step "recorder self-test (/bin/bash 3.2)" /bin/bash scripts/record-agent-fixtures.sh --self-test
  step "recorder dry-run (/bin/bash 3.2)"   /bin/bash scripts/record-agent-fixtures.sh --dry-run --agent all
fi
step "memory-scorecard self-test" bash scripts/memory-scorecard.sh --self-test

if [ "$FAST" -eq 0 ]; then
  # --- clippy + test (workspace) job: the Linux + macOS matrix ---
  step "workspace: clippy" cargo clippy --workspace --all-targets --all-features -- -D warnings
  step "workspace: test"   cargo test --workspace
  # --- build + clippy (local feature) job ---
  step "local-feature: build"  cargo build -p rusty-brain --features local
  step "local-feature: clippy" cargo clippy -p rusty-brain --features local --all-targets -- -D warnings
  step "local-feature: test"   cargo test -p rb-embed --features local
fi

printf '\n==== %s ====\n' "$([ $rc -eq 0 ] && echo 'ALL LOCAL CI GATES PASSED' || echo 'SOME GATES FAILED')"
exit $rc
