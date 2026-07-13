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
  --operations 50 \
  --burst 32 \
  --output target/scale-benchmark.json
```

Always use a release build. Each corpus is fresh and reports:

- p50/p95/p99 and throughput for UDS recall, HTTP recall, UDS remember, and a
  concurrent hook-shaped remember burst, with errors timed separately;
- a concurrent mixed read/write phase over UDS, HTTP, and MCP plus bounded
  read-pool permit wait/saturation;
- bounded-writer-queue enqueue wait, saturation, and capacity;
- dropped change broadcasts, current process RSS, DB size, pinned WAL size,
  shutdown/checkpoint time, and retry/truncate time;
- namespace-isolation and acknowledged-write durability checks.

The default upper bound is 25,000 memories because sqlite-vec search is exact
today. Change `--corpora` to probe a larger local envelope; do not silently
replace the committed default or compare results from different corpus lists.
For fast wiring checks only, pass `--provider fixture`; such reports are marked
ineligible for a production envelope. Eligibility also requires the complete
committed 1k/10k/25k corpus matrix, adequate samples, zero measured path
errors, and the acknowledged-write/namespace/checkpoint invariants.
Only `rusty-brain-scale-v2` reports carry that eligibility contract; older v1
artifacts cannot support a production-envelope claim.

## Disk exhaustion

Disk-full and low-disk results require `--disk-exhaustion-dir` to name a
dedicated quota-limited filesystem mount root containing the marker file
`.rusty-brain-scale-disposable-volume`. The path must be the mount root, have no
pre-existing probe artifacts, and stay below `--disk-exhaustion-max-mib` free.
Otherwise the probe refuses to run. RAII cleanup removes only its known filler
and DB artifacts, including on errors.

## Rerunning after schema changes

The harness seeds through public `rb-store` APIs, closes the seeding connection,
then starts the daemon normally. This deliberately exercises all open-time
schema checks and makes the same command suitable for rerun after the task #54
schema-initialization change lands.
