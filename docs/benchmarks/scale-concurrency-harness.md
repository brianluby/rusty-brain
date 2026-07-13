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
- namespace-isolation and acknowledged-write durability checks;
- timed errors and timeout counts, including a residual-blocking flag when
  cancellation cannot stop daemon-side SQLite `spawn_blocking` work.

The default upper bound is 25,000 memories because sqlite-vec search is exact
today. Change `--corpora` to probe a larger local envelope; do not silently
replace the committed default or compare results from different corpus lists.
For fast wiring checks only, pass `--provider fixture`; such reports are marked
ineligible for a production envelope. V3 preregisters at least 100 actual,
error-free successes per measured path for p99; configured counts alone do not
qualify. `representative_load_eligible` also requires the complete committed
1k/10k/25k matrix and acknowledged-write/namespace/checkpoint invariants.
`production_envelope_eligible` additionally requires every mandatory fault,
including actual disk exhaustion, plus SHA-256 references for retained
interrupted-write and writer-death/reopen evidence. Only
`rusty-brain-scale-v3` reports carry this contract; older artifacts cannot
support a production-envelope claim.

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
