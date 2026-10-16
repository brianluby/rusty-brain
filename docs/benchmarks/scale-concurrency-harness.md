# Scale, concurrency, and resource harness

This harness is the reproducible evidence path for Vikunja #57. It runs the
real single-writer daemon, drives UDS, opt-in loopback HTTP, and MCP-facing
`tools/call` paths concurrently, and writes machine-readable JSON. The default
provider is the real local `all-MiniLM-L6-v2` model at 384 dimensions.
The first run may download the model weights into the local model cache, so
record the cache/model identity with retained evidence and exclude download
time from cross-run latency comparisons.

```bash
scripts/run-scale-benchmark.sh \
  --corpora 1000,10000,25000 \
  --operations 100 \
  --burst 100 \
  --operation-timeout-ms 30000 \
  --seed-batch-timeout-ms 120000 \
  --fault-evidence-dir target/scale-fault-evidence \
  --output target/scale-benchmark.json
```

Always use a release build. Each corpus is fresh and reports:

- p50/p95/p99 and throughput for UDS recall, HTTP recall, UDS remember, and a
  concurrent hook-shaped remember burst, with errors timed separately;
- a concurrent mixed read/write phase over UDS, HTTP, and the production MCP
  newline-delimited `serve_stdio` transport plus bounded read-pool permit
  wait/saturation;
- bounded-writer-queue enqueue wait, saturation, and capacity;
- dropped change broadcasts, current process RSS, DB size, pinned WAL size,
  shutdown/checkpoint time, and retry/truncate time;
- exact acknowledged-write-id durability checks: UDS, HTTP, and MCP remember
  ids from every write phase must each exist in the `scale` namespace after
  reopen, in addition to the aggregate row-count invariant;
- timed errors and timeout counts, including a residual-blocking flag when
  cancellation cannot stop daemon-side SQLite `spawn_blocking` work.

Queue, hook-burst, and mixed-path throughput timers begin before task spawning;
all mixed paths share the same phase start. Each MCP operation uses one absolute
deadline across session setup, newline-framed call, and adapter shutdown, so it
cannot consume multiple full timeout budgets.

The default upper bound is 25,000 memories because sqlite-vec search is exact
today. Change `--corpora` to probe a larger local envelope; do not silently
replace the committed default or compare results from different corpus lists.
For fast wiring checks only, pass `--provider fixture`; such reports are marked
ineligible for a production envelope. V4 retains at least 100 actual,
error-free successes per measured path for p99; configured counts alone do not
qualify. The report records `worktree_clean`, and machine eligibility requires
a clean tracked and untracked worktree so a modified harness cannot claim only
the clean `git_sha`. `representative_load_eligible` also requires the complete
committed 1k/10k/25k matrix and acknowledged-write/namespace/checkpoint invariants.
`production_envelope_eligible` additionally requires every mandatory fault,
including actual disk exhaustion, plus SHA-256 references for retained
interrupted-write, writer-death/reopen and SQLite page-limit recovery evidence.
The harness executes the four required `cargo test --release --locked --lib`
selectors itself. Eligibility uses actual subprocess exit status, a matching
individual `test ...::name ... ok` outcome, and a nonzero passing count with
zero failures/ignored tests. It writes and hashes its own captured records
(`rusty-brain-scale-fault-evidence-v2`) under `--fault-evidence-dir`; those files
are outputs, never caller-authored input authority. Fabricated records cannot
be supplied through the removed `--*-artifact` flags. Empty, ignored, zero-test
or failing executions remain incomplete. Only
`rusty-brain-scale-v4` reports with an explicit `worktree_clean: true` carry
this contract; older or incomplete artifacts cannot support a new production-
envelope claim.

## Bounded pilot replacement evidence

The original #57 benchmark/regression commits and v4 artifact were not recovered.
This replacement does not inherit their claimed results. `bounded_pilot_eligible`
requires the unchanged clean-revision, real-local 384-dimensional 1k/10k/25k
matrix, 100 error-free successes per latency path, acknowledged-ID durability,
namespace isolation and checkpoint invariants. It additionally requires every
nonphysical fault: pinned reader, interrupted writes, provider timeout,
writer death/reopen and actual SQLite `SQLITE_FULL` rollback/recovery.
Only then is `bounded_pilot_max_active_memories=25000`; otherwise it is null.

Maintainer #57 comment 182 explicitly allowed this bounded single-user trial
to defer physical filesystem exhaustion. The new field expresses that existing
decision only. `production_envelope_eligible` still requires the guarded physical
disk probe and every fault; no physical-full or broader resource gate is waived.
SQLite `max_page_count` forces the real store connection to refuse page growth
on a disposable on-disk database, not a mocked exception. The regression checks
the refused note/vector/FTS/oplog leave no partial state, autocommit is restored,
a subsequent insert succeeds after lifting the cap, acknowledged IDs and vectors
survive reopen, and `integrity_check` returns `ok`. It does not simulate a full
filesystem, failed WAL sync, process-kill crash or kernel I/O failure.

On the clean integrated revision, run the matrix; the harness executes and
retains the fault regressions itself (no prior output-capture command):

```bash
scripts/run-scale-benchmark.sh --corpora 1000,10000,25000 \
  --operations 100 --burst 100 --operation-timeout-ms 30000 \
  --fault-evidence-dir target/scale-fault-evidence \
  --output target/scale-benchmark-v4-replacement.json
```

Preserve the generated JSON fault records, report and SHA-256 values with the
dated report. A fixture provider, small smoke, dirty worktree, unexecuted fault,
failed sample, or timed-out path is not admission proof. Run on the same clean
integration revision used by #56; preserve failed distributions rather than
changing counts, timeouts or the practical bound after observing results.


## Disk exhaustion

Disk-full and low-disk results require `--disk-exhaustion-dir` to name a
dedicated quota-limited filesystem mount root containing the marker file
`.rusty-brain-scale-disposable-volume`. The path must be the mount root, have no
pre-existing probe artifacts, and stay below `--disk-exhaustion-max-mib` free.
Otherwise the probe refuses to run. RAII cleanup removes only its known filler
and DB artifacts, including on errors.

Seeding embeddings and database batches have their own configurable deadline.
A timed-out SQLite batch reports that its `spawn_blocking` work may remain
active after cancellation; do not reuse that partial corpus as evidence.

## Rerunning after schema changes

The harness seeds through public `rb-store` APIs, closes the seeding connection,
then starts the daemon normally. This deliberately exercises all open-time
schema checks and makes the same command suitable for rerun after the task #54
schema-initialization change lands.
