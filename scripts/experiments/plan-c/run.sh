#!/usr/bin/env bash
# Plan C (kanban #83): budget-capped Claude implements, Codex reviews the diff
# adversarially, tests plus a hidden oracle decide. One experiment worktree.
# Usage: run.sh N "gate cargo args" HIDDEN_CRATE "hidden cargo args"
#   PLANC_WORKTREE  experiment worktree (default ~/repos/rusty-brain/.worktrees/plan-c),
#                   created with: git worktree add --detach <path> origin/main
#   PLANC_OUT       run output dir (default ./runs next to this script)
#   BUDGET          Claude hard cap in USD per run (default 5)
# Run 1: run.sh 1 "-p rb-store" rb-store ""
# Run 2: run.sh 2 "-p rb-embed -p rusty-brain --features local" rb-embed "--features local"
# Run 3: run.sh 3 "-p rb-daemon -p rusty-brain" rb-daemon ""
set -uo pipefail
N=$1; GATE=$2; HCRATE=$3; HARGS=$4
P=$(cd "$(dirname "$0")" && pwd)
WT=${PLANC_WORKTREE:-$HOME/repos/rusty-brain/.worktrees/plan-c}
OUT=${PLANC_OUT:-$P/runs}/run$N; mkdir -p "$OUT" && OUT=$(cd "$OUT" && pwd) || exit 2
BUDGET=${BUDGET:-5}
log(){ printf '%s %s\n' "$(date -u +%H:%M:%S)" "$*" | tee -a "$OUT/log.txt"; }

cd "$WT" || exit 2
# Refuse anything but the root of a linked worktree: a stale or mistyped path
# would otherwise let `reset --hard` discard work in the main checkout.
top=$(git rev-parse --show-toplevel 2>/dev/null) || { echo "not a git worktree: $WT" >&2; exit 2; }
[ "$top" = "$(pwd -P)" ] || { echo "$WT is not a worktree root (resolves to $top)" >&2; exit 2; }
[ "$(git rev-parse --absolute-git-dir)" != "$(cd "$(git rev-parse --git-common-dir)" && pwd -P)" ] \
  || { echo "$WT is the main checkout, not a linked worktree" >&2; exit 2; }
git reset -q --hard origin/main && git clean -qfd || exit 2
FAILED=0
# The worktree is reset at the start of every run, so an interrupted run needs
# no repair; only the reviewer's scratch dir must not leak.
EMPTY=$(mktemp -d)
trap 'rm -rf "$EMPTY"' EXIT
trap 'log "interrupted"; exit 130' INT TERM
log "base $(git rev-parse --short HEAD)"

# 1. Implement (hard dollar cap; hooks and MCP off so nothing leaks into user stores).
log "implement: claude -p --max-budget-usd $BUDGET"
cat "$P/tasks/preamble.md" "$P/tasks/run$N.md" > "$OUT/prompt.md"
start=$(date +%s)
claude -p "$(cat "$OUT/prompt.md")" \
  --max-budget-usd "$BUDGET" --output-format json \
  --permission-mode acceptEdits \
  --allowedTools "Read Edit Write Glob Grep Bash(cargo:*) Bash(git diff:*) Bash(git status:*) Bash(ls:*)" \
  --settings '{"disableAllHooks":true}' --strict-mcp-config --no-session-persistence \
  > "$OUT/claude.json" 2> "$OUT/claude.stderr"
rc=$?; log "implement exit $rc in $(( $(date +%s) - start ))s"
# A failed or over-budget implementation is a failed run, never retried.
[ $rc -eq 0 ] || FAILED=1
# claude -p can exit 0 yet report an error or the budget cap in its JSON.
summary=$(python3 -I -c 'import json,sys
d=json.load(open(sys.argv[1]))
print("claude cost_usd", d.get("total_cost_usd"), "turns", d.get("num_turns"), "subtype", d.get("subtype"), "is_error", d.get("is_error"))
sys.exit(1 if d.get("is_error") or d.get("subtype") != "success" else 0)' "$OUT/claude.json" 2>&1)
rc=$?; log "$summary"; [ $rc -eq 0 ] || FAILED=1

# 2. Capture the diff (untracked files included).
git add -N . && git diff > "$OUT/diff.patch"
log "diff $(wc -l < "$OUT/diff.patch") lines, files: $(git diff --name-only | tr '\n' ' ')"

# 3. Adversarial review: one call, diff-only, read-only, 10-minute timeout, no retry.
{ cat "$P/tasks/review.md"; echo; echo "## Task"; cat "$P/tasks/run$N.md"; echo; echo "## Diff"; echo '```diff'; cat "$OUT/diff.patch"; echo '```'; } > "$OUT/review-prompt.md"
start=$(date +%s)
timeout 600 codex exec --json -s read-only --skip-git-repo-check --ephemeral -C "$EMPTY" \
  -o "$OUT/review.json" - < "$OUT/review-prompt.md" > "$OUT/codex.jsonl" 2> "$OUT/codex.stderr"
rc=$?; log "review exit $rc in $(( $(date +%s) - start ))s$([ $rc -eq 124 ] && echo ' (timed out; no retry)')"

# 4. Gate: the implementer's own tests and clippy, then the hidden oracle.
log "gate: cargo nextest run $GATE"
cargo nextest run $GATE > "$OUT/gate-test.txt" 2>&1; rc=$?; log "gate test exit $rc"; [ $rc -eq 0 ] || FAILED=1
cargo clippy $GATE --all-targets -- -D warnings > "$OUT/gate-clippy.txt" 2>&1; rc=$?; log "gate clippy exit $rc"; [ $rc -eq 0 ] || FAILED=1
cp "$P/hidden/"run${N}_*.rs "crates/$HCRATE/tests/"
if [ "$HCRATE" = rb-embed ] && ! grep -q '^tempfile' crates/rb-embed/Cargo.toml; then
  printf '\n[dev-dependencies.tempfile]\nworkspace = true\n' >> crates/rb-embed/Cargo.toml
fi
# Run 2's hidden tests REQUIRE PLANC_PIN_JSON_DIR (the four pinned JSON files)
# and PLANC_REAL_CACHE (a populated model cache); they fail when unset.
cargo test -p "$HCRATE" $HARGS --test "$(basename "$P"/hidden/run${N}_*.rs .rs)" > "$OUT/hidden.txt" 2>&1
rc=$?; log "hidden oracle exit $rc"; [ $rc -eq 0 ] || FAILED=1
grep -E '^test |test result' "$OUT/hidden.txt" | tee -a "$OUT/log.txt"
log "done (gates $([ $FAILED -eq 0 ] && echo passed || echo FAILED))"
exit $FAILED
