# Scale, concurrency, and resource report — 2026-07-12

## Scope and method

Release-mode measurements ran on Apple silicon (`Darwin vega.luby.us 25.5.0`,
arm64) at commit `9e8312d` plus the task #57 harness changes. The deterministic
8-dimensional provider removes network/provider variance. Fresh corpora used
1,000, 10,000, and 25,000 rows; every row had FTS text and a sqlite-vec vector.
The harness drove the real daemon over UDS and opt-in loopback HTTP, then held a
raw SQLite read transaction while writes and shutdown exercised WAL behavior.

The task runner limits one command to roughly two minutes. The 1k corpus has 50
samples per transport. Exact recall was too slow to collect a useful population
within that bound at 10k/25k, so those corpora have one sample per transport.
Their p50/p95/p99 fields are therefore the same single observation and are
**directional scale evidence, not percentile estimates or release gates**.

## Results

| Corpus | UDS recall p50 / p95 / p99 | HTTP recall p50 / p95 / p99 | UDS remember p50 / p95 / p99 | RSS | DB |
|---:|---:|---:|---:|---:|---:|
| 1k (n=50) | 108.9 / 130.5 / 511.7 ms | 116.0 / 161.5 / 209.7 ms | 1.23 / 1.97 / 2.02 ms | 23.4 MB | 1.01 MB |
| 10k (n=1) | 23.77 s | 26.36 s | 4.82 ms | 26.4 MB | 7.39 MB |
| 25k (n=1) | 174.56 s | **failed at 30 s HTTP deadline** | 19.41 ms | 28.0 MB | 18.73 MB |

At 1k, UDS and HTTP recall throughput was 8.57/s and 8.17/s respectively;
sequential remembers reached 810/s. A 32-client hook-shaped burst committed all
32 writes, with 20.20 ms p50 and 27.97 ms p95 completion latency (1,121/s wall
throughput). At 10k and 25k the smaller four-client bursts also committed fully;
write latency rose, but recall—not storage size or RSS—was the limiting resource.

### Queue saturation and write durability

A separate 1,024-write probe deliberately filled the 256-slot writer queue:

- 767/1,024 enqueues (74.9%) observed a full queue;
- average queue-capacity wait was 42.78 ms; maximum was 115.06 ms;
- end-to-end write completion was 75.52 ms p50, 147.01 ms p95, and
  153.01 ms p99 (6,477/s wall throughput);
- all 1,024 acknowledged writes were visible afterward.

The normal 1k mixed run did not saturate the queue. No-subscriber change
broadcast counters advanced once per committed write (82 in that run); durable
oplog/state verification found no lost committed writes. Namespace-isolation
probes returned no foreign-namespace recall results at every corpus size.

### WAL, readers, and shutdown

| Corpus | WAL while reader pinned | Shutdown/checkpoint | Explicit retry | WAL after retry |
|---:|---:|---:|---:|---:|
| 1k | 5.01 MB | 5.20 s | <1 ms | 0 |
| 10k | 1.25 MB | 5.17 s | <1 ms | 0 |
| 25k | 1.23 MB | 5.20 s | 5.19 s | **1.23 MB remained** |

The 25k HTTP recall timed out, but its `spawn_blocking` SQLite work was not
cancellable and continued after the response deadline. That long read held the
WAL through daemon shutdown and the immediate retry. This is a concrete
read-side resource-exhaustion/backpressure gap: an HTTP deadline bounds client
wait, not underlying blocking DB work or its WAL lifetime.

### Fault matrix

- **Provider timeout:** executed. A 200 ms embedding provider behind a 20 ms
  HTTP deadline returned bounded HTTP 503 and the daemon shut down cleanly.
- **Interrupted/poisoned writer recovery and writer death:** existing focused
  tests were rerun (`caught_writer_panic_isolates_and_does_not_lose_later_writes`,
  `writer_alive_reports_liveness_and_flips_on_death`, and
  `writer_death_exits_the_accept_loop`). The harness separately proves
  acknowledged-write durability under saturation.
- **Long-lived reader:** executed for every corpus; the 25k result above is a
  failure to clear WAL immediately, not a pass.
- **Disk-full/low-disk:** **not executed**. It requires a disposable,
  quota-limited filesystem. Applying a process-wide file-size rlimit inside a
  shared build environment was rejected as unsafe. This remains a release-test
  infrastructure blocker and must not be claimed green.

## Recommended operating envelope

Until retrieval changes, use **about 1,000 active memories per database** for
interactive hybrid recall. At that size, p95 recall stayed under 162 ms across
UDS/HTTP and writes remained low-millisecond. The 10k corpus is acceptable for
durable writes but not interactive recall (24–26 seconds observed); 25k is
outside the supported interactive envelope and can outlive HTTP deadlines.

The queue's existing bounded backpressure worked as designed under a 1,024
write burst: callers waited, memory stayed bounded, and no acknowledged writes
were lost. Keep the 256-command bound for now. Add read-side admission/budgets
or cancellable query work before raising the corpus envelope; an HTTP timeout
alone is insufficient.

## ANN, backpressure, and sharding decision

- **ANN: pursue next, behind quality gates.** Exact retrieval is the evidenced
  bottleneck. Prototype ANN with exact-search recall-quality comparison and the
  task #56 semantic gate before changing defaults.
- **Backpressure: retain write queue; add read-side controls.** No new write
  queue design is justified. The timed-out 25k blocking read demonstrates the
  need for bounded/cancellable read work and explicit saturation metrics.
- **Sharding: defer.** DB/RSS growth remained modest and namespace isolation
  held. Sharding would not address the measured exact-retrieval cliff and would
  add cross-shard ranking/consistency complexity. Reconsider only after ANN and
  read-side controls are measured.

## Reproduction

```bash
scripts/run-scale-benchmark.sh --corpora 1000 --operations 50 --burst 32 \
  --output target/scale-benchmark-1k.json
scripts/run-scale-benchmark.sh --corpora 10000 --operations 1 --burst 4 \
  --output target/scale-benchmark-10k.json
scripts/run-scale-benchmark.sh --corpora 25000 --operations 1 --burst 4 \
  --output target/scale-benchmark-25k.json
```

For statistically meaningful 10k/25k percentiles, run the default 50-operation
matrix unattended outside the task runner and preserve its JSON. Rerun the same
commands after task #54 lands; corpus seeding closes before normal daemon open,
so schema initialization remains in the exercised path.
