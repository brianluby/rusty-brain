# rusty-brain

> Local-first, persistent memory for AI coding agents — a Rust daemon with hybrid
> (vector + keyword + graph) retrieval over SQLite, exposed via MCP and a CLI.
>
> **Status: early development.** The pieces described below are implemented and
> covered by unit and integration tests, but the project has only had **basic
> correctness testing**. It has **not** been performance-, scale-, or
> capability-tested, and retrieval quality has not been measured against real
> embedding models at any meaningful corpus size. Treat it as a work in progress,
> not a finished product. Interfaces and on-disk format may change.

## What it is

`rusty-brain` is a small memory substrate that an AI coding agent (or several,
concurrently) can write notes to and recall from. It runs as a single local
daemon backed by one SQLite database, and is reachable two ways:

- a **command-line interface** (`rusty-brain`), and
- an **MCP server** (`rusty-brain mcp`) that exposes memory operations as tools
  to any MCP-capable agent.

Memories are scoped to a **namespace** (global, per-project, or per-session),
carry lightweight enrichment (summary, keywords, tags, type, importance), and can
be linked to one another. Recall fuses full-text search, vector similarity, and
graph proximity into a single ranked result set.

It is deliberately a *substrate*, not an orchestrator: it stores and retrieves
memory for agents but has no notion of tasks, scheduling, or agent lifecycles.

## Background

Before building this, we surveyed a number of existing agentic-memory systems.
They informed the design — our recency/importance/relevance ranking and our
treatment of consolidation and forgetting are well-trodden ideas — but their
**architectures did not align with our design criteria**. We wanted something
local-first, built from small purpose-built crates with compiler-enforced
boundaries, with a single source of truth on disk and a lean default dependency
footprint, rather than a large general-purpose system. `rusty-brain` is that
focused rebuild. The principles we settled on:

- **A workspace of focused crates, never a monolith** — boundaries enforced by the
  compiler, heavy/optional pieces behind feature flags or separate crates.
- **One database, one transaction, one source of truth** — memories, full-text
  index, and vectors live in a single SQLite file; writes serialize through one
  thread, reads run concurrently over WAL.
- **Fail-closed where it matters, fail-open where it shouldn't** — namespace
  isolation and the embedding-dimension contract refuse to run on doubt; best-effort
  capture hooks never block or break an agent session.
- **Reproducible from git** — file-discovered, checksummed migrations; CI rebuilds a
  fresh database from committed SQL and exercises every query path.

## Architecture (high level)

```mermaid
flowchart TB
  subgraph clients["Clients"]
    CLI["rusty-brain CLI"]
    AGENT["MCP-capable agent"]
    HOOKS["rusty-brain-hooks<br/>(capture, fail-open)"]
  end

  AGENT -->|"JSON-RPC over stdio"| MCPSRV["rusty-brain mcp<br/>(MCP server)"]

  subgraph daemon["rusty-brain daemon (one local process)"]
    ENGINE["engine<br/>enrich → embed → store → link → recall"]
    WRITER["single writer thread"]
    POOL["read pool (WAL)"]
    BUS["change broadcast"]
  end

  CLI -->|"Unix socket"| daemon
  MCPSRV -->|"Unix socket"| daemon
  HOOKS -->|"Unix socket"| daemon

  ENGINE --> WRITER
  ENGINE --> POOL
  ENGINE --> EMB["embeddings<br/>deterministic · Voyage · local ONNX"]
  WRITER --> DB[("SQLite + sqlite-vec<br/>memories · vectors · links · FTS")]
  POOL --> DB
  WRITER --> BUS

  classDef client fill:#e3f2fd,stroke:#4781c4,color:#0d2b4e;
  classDef proc fill:#ede7f6,stroke:#7e57c2,color:#311b54;
  classDef store fill:#fff3e0,stroke:#ef8e3a,color:#5a3209;
  classDef embed fill:#e8f5e9,stroke:#52a45a,color:#1b3d20;
  classDef accent fill:#fce4ec,stroke:#c96198,color:#5a1535;
  class CLI,AGENT,HOOKS,MCPSRV client;
  class ENGINE,WRITER,POOL proc;
  class EMB embed;
  class DB store;
  class BUS accent;
  style clients fill:#f2f7fd,stroke:#9bbbe0,color:#0d2b4e;
  style daemon fill:#f6f3fb,stroke:#b9a7da,color:#311b54;
```

A deeper walkthrough — component boundaries, the write and recall data flows, and
the namespace model — lives in [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).

## What works today

Implemented and test-covered (correctness only — see [Testing](#testing)):

- **Store and recall** memories scoped by namespace, with enrichment and graph links.
- **Hybrid retrieval** fusing FTS5 keyword search, `sqlite-vec` vector similarity, and
  1-hop graph expansion. Ranking is a weighted linear blend of vector/keyword/graph/
  importance/recency signals by default, with an opt-in Reciprocal Rank Fusion (RRF)
  mode and a confidence dampener.
- **Contradiction surfacing** — results carry a `contested` flag when a memory has an
  active `contradicts` link (best-effort, never fails recall).
- **A single-writer daemon** over SQLite WAL with a concurrent read pool, change
  notifications via an in-process broadcast, and auto-start from any client command.
- **An MCP server** exposing ten tools over stdio.
- **A CLI** for direct use and scripting.
- **Pluggable embeddings** — an offline deterministic provider (default), a Voyage API
  provider, and an optional local ONNX provider behind a feature flag.
- **Best-effort capture hooks** (`rusty-brain-hooks`) and an installer
  (`rusty-brain-install`) for wiring memory capture into agent CLIs.

Planned / deferred (designed but not built): LLM-assisted memory evolution
(reconciliation, reflection), a networked multi-host surface with real auth, and an
ANN vector index for larger corpora. See [Roadmap](#roadmap).

## Quickstart

### Install

Tagged releases ship prebuilt, checksummed tarballs for macOS (Apple Silicon and
Intel) and Linux x86_64 on the
[releases page](https://github.com/brianluby/rusty-brain/releases). Each tarball
contains the three binaries (`rusty-brain`, `rusty-brain-hooks`,
`rusty-brain-install`). One-line install (needs the `gh` CLI; pick your target):

```bash
TARGET=aarch64-apple-darwin   # or x86_64-apple-darwin / x86_64-unknown-linux-gnu
gh release download --repo brianluby/rusty-brain --pattern "*-${TARGET}.tar.gz" --pattern SHA256SUMS && shasum -a 256 --check --ignore-missing SHA256SUMS && mkdir -p ~/.local/bin && tar -xzf rusty-brain-*-"${TARGET}".tar.gz --strip-components=1 -C ~/.local/bin
```

(`sha256sum` on Linux; make sure `~/.local/bin` is on your `PATH`. Optionally verify
build provenance first: `gh attestation verify rusty-brain-*.tar.gz --repo
brianluby/rusty-brain`.)

Or manually: download the tarball for your platform plus `SHA256SUMS` from the
release, check it, and unpack the binaries somewhere on your `PATH`.

Or build from source:

### Prerequisites

- Rust **stable** (pinned via `rust-toolchain.toml`). SQLite is bundled (no system
  dependency).

### Build

```bash
cargo build --release
# binaries: target/release/rusty-brain  (+ rusty-brain-hooks, rusty-brain-install)
```

### Use the CLI

Client commands auto-start the daemon on first use (and connect to it thereafter),
so you can go straight to storing and recalling:

```bash
# seed a fresh brain from existing project context (CLAUDE.md, README,
# CHANGELOG.md, docs/**/*.md, and recent decision-ish git commits)
rusty-brain init --yes

# preview a one-off text/markdown import without storing anything
rusty-brain import docs/architecture.md --dry-run

# store a memory (namespace is detected from the current project / git root)
rusty-brain remember "We use a single-writer daemon over SQLite WAL" \
  --type architecture_decision --importance 8 --tags storage --tags concurrency

# recall by free-text query (hybrid keyword + vector + graph)
rusty-brain recall "how is writing serialized?" --limit 5

# fetch, list, traverse links, see project context
rusty-brain get <id>
rusty-brain list --min-importance 6
rusty-brain graph <id> --depth 1
rusty-brain context

# adjust a memory you no longer fully trust, or mark two memories as
# contradicting (both then surface `contested: true` on reads)
rusty-brain update <id> --confidence 0.3
rusty-brain link <from-id> <to-id> --type contradicts --reason "policy reversed"

# retroactively redact secrets from every stored memory, then re-embed
rusty-brain scrub
rusty-brain reembed

# list or undo first-run/import batches
rusty-brain init --list-batches
rusty-brain init --undo <batch-id>

# export memories (post-redaction, sorted by id for git-diffability)
rusty-brain export --format markdown
rusty-brain export --format json --min-importance 5
rusty-brain export --format csv

# one-command timestamped backup (with optional retention pruning)
rusty-brain backup
rusty-brain backup --retention 5 --format json
rusty-brain backup --list

# restore from a JSON export (idempotent via dedup, re-embeds under current model)
rusty-brain restore backup-20260702T210230.123456789.json
echo "$JSON" | rusty-brain restore -

# add --json to any command for machine-readable output
rusty-brain recall "sqlite" --json
```

You can also run the daemon explicitly in the foreground:

```bash
rusty-brain serve            # runs until Ctrl-C
```

### Embedding providers

By default the daemon uses an **offline deterministic** provider. It is reproducible
and requires no network or API key, but its vectors are **not semantic** — they exist
so the system runs and so ranking is deterministic in tests, not to give
good real-world recall. For meaningful semantic recall, configure a real provider:

| Provider | How to select | Notes |
|---|---|---|
| Deterministic (default) | (none) | Offline, non-semantic. Fallback when nothing else is configured. |
| Voyage API | set `VOYAGE_API_KEY` | Remote HTTP embeddings (`voyage-3-lite`, dim 512). |
| Local ONNX | build `--features local`, then set `RB_EMBED_BACKEND=local` | Downloads `all-MiniLM-L6-v2` (dim 384) on first use via `fastembed`. |

The embedding dimension AND model identity are fixed at first daemon init and checked
fail-closed on every startup (vector reads are length-checked too); switching
providers — or models, even at the same dimension — against an existing database
refuses to run rather than silently mix vector spaces. To accept a deliberate model
swap, restart with `rusty-brain serve --accept-model-change` (or set
`RB_ACCEPT_MODEL_CHANGE=1` for auto-started daemons) and re-embed the corpus with
`rusty-brain reembed`.

### Wire the MCP server into an agent

`rusty-brain mcp` speaks newline-delimited JSON-RPC (MCP) over stdin/stdout. Point an
MCP-capable agent at it as a stdio server, for example:

```json
{
  "mcpServers": {
    "rusty-brain": { "command": "rusty-brain", "args": ["mcp"] }
  }
}
```

It exposes eleven tools: `remember`, `recall`, `get`, `list`, `graph`, `update`,
`link` (e.g. mark one memory as contradicting another), `delete`, `context`,
`memory_feedback` (report whether a recalled memory was `helpful`/`wrong`/`stale`,
nudging its trust prior), and `poll_changes` (drains buffered change
notifications).

### Capture hooks (optional)

`rusty-brain-install` wires the `rusty-brain-hooks` binary into a supported agent
CLI's hook configuration so that file edits, session starts/stops, and similar events
are captured into memory automatically. The hooks are **fail-open**: any error
degrades silently and never blocks the agent. The agent surface is
**cross-agentic**: OpenCode and Codex are first-class targets alongside Claude
Code, with Hermes discovery-gated. Per-agent setup, validation commands, and
troubleshooting live in [docs/AGENTS.md](docs/AGENTS.md).

Current agent capability matrix:

| Agent | Adapter | Capture | Retrieval | Config | Scorecard | Lifecycle source / limitation |
|---|---|---|---|---|---|---|
| `claude-code` | stable | supported | supported | supported | supported | Fixture-backed `SessionEnd` lifecycle under `crates/rb-hooks/tests/fixtures/claude_code/`. |
| `codex` | experimental | partial | unsupported | partial | unsupported | `Stop` remains a no-op boundary until real fixtures prove a checkpoint or terminus; `apply_patch` capture is upstream-blocked ([openai/codex#16732](https://github.com/openai/codex/issues/16732)). |
| `opencode` | experimental | supported | unsupported | unsupported | unsupported | `session.idle` folds via canonical `SessionCheckpoint`; bash and `apply_patch` file-edit capture are fixture-backed. Installer/plugin support is deferred. |
| `gemini` | experimental | partial | unsupported | partial | unsupported | Native `SessionEnd` remains canonical `Stop`; terminus mapping is descoped from the cross-CLI track. |
| `hermes` | discovery | unknown | unknown | unknown | unsupported | Discovery-gated; no hook names or config paths are hard-coded. |

The scorecard runner is target-aware:

```bash
scripts/memory-scorecard.sh --agent claude-code --runs 1
scripts/memory-scorecard.sh --agent codex --runs 1      # explicit skip today
scripts/memory-scorecard.sh --agent opencode --runs 1   # explicit skip today
scripts/memory-scorecard.sh --agent all --runs 1        # prints skips, then runs Claude Code
```

## Configuration

Daemon knobs live in a user config file: `~/.config/rusty-brain/config.toml`
(or `$XDG_CONFIG_HOME/rusty-brain/config.toml`). Precedence, per knob:

**CLI flag > environment variable > config file > built-in default.**

Every binary — the CLI, the hooks, and the daemon — re-reads this file from
disk itself, so a knob set here reaches **auto-started** daemons with no env
forwarding. The auto-start env allowlist is therefore only secrets
(`VOYAGE_API_KEY`, `RB_ENRICH_API_KEY`), identity fallback (`USER`/`LOGNAME`),
and path resolution (`HOME`/`PATH`/`XDG_*`); the pre-existing knob env vars
below stay supported for compatibility (env wins over the file), but new knobs
are config-file-only. A missing file means all defaults; **unknown keys warn
and are ignored** (forward compat); **malformed TOML fails closed** with a
message naming the file. Secrets are deliberately env-only: the config file
never holds credentials, and `accept_model_change` is deliberately not a file
knob (consent must be explicit per model change).

```toml
# ~/.config/rusty-brain/config.toml — every key optional
socket_path = "/run/user/1000/rusty-brain/sock"
db_path = "/home/me/.local/share/rusty-brain/memory.db"
idle_timeout_secs = 60
jobs_config = "/home/me/.config/rusty-brain/jobs.toml"

[embed]
backend = "local"            # "local" forces the local ONNX provider
local_model = "all-MiniLM-L6-v2"

[enrich]
base_url = "http://localhost:11434/v1"
model = "llama3"             # both base_url and model required to activate

[search]
fusion = "linear"            # or "rrf" (Reciprocal Rank Fusion) for recall ranking
```

Two further small files exist for namespace identity (and identity ONLY — a
repo-committed file must never be able to set sockets, paths, or backends): a
repo-committed `.rusty-brain.toml` at the git toplevel (`namespace = "..."`)
pins a project's namespace across clones — it is read from the blob committed
at `HEAD` (`git show HEAD:.rusty-brain.toml`), so untracked or uncommitted
edits are ignored with a warning until committed — and a per-user pin store
(`namespace-pins.toml` under the user state dir) records `CLAUDE.md`
frontmatter overrides accepted via `rusty-brain --accept-namespace-override` —
unpinned overrides are never silently honored.

| Variable | Config-file key | Purpose |
|---|---|---|
| `RUSTY_BRAIN_DB` | `db_path` | SQLite database path (default under the user data dir: `~/Library/Application Support` on macOS, `$XDG_DATA_HOME` or `~/.local/share` on Linux). |
| `RUSTY_BRAIN_SOCKET` | `socket_path` | Unix socket path for the daemon. |
| `RUSTY_BRAIN_NAMESPACE` | — | Skip namespace detection and use this namespace (same precedence as the global `--namespace` flag). |
| `RUSTY_BRAIN_IDLE_TIMEOUT_SECS` | `idle_timeout_secs` | Per-connection request idle timeout for the daemon (default 60; mainly for tests). |
| `VOYAGE_API_KEY` | — (secret, env-only) | Selects the Voyage embedding provider when set. |
| `RB_EMBED_BACKEND` | `embed.backend` | `local` forces the local ONNX provider (requires `--features local`). |
| `RB_LOCAL_MODEL` | `embed.local_model` | Local model name (implies local backend); defaults to `all-MiniLM-L6-v2`. |
| `RB_ACCEPT_MODEL_CHANGE` | — (deliberately env/flag-only) | Opt in to an embedding-model swap on an existing DB (flag equivalent: `serve --accept-model-change`); follow with `rusty-brain reembed`. |
| `RB_ENRICH_BASE_URL`, `RB_ENRICH_MODEL` | `enrich.base_url`, `enrich.model` | Optional LLM enrichment endpoint (off by default; heuristic enrichment is used otherwise). |
| `RB_ENRICH_API_KEY` | — (secret, env-only) | API key for the enrichment endpoint (optional, e.g. Ollama needs none). |
| `RB_JOBS_CONFIG` | `jobs_config` | Path to an evolution-jobs TOML config for `serve` (jobs are disabled if absent). |
| `RB_FUSION_MODE` | `search.fusion` | Recall fusion strategy: `linear` (default) or `rrf`. |
| `RUST_LOG` | — | Log verbosity (logs go to stderr; stdout is reserved for results). |

## Workspace layout

Thirteen crates, plus a dev-only evaluation harness. Each crate is small and
single-purpose; dependencies form a compiler-enforced DAG.

| Crate | Responsibility |
|---|---|
| `rb-types` | Domain vocabulary: `MemoryId`, `Namespace`, `MemoryNote`, errors, enums. |
| `rb-store` | SQLite + `sqlite-vec` engine: schema, migrations, FTS, vector KNN, graph queries. |
| `rb-proto` | Daemon wire protocol: request/response types, length-delimited JSON framing, socket client. |
| `rb-embed` | `EmbeddingProvider` trait + deterministic / Voyage / local (feature-gated) providers. |
| `rb-search` | Pure hybrid ranking functions (Linear and RRF). |
| `rb-engine` | Per-request orchestration: enrich → embed → store → link → recall. |
| `rb-enrich` | Opt-in LLM enrichment and semantic linking (heuristic offline by default). |
| `rb-daemon` | Single-writer service: writer thread, read pool, socket listener, change broadcast, namespace isolation. |
| `rb-mcp` | MCP stdio adapter (the ten tools). |
| `rb-redact` | Shared secret-redaction pass (rules + entropy sweep) for capture and `scrub`. |
| `rusty-brain` | The `rusty-brain` binary: `serve`, `mcp`, and client subcommands. |
| `rb-agents` | CLI-agnostic agent hook spine: event model and per-CLI adapters. |
| `rb-hooks` | The `rusty-brain-hooks` capture binary (fail-open). |
| `rb-install` | The `rusty-brain-install` binary: wire/unwire hooks into agent CLIs. |
| `rb-eval` *(dev-only)* | Offline deterministic regression harness; excluded from the shipped binary. |

## Development

```bash
cargo build                                                   # default (lean) build
cargo build -p rusty-brain --features local                   # with local ONNX embeddings
cargo test --workspace                                        # all tests
cargo test -p rb-eval                                         # ranking regression harness
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all --check
cargo deny check                                              # supply-chain / license policy
```

Conventions:

- Workspace lints **deny** `unwrap`, `expect`, and `panic` by default, with a few
  narrow, explicit per-module exceptions (e.g. panic-recovery and test-support seams);
  `unsafe` is warned. Shipped request paths return errors, they don't panic.
- Migrations are append-only and checksummed; a CI gate rebuilds a fresh database from
  committed SQL and exercises every query path.
- Commits follow Conventional Commits.

## Testing

The current test suite is about **correctness, not performance or quality**:

- **Unit and integration tests** across the workspace cover type invariants, ranking
  determinism, protocol round-trips, the FTS/vector/graph query paths, migration
  reproducibility, concurrency and namespace isolation, and the MCP contract.
- **`rb-eval`** is an offline regression harness over a committed fixture corpus with
  deterministic (non-semantic) vectors. It guards **ranking determinism and
  relative-ordering regressions** — "did this change reorder results?" — and explicitly
  does **not** measure absolute semantic quality.
- **Nightly real-agent smoke** (`.github/workflows/nightly-claude-smoke.yml`): a
  scheduled macOS job drives a real headless Claude Code session (`claude -p`) against
  freshly built binaries with hooks + MCP installed into an isolated `HOME`, then proves
  capture → idle → recall against exactly one daemon and one `0600` database,
  and greps the raw DB bytes for a planted fake AWS key (zero plaintext hits, redaction
  marker present). It is scheduled-only — it never gates PRs — and on failure it opens
  or updates a pinned issue titled "nightly claude smoke failing". It requires the
  **`ANTHROPIC_API_KEY`** repository secret (small spend: the session runs
  `--model haiku --max-budget-usd 1`). Run it locally with
  `scripts/nightly-claude-smoke.sh --bin-dir target/release`.

**Not yet tested:** performance and latency, behavior at scale (large corpora, many
concurrent clients), real-world semantic recall quality with a production embedding
model, and failure modes under resource exhaustion or partial outages. A real-model
evaluation mode exists but is run manually and is not part of CI.

## Roadmap

Implemented through the retrieval-quality phase (store/recall, the daemon, MCP and CLI
surfaces, agent capture hooks, composite embeddings, RRF, confidence, and contradiction
surfacing). Designed but not yet built:

- LLM-assisted memory **evolution** (reconciliation, reflection, importance
  recalibration) as opt-in background jobs.
- A **networked / multi-host** surface with real authentication (today it is
  single-machine and per-user).
- An **ANN vector index** for larger corpora (`sqlite-vec` brute-force KNN is the
  documented current limit).

## License

Released under the [MIT License](LICENSE).
