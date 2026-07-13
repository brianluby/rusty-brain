# Scale, concurrency, and resource harness

This harness is the reproducible evidence path for Vikunja #57. It runs the
real single-writer daemon with deterministic embeddings, drives both UDS and
opt-in loopback HTTP, and writes machine-readable JSON. It does not call a
paid provider or require network access.

```bash
scripts/run-scale-benchmark.sh \
  --corpora 1000,10000,25000 \
  --operations 50 \
  --burst 32 \
  --output target/scale-benchmark.json
```

Always use a release build. Each corpus is fresh and reports:

- p50/p95/p99 and throughput for UDS recall, HTTP recall, UDS remember, and a
  concurrent hook-shaped remember burst;
- bounded-writer-queue enqueue wait, saturation, and capacity;
- dropped change broadcasts, current process RSS, DB size, pinned WAL size,
  shutdown/checkpoint time, and retry/truncate time;
- namespace-isolation and acknowledged-write durability checks.

The default upper bound is 25,000 memories because sqlite-vec search is exact
today. Change `--corpora` to probe a larger local envelope; do not silently
replace the committed default or compare results from different corpus lists.
The harness intentionally keeps deterministic-provider time out of the storage
numbers. Provider timeout, writer-death, and interrupted-write behavior remain
separate fault tests named in the JSON report.

## Disk exhaustion

Disk-full and low-disk results require a disposable quota-limited filesystem.
The harness deliberately does not set process-wide file-size limits because
that can corrupt unrelated files produced by the same build/test process. Until
CI provisions such a mount, the JSON fault matrix reports this case as blocked;
it must not be described as passing.

## Rerunning after schema changes

The harness seeds through public `rb-store` APIs, closes the seeding connection,
then starts the daemon normally. This deliberately exercises all open-time
schema checks and makes the same command suitable for rerun after the task #54
schema-initialization change lands.
