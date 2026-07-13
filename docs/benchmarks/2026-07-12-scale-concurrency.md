# Scale, concurrency, and resource report — 2026-07-12

## Status: incomplete

This report records harness development and bounded exploratory runs. It does
**not** establish a production operating envelope. An adequate run still needs
at least 30 successful samples per latency path at 1k, 10k, and a practical
upper bound, using the real 384-dimensional local model in an unattended
environment without this task runner's execution ceiling.

## Corrected harness contract

The committed command defaults to the real local `all-MiniLM-L6-v2` provider
and derives its 384-dimensional vector shape. Corpus document embeddings,
query embeddings, and writes all use that provider. `--dimension` and
`--model-id` are configurable, but the local path fails closed if the requested
dimension disagrees with the loaded model. `--provider fixture` is explicitly
smoke-only and its JSON sets `production_envelope_eligible=false`.

Each corpus now includes:

- concurrent UDS, loopback HTTP, and MCP `tools/call` recall/remember traffic;
- direct writer-queue and read-pool wait/saturation measurements;
- success/error latency accounting for every path;
- pinned-reader write successes/errors plus an exact committed-row check after
  shutdown, freeing the reader, checkpoint retry, and database reopen;
- RSS, DB/WAL size, shutdown/checkpoint time, provider timeout, writer
  recovery/death tests, and an optional guarded disk-exhaustion probe.

## Invalidated pre-review evidence

The original 1k/10k/25k measurements used deterministic 8-dimensional vectors.
They were useful for finding a severe exact-recall cliff and a timed-out
blocking read that pinned WAL, but they are **not production-representative**.
In addition, 10k and 25k had only one observation per transport; those values
are individual durations, not p50/p95/p99 estimates. They cannot support the
old 1k envelope, an ANN decision, or a sharding decision.

For provenance only, the single observations were:

| Exploratory corpus | UDS recall | HTTP recall | Qualification |
|---:|---:|---:|---|
| 1k | p95 130.5 ms (n=50) | p95 161.5 ms (n=50) | 8-dimensional fixture |
| 10k | 23.77 s (n=1) | 26.36 s (n=1) | not a percentile |
| 25k | 174.56 s (n=1) | failed at 30 s deadline | not a percentile |

At 25k the HTTP deadline cancelled the request future but not its
`spawn_blocking` SQLite read. That work continued, leaving 1.23 MB of WAL pinned
through shutdown and an immediate retry. This remains a valid failure-mode
observation, but its corpus threshold must be remeasured with the real model.

## Corrected bounded evidence

### 384-dimensional fixture smoke (100 rows; inadequate n)

This run verified harness wiring only. It used `--provider fixture`, three
sequential operations per transport, six operations per mixed path, and is not
eligible for an envelope:

- all six UDS recall, UDS remember, HTTP recall, HTTP remember, MCP recall, and
  MCP remember operations succeeded while running concurrently;
- the four-connection read pool observed saturation on 157/229 acquisitions
  (68.6%), averaging 4.58 ms permit wait with a 29.06 ms maximum;
- the 1,024-write queue probe observed 767/1,024 saturated enqueues (74.9%);
- 35 writes were acknowledged across sequential, burst, mixed, and pinned-reader
  phases; 127 rows were visible before the eight pinned-reader writes and all
  135 expected rows were present after reopen;
- WAL retry cleared the sidecar.

The real-local ten-row smoke was attempted after the corrected default was
implemented, but the task runner terminated it before a JSON artifact was
written. No latency or envelope claim is made from that attempt.

### Disk-full/low-disk probe

The optional probe ran on a dedicated 64 MiB APFS image mounted under
`/private/tmp`, marked with `.rusty-brain-scale-disposable-volume`. The harness
verified that the supplied path was the filesystem mount root and under the
configured free-space cap before writing filler data.

- The mount began with 63 MiB free.
- Two pressure writes committed.
- The next write failed with `database or disk is full`.
- After the RAII cleanup freed the filler, all three committed rows (baseline
  plus two acknowledged pressure writes) survived reopen.
- The image was detached and deleted after the run.

Without an explicitly supplied dedicated mount root and marker, the probe does
not write filler data; it reports `not_run_requires_explicit_disposable_mount`.
Shared paths, non-mount roots, pre-existing probe files, and mounts over the
free-space cap are refused.

## Decisions still pending

- **Operating envelope:** not established. The previous 1k recommendation is
  withdrawn until adequate real-local samples complete.
- **ANN:** the exploratory exact-recall cliff makes ANN worth evaluating, but
  implementation/default decisions require the real-local scale matrix plus
  task #56's semantic-quality comparison against exact search.
- **Backpressure:** the bounded writer queue and read pool both surfaced
  measurable saturation without losing acknowledged writes in the corrected
  fixture smoke. Production limits still need real-local mixed-load evidence.
- **Sharding:** no decision. Reconsider only after representative ANN/read-side
  controls are measured; the old 8-dimensional DB/RSS numbers are insufficient.

## Remaining blockers

1. Run the default real-local matrix unattended with at least 30 successful
   samples per latency path at 1k, 10k, and a declared practical upper bound.
2. Preserve those JSON artifacts and rerun after task #54 lands.
3. If 10k/upper-bound requests exceed deadlines, report the failed sample
   distribution and residual blocking work; do not collapse failures into zero
   latency or call n=1 a percentile.
4. Use the resulting representative evidence with task #56 before selecting
   ANN, changing backpressure defaults, or proposing sharding.

## Reproduction

```bash
# Real local model (default; compiles rb-eval's record-local feature):
scripts/run-scale-benchmark.sh --corpora 1000,10000,25000 \
  --operations 50 --burst 32 \
  --output target/scale-local-384.json

# Explicit smoke-only deterministic fixture:
scripts/run-scale-benchmark.sh --provider fixture --corpora 100 \
  --operations 3 --burst 6 \
  --output target/scale-fixture-384-smoke.json

# Disk probe: only a dedicated quota-limited mount root with the marker:
scripts/run-scale-benchmark.sh --provider fixture --corpora 10 \
  --operations 1 --burst 2 \
  --disk-exhaustion-dir /path/to/dedicated-mount \
  --disk-exhaustion-max-mib 128 \
  --output target/scale-disk-probe.json
```
