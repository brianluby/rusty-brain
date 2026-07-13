# Local session-replay evaluation dataset

Status: evaluation infrastructure only. This pipeline does not change ranking,
admission, retention, embeddings, safety thresholds, or the blocked Phase 5
pilot.

The pipeline expands the evidence path behind Vikunja #56 with realistic local
session shapes and can generate the controlled 1k/10k/25k distractor inputs
needed by #57. Raw Claude Code transcripts and the OpenCode database remain on
the operator's machine. The default command is an aggregate-only dry run and
prints no transcript text.

## Sources and read boundary

- Claude Code: recursively reads local JSONL beneath `~/.claude/projects/`.
- OpenCode: opens `~/.local/share/opencode/opencode.db` read-only and consumes
  `session`, `message`, and `part` rows. Message role and part content remain
  separate during normalization.
- No network call is made by either adapter, redaction, candidate construction,
  Faker augmentation, or artifact writing.
- The adapters never write raw input. Local output is accepted only in a
  directory whose final component is `session-replay-local`; that directory is
  ignored by Git and written owner-only on Unix.

Run an aggregate dry run:

```bash
cargo run --release -p rb-eval --example session_replay --quiet
```

The JSON on stdout contains counts and rejection/redaction categories only. To
materialize reviewable local datasets:

```bash
cargo run --release -p rb-eval --example session_replay --quiet -- \
  --write-local --output target/session-replay-local
```

Use `--holdout-after <RFC3339>` to apply a preregistered boundary. Without it,
the tool deterministically chooses the 80th percentile of session start times.
In both modes, a session ending before the boundary is development, a session
starting at/after it is holdout, and a session crossing it is rejected. A
session is never split.

## Normalized schema

Every normalized object uses schema
`rusty-brain-session-replay-v1`. Identifiers are deterministic seeded
pseudonyms; no raw session id, project id, database path, JSONL path, message
id, or part id is emitted.

### Session

| Field | Meaning |
|---|---|
| `session_id`, `project_id` | Stable pseudonymous identifiers |
| `source` | `claude_code` or `open_code` |
| `started_at`, `ended_at` | Original chronological bounds in UTC |
| `events` | Events sorted by timestamp then source ordinal |

The local `sessions.jsonl` manifest contains the bounds and event count without
duplicating event content.

### Event

| Field | Meaning |
|---|---|
| `event_id` | Stable pseudonymous event identifier |
| `session_id`, `project_id` | Parent identifiers |
| `timestamp`, `ordinal` | Source time and deterministic tie-break order |
| `role` | `user`, `assistant`, `tool`, or `system` |
| `kind` | `dialogue`, `tool_call`, `tool_result`, `repository_evidence`, or `lifecycle` |
| `authority` | User statement, uncorroborated assistant, tool evidence, committed repository state, or system metadata |
| `content` | Sanitized dialogue text when `kind=dialogue` |
| `tool` | Sanitized tool name/status/input/output for non-dialogue tool events |
| `provenance` | Source kind, pseudonymous locator/record ids, source index, adapter version |
| `redactions` | Category names only; never matched values |

OpenCode tool parts remain tool events. Claude `tool_use` and `tool_result`
blocks remain tool call/result events. Neither is flattened into dialogue.
Private reasoning/thinking content is rejected, not normalized.

## Two lanes

`dialogue.jsonl` contains only user and assistant text events. It preserves the
real alternating message shape needed for semantic replay. Assistant text is
always `assistant_uncorroborated`: it can supply dialogue context but cannot
become an authoritative candidate memory.

`full-events.jsonl` contains dialogue, tool, repository-evidence, and lifecycle
events. Tool results are evidence, not dialogue. A completed `git show`, `git
log`, `git cat-file`, or `git rev-parse` probe may be classified as committed
repository state; other completed tool output remains tool evidence.

The conservative authority order for candidate construction is:

1. user statements;
2. tool evidence;
3. committed repository state;
4. assistant statements only after an explicit future corroboration/review
   step (not implemented or inferred here).

Later corrections therefore win only when represented by user/tool/repository
evidence. The pipeline never promotes an assistant answer merely because it is
later or confident.

## Chronological candidates and queries

Within each whole-session split, eligible earlier events become candidate
memories. A later user turn becomes a proposed natural query only when it has a
refer-back marker such as “earlier,” “last time,” “what did we decide,” or
“where we left off,” and at least one earlier eligible candidate exists.

Automatic extraction does not know relevance. Consequently:

- every extracted candidate and query has `review_status=unreviewed`;
- every extracted record has `semantic_ground_truth=false`;
- a query's `candidate_pool_ids` is the earlier chronological pool in
  newest-first order (so later corrections precede older claims), not a
  relevance judgment;
- no unreviewed local artifact may be committed or used as a semantic label.

An explicitly reviewed sanitized fixture may be copied into the committed
fixture tree in a separate review, changing `review_status` only after a human
checks privacy, provenance, correction handling, and relevance.

## Privacy and failure behavior

All strings that could be emitted pass through `rb-redact` and the stricter
session replay redactor. It replaces or rejects:

- credential assignments, recognized token families, private keys, bearer
  values, and high-entropy tokens;
- email addresses, phone numbers, IP addresses, URLs, hostnames, and labeled
  user/account identifiers;
- absolute home paths and selected other absolute local paths;
- labeled personal names and organization/client/company values.

Faker aliases are consistent within a session. After replacement, the shared
secret redactor and every sensitive-value detector run again. A non-fixpoint,
residual match, unavailable pattern set, or oversized field rejects the event.
The aggregate report records only the rejection category. Pattern redaction is
still not proof of anonymity; owner-only files, Git ignore rules, aggregate
reporting, and human review are independent backstops.

## Faker augmentation contract

- Rust crate `fake` is pinned exactly to `5.1.0`.
- Rust crate `rand` is pinned exactly to `0.10.2`; changing the RNG version is
  an explicit dataset-version event because `StdRng` does not promise a stable
  algorithm across releases.
- The default seed is pinned to `0x5255535459425241` and is recorded in every
  report and generated record. A different seed must be passed explicitly and
  retained with the evaluation artifact.
- The same source value maps to the same fake alias within a session.
- Controlled variants add one of two fixed, auditable frames around already
  sanitized candidate text. They are marked `semantic_ground_truth=false`.
- Distractors use fixed templates; Faker supplies identifiers and numeric
  dimensions only. Faker lorem/prose is never an answer, relevance label, or
  semantic ground truth.
- The committed scale shapes are exactly 1,000, 10,000, and 25,000 distractors,
  each with `relevant=false` and `semantic_ground_truth=false`.

## Local outputs

`target/session-replay-local/` contains:

- `inventory.json` — aggregate-only counts/categories;
- `sessions.jsonl` — pseudonymous whole-session manifest;
- `dialogue.jsonl` — dialogue-only lane;
- `full-events.jsonl` — full event lane;
- `candidates.jsonl` and `queries.jsonl` — unreviewed chronological proposals;
- `controlled-variants.jsonl` — augmentation-only variants;
- `distractors-1000.jsonl`, `distractors-10000.jsonl`, and
  `distractors-25000.jsonl` — known-irrelevant scale corpora.

On Unix, the writer creates the output directory with mode `0700`, writes files
with mode `0600`, and uses same-directory temporary files plus atomic
replacement. Other platforms fail before creating output because the standard
filesystem API cannot guarantee equivalent owner-only permissions.

Raw transcripts, raw database copies, unreviewed normalized content, and these
generated files must never be staged. Only parsers, schemas, invented fixtures,
tests, documentation, aggregate reports, and separately reviewed sanitized
fixtures belong in Git.

## Verification

```bash
cargo test -p rb-eval --test session_replay
cargo test -p rb-eval session_replay --lib
cargo clippy -p rb-eval --all-targets --all-features -- -D warnings
cargo fmt --all --check

# The evaluation/Faker closure must not leak into the shipped binary.
cargo tree -e normal -p rusty-brain | rg 'rb-eval|fake' && exit 1 || true
```
