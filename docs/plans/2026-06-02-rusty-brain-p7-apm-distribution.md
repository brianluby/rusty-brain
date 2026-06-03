# P7 — APM Package Distribution Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Implement Parts strictly in build order **A → B → C → D**; each Part ends with a gate that must be green before the next Part starts.

**Goal:** Publish rusty-brain as a first-class Microsoft APM (Agent Package Manager) package — a committed `apm/apm.yml` plus bundled memory instructions/skills/prompts/hooks — and make the binary self-describing and the installer APM-aware: a read-only `rusty-brain apm emit|validate|doctor` CLI keeps the manifest in lockstep with the binary's MCP surface, and an `rb-install` APM delegation backend prefers `apm install` when `apm` is on PATH while falling back to the existing P4 direct-config installer, fail-open end to end.

**Architecture:** P7 is additive and distribution-side. It adds no daemon, store, or wire-protocol changes. A new `apm` subcommand group on the `rusty-brain` binary emits the canonical stdio MCP descriptor (carrying `rb_proto::CONTRACT_VERSION`), validates a committed `apm.yml` against the package's expectations (correct stdio entry, no literal secrets, present `ref`), and runs a read-only `doctor` prerequisites probe — none of these mutate any config (mutation is APM's job). `rb-install` gains an `apm` backend that reuses P4's executable-bit PATH detection (`detect::find_binary_on_path`) to find `apm`, delegates to `apm install` via an `env_clear()`-hardened subprocess, and falls back to the existing per-CLI installer (`engine::run_install`) when `apm` is absent — every path fail-open. The package's YAML manifest declares one stdio MCP server (`command: rusty-brain`, `args: ["mcp"]`, `registry: false`) and a `dependencies.apm` mapping referencing the bundled artifacts by hash-pinnable git refs.

**Tech Stack:** Rust 2021 (stable, pinned). Workspace crates touched: `rusty-brain` (new `apm` module + CLI subcommand), `rb-install` (new `apm_backend` module), plus committed data files under `apm/`. Reused: `clap` (CLI), `serde`/`serde_json` (descriptor/report types), `rb_proto::CONTRACT_VERSION` (version surfacing), `rb_install::detect::find_binary_on_path` (PATH+exec-bit detection), `rb_install::engine::run_install` (P4 fallback). One new dep — `serde_yaml_ng` (the maintained drop-in fork of `serde_yaml`; MIT/Apache-2.0, no advisory, deny-clean) — added in Part B for parsing `apm.yml` during `validate` (YAML is APM's manifest format; the workspace has no YAML parser). `serde_yaml_ng` is chosen because the original `serde_yaml` is unmaintained (RUSTSEC-2024-0320) and would fail the repo's `cargo deny check`. Tests are TDD, in-process, offline: a fake `apm` stub script on PATH asserts delegation args; a fixture test parses the committed `apm/apm.yml`; the real `apm install` smoke test is `#[ignore]`.

**Reference spec:** `docs/specs/2026-06-02-rusty-brain-p7-apm-distribution.md` — §5 (package shape + manifest), §6 (`apm` CLI), §7 (`rb-install` backend), §8 (security), §9 (testing). Architecture: `docs/specs/2026-05-31-rusty-brain-architecture-design.md` §12 (interfaces/`ContractVersion`), §14 (security). Style template: `docs/plans/2026-06-02-rusty-brain-p3-deferred-features.md`.

---

## Hard rules (carry forward from P0–P4; apply to every task)

- **TDD:** failing test first (RED), minimal implementation (GREEN), then clippy + fmt, then commit. One logical change per commit.
- **Conventional commits**, lowercase, crate-scoped, one line, **NO AI attribution** (no "Generated with…", no `Co-Authored-By`).
- **`rusty-brain apm` is read-only:** `emit`/`validate`/`doctor` NEVER mutate any config, never write the daemon, never spawn `apm install`. Mutation is APM's job (or `rb-install`'s fallback). `doctor` may run `<bin> --version`-style probes but writes nothing.
- **`rb-install` APM backend is fail-open end to end:** detection failure, a missing `apm`, or a non-zero `apm install` exit logs and falls back to the P4 direct-config installer or no-ops — it NEVER breaks a user's harness setup and NEVER returns a non-zero process exit (mirrors P4 `exit_code` always-0).
- **No secrets in `apm.yml`:** the manifest uses env interpolation only (`${VAR}`-style); `rusty-brain apm validate` rejects any literal secret (a value that looks like a `voy-…` / `sk-…` / hex/base64 API token, or a `VOYAGE_API_KEY`-keyed literal) with a non-zero exit. The committed `apm/apm.yml` contains NO secret.
- **stdio transport only / local model unchanged:** the package's MCP entry is `transport: stdio`, `registry: false`, `command: rusty-brain`, `args: ["mcp"]`; the daemon's 0600 UDS and namespace isolation are untouched. No network surface is added.
- **Subprocess hardening (global security rule):** any `apm` subprocess spawn uses `Command::env_clear()` then sets ONLY the vars it needs (`PATH`, `HOME`/`USERPROFILE`) — never inherit the parent environment wholesale. PATH detection for `apm` requires an executable bit (reuse `rb_install::detect::find_binary_on_path`). **TOCTOU:** re-check `apm` presence immediately before spawning `apm install`, not just at backend selection.
- **Capture hooks stay fail-open:** the optional `apm/hooks/` artifact ports the P4 capture-hook ethos — a hook installed via the package never blocks or breaks an agent session.
- **No-panic in non-test code:** workspace lints deny `unwrap_used`/`expect_used`/`panic`. Return `rb_types::Error`/`anyhow::Error` (binary top level) or a typed module error. Test modules opt out with `#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]`.
- **Default closure unchanged:** `rb-install`/`rb-hooks`/`rb-agents` MUST stay out of the default `rusty-brain` (non-dev) dependency closure (CI `build-agents` job asserts this). The `apm` module lives in the `rusty-brain` binary and pulls in NO agent crate.
- **Per-Part gate** (final task of each Part): `cargo test --workspace`; `cargo clippy --workspace --all-targets --all-features -- -D warnings`; `cargo fmt --all --check`. Parts that add a dep (Part B adds `serde_yaml_ng`, the maintained fork of the unmaintained `serde_yaml`/RUSTSEC-2024-0320) also run `cargo deny check`.
- **Commands run from the worktree root** (so commands are plain `cargo test -p <crate>`). The committed package lives at `apm/` under the repo root.

## Seam map (verified against `origin/main` + the local P3 worktree; the exact code each Part builds on)

| Seam | Location | Used by |
|---|---|---|
| `CONTRACT_VERSION: u32 = 1` | `crates/rb-proto/src/messages.rs` (re-exported `rb_proto::CONTRACT_VERSION`) | B (`apm emit` embeds it) |
| clap `Command` enum (`Serve`/`Mcp`/`Remember`/…/`Evolve`) + `#[arg(long, global=true)] json` | `crates/rusty-brain/src/cli.rs` | B (adds `Apm { … }`) |
| `run(cli, namespace)` → `Serve`/`Mcp` early-handled, else `run_client` | `crates/rusty-brain/src/run.rs` | B (adds an `Apm` arm BEFORE the client connect, since `apm` never touches the daemon) |
| MCP entry contract: stdio `command: rusty-brain`, `args: ["mcp"]`; daemon auto-starts on first connect | `crates/rusty-brain/src/mcp.rs` (`run_mcp`), spec §4/§5 | A, B (`emit` descriptor mirrors this) |
| `find_binary_on_path(name) -> Option<PathBuf>` — `$PATH` scan requiring an exec bit; no shell | `crates/rb-install/src/detect.rs` (re-exported `rb_install::find_binary_on_path`) | C (detects `apm`) |
| `version_of(&Path) -> Option<String>` — `<bin> --version` under a 2s timeout, kills the child | `crates/rb-install/src/detect.rs` | C (optional `apm --version` in the report) |
| `run_install(installers, hooks_bin, scope, dry_run) -> InstallReport`, `resolve_hooks_bin()`, `select_installers()` | `crates/rb-install/src/engine.rs` | C (the P4 fallback path) |
| `InstallReport` / `AgentReport` / `AgentStatus` / `ReportStatus` JSON report types + `roll_up` | `crates/rb-install/src/report.rs` | C (the backend returns one of these) |
| `InstallScope::{Project(PathBuf),Global}` | `rb_agents::install::InstallScope` (via `rb-install`) | C (scope plumbed to fallback + delegation cwd) |
| `Cli`/`Command` (`Install`/`Uninstall`/`Status`) + `execute`/`render`/`exit_code` (always 0) | `crates/rb-install/src/cli.rs` | C (Install routes through the apm backend) |
| `assert_cmd::Command::cargo_bin(...)`, fake-binary-on-PATH test pattern (`fake_claude_path`) | `crates/rb-install/tests/cli.rs` | C, D (fake `apm` stub + fixture tests) |
| `deny.toml` permissive allowlist + crate-scoped exceptions; `[graph] all-features = true` | `deny.toml` (origin/main) | B (confirm `serde_yaml_ng` license is allowed) |
| CI jobs `fmt`/`clippy-test`/`deny`/`audit`/`build-agents` | `.github/workflows/ci.yml` (origin/main) | D (adds an `apm-manifest` validation job) |

## Build order & dependencies

```text
Part A  APM package artifacts          (independent data files: apm/apm.yml + instructions/skills/prompts/hooks; a fixture test parses + secret-scans the manifest)
Part B  rusty-brain `apm` CLI          (emit/validate/doctor on the binary; depends on A's manifest for the round-trip + validate fixture)
Part C  rb-install APM backend         (capability detection + delegation + P4 fallback + fail-open; reuses C's detect/engine seams; depends on nothing in A/B at compile time)
Part D  CI manifest validation + gate  (CI job runs `rusty-brain apm validate apm/apm.yml`; final cross-Part gate)
```

Parts are mostly independent: Part C (the `rb-install` backend) compiles without Parts A/B. The ordering puts the committed package first (A) so Part B's `validate` and the round-trip test have a real manifest to assert against, and Part D wires CI once the binary subcommand exists. Part B introduces the only new dependency (`serde_yaml_ng`) and runs `cargo deny check` in its gate.

---

## Part A — APM package artifacts (the committed `apm/` package)

This Part lands the static, hand-authored package APM consumes: the `apm.yml` manifest declaring the stdio `rusty-brain` MCP server and a `dependencies.apm` mapping of bundled artifacts, plus the bundled `instructions/`, `skills/memory/`, `prompts/`, and an optional fail-open `hooks/` entry. These are data files (no Rust crate), so the only test is a fixture test compiled into the `rusty-brain` binary's test target that reads the committed `apm/apm.yml`, asserts it parses as JSON-compatible-but-YAML-shaped text via a minimal structural check (Part A uses a string-level check; Part B upgrades to a typed YAML parse once `serde_yaml_ng` is added), confirms the stdio MCP entry is present, and scans for literal secrets.

HARD RULES honored throughout: NO secret appears in any committed file (env interpolation only); the MCP entry is `transport: stdio` / `registry: false`; the bundled hook ports the fail-open ethos.

---

### Task A1: `apm/apm.yml` — the package manifest

Author the committed manifest: one stdio MCP server plus a `dependencies.apm` mapping (object shape — APM rejects a flat list) referencing the bundled artifacts by hash-pinnable git refs.

**Files:**
- Create: apm/apm.yml
- Create: crates/rusty-brain/tests/apm_manifest.rs

**Test:** a `rusty-brain` integration test reads `apm/apm.yml` from the repo root, asserts the stdio MCP entry exists, and finds no literal secret.

- [ ] **Step 1 RED: write the failing fixture test.** Create `crates/rusty-brain/tests/apm_manifest.rs` with this exact content:

```rust
//! Fixture test: the committed APM package manifest (`apm/apm.yml`) must declare
//! the stdio `rusty-brain` MCP server and contain NO literal secret. This is the
//! string-level guard for Part A; Part B replaces the structural checks with a
//! typed `serde_yaml_ng` parse once that dependency exists. Run from the workspace
//! root so the manifest path resolves relative to the repo.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;

/// Resolve `<repo-root>/apm/apm.yml` from this crate's manifest dir.
/// `CARGO_MANIFEST_DIR` is `<root>/crates/rusty-brain`, so go up two levels.
fn manifest_path() -> PathBuf {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    crate_dir
        .parent()
        .and_then(|p| p.parent())
        .map(|root| root.join("apm").join("apm.yml"))
        .expect("repo root resolves from CARGO_MANIFEST_DIR")
}

#[test]
fn committed_manifest_exists_and_declares_stdio_mcp_entry() {
    let path = manifest_path();
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    // The stdio MCP entry, asserted at the string level (Part A).
    assert!(text.contains("transport: stdio"), "manifest must use stdio transport:\n{text}");
    assert!(text.contains("registry: false"), "manifest MCP entry must set registry: false:\n{text}");
    assert!(text.contains("command: rusty-brain"), "manifest must declare command: rusty-brain:\n{text}");
    assert!(text.contains(r#"["mcp"]"#) || text.contains("- mcp") || text.contains("- \"mcp\""),
        "manifest MCP args must invoke the `mcp` subcommand:\n{text}");
    assert!(text.contains("name: rusty-brain"), "manifest MCP entry must be named rusty-brain:\n{text}");
}

#[test]
fn committed_manifest_has_no_literal_secret() {
    let path = manifest_path();
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    // A real Voyage key starts `voy-`; OpenAI `sk-`. Env interpolation (${VAR})
    // is the ONLY way a token may appear. A bare `voy-…`/`sk-…` token is a leak.
    for needle in ["voy-", "sk-", "Bearer "] {
        assert!(
            !text.contains(needle),
            "manifest must not contain a literal secret token ({needle:?}); use env interpolation:\n{text}"
        );
    }
    // If VOYAGE_API_KEY is mentioned at all, it must be via interpolation, never `=`/`:` to a literal.
    if let Some(idx) = text.find("VOYAGE_API_KEY") {
        let rest = &text[idx..];
        assert!(
            rest.contains("${VOYAGE_API_KEY}") || rest.contains("$VOYAGE_API_KEY") || rest.contains("# "),
            "VOYAGE_API_KEY may appear only via env interpolation or a comment:\n{text}"
        );
    }
}
```

- [ ] **Step 2: run it.** Run: `cargo test -p rusty-brain --test apm_manifest` — Expected: FAIL — `apm/apm.yml` does not exist yet, so both tests panic in `read_to_string` (`No such file or directory`).

- [ ] **Step 3 GREEN: create the manifest.** Create `apm/apm.yml` with this exact content:

```yaml
# rusty-brain — Microsoft APM (Agent Package Manager) package manifest.
#
# `apm install` wires this into every detected harness (GitHub Copilot, Claude
# Code, Cursor, Codex, OpenCode, Gemini, Windsurf): it registers the stdio
# `rusty-brain` MCP server and installs the bundled memory instructions, skills,
# prompts, and the optional fail-open capture hook. `apm.lock.yaml` hash-pins the
# resolved tree.
#
# PREREQUISITE (out of band): the `rusty-brain` binary must be on PATH
# (cargo install / Homebrew / release artifact). APM wires AGENT CONTEXT — not
# binaries. `rusty-brain apm doctor` checks this prerequisite.
#
# SECURITY: no secret appears here. The Voyage embedding key lives in the
# environment / OS keychain and is read by the daemon, not the manifest. Any
# token would be referenced via env interpolation (${VOYAGE_API_KEY}) only.
# `rusty-brain apm validate apm/apm.yml` rejects any literal secret in CI.

name: rusty-brain
version: 0.1.0
description: Shared semantic memory for AI agents — one MCP server + memory skills, prompts, instructions, and a fail-open capture hook, wired across all harnesses.

dependencies:
  # MCP servers wired into every detected harness. rusty-brain is a LOCAL stdio
  # server: `apm` launches `rusty-brain mcp`, and the daemon auto-starts on the
  # first MCP connection (no remote endpoint, no secret, no network surface).
  mcp:
    - name: rusty-brain
      registry: false
      transport: stdio
      command: rusty-brain
      args: ["mcp"]

  # Bundled agent-context artifacts (instructions / skills / prompts / hooks).
  # APM requires the MAPPING shape here (a flat list is rejected). Each entry
  # references this package's git repo, a virtual subdirectory or file, and a
  # hash-pinnable `ref` (resolved + hashed into apm.lock.yaml).
  apm:
    rusty-brain-using-memory:
      git: brianluby/rusty-brain
      path: apm/instructions/using-memory.md
      ref: v0.1.0
    rusty-brain-memory-skill:
      git: brianluby/rusty-brain
      path: apm/skills/memory
      ref: v0.1.0
    rusty-brain-recall-context:
      git: brianluby/rusty-brain
      path: apm/prompts/recall-context.prompt.md
      ref: v0.1.0
    rusty-brain-capture-hook:
      git: brianluby/rusty-brain
      path: apm/hooks
      ref: v0.1.0
```

- [ ] **Step 4: run it.** Run: `cargo test -p rusty-brain --test apm_manifest` — Expected: PASS (2 tests: `committed_manifest_exists_and_declares_stdio_mcp_entry`, `committed_manifest_has_no_literal_secret`).

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rusty-brain --test apm_manifest -- -D warnings` (Expected: no warnings) then `cargo fmt --all` (Expected: no diff; YAML is untouched by rustfmt).

- [ ] **Step 6: commit.** Run: `git add apm/apm.yml crates/rusty-brain/tests/apm_manifest.rs && git commit -m "feat(apm): add committed apm.yml manifest with stdio rusty-brain MCP entry"` — Expected: one commit.

---

### Task A2: `apm/instructions/using-memory.md` — always-on context

Bundle the instructions artifact: when/why an agent uses shared memory. This is always-on harness context (no test gate beyond the existing fixture test, which we extend to assert the file exists).

**Files:**
- Create: apm/instructions/using-memory.md
- Modify: crates/rusty-brain/tests/apm_manifest.rs

**Test:** the fixture test asserts the bundled instructions file exists at the path the manifest references.

- [ ] **Step 1 RED: extend the fixture test.** Append this test to `crates/rusty-brain/tests/apm_manifest.rs` (inside the file, after the existing tests):

```rust
/// Resolve `<repo-root>/apm/<rel>` from this crate's manifest dir.
fn apm_path(rel: &str) -> PathBuf {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    crate_dir
        .parent()
        .and_then(|p| p.parent())
        .map(|root| root.join("apm").join(rel))
        .expect("repo root resolves from CARGO_MANIFEST_DIR")
}

#[test]
fn bundled_instructions_file_exists() {
    let path = apm_path("instructions/using-memory.md");
    assert!(path.is_file(), "manifest references {} which must exist", path.display());
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("remember"), "instructions must mention remember:\n{text}");
    assert!(text.contains("recall"), "instructions must mention recall:\n{text}");
}
```

- [ ] **Step 2: run it.** Run: `cargo test -p rusty-brain --test apm_manifest bundled_instructions_file_exists` — Expected: FAIL — the file does not exist (`assert!(path.is_file())` fails).

- [ ] **Step 3 GREEN: create the instructions file.** Create `apm/instructions/using-memory.md` with this exact content:

```markdown
# Using rusty-brain shared memory

You have access to **rusty-brain**, a shared semantic-memory service exposed as
an MCP server. It is project-scoped: every memory you store or recall belongs to
the current project's namespace (resolved from the git root), so teammates and
future sessions on the same project share the same memory. The daemon starts
automatically on first use.

## When to RECALL (read)

Before starting non-trivial work, recall what is already known:

- At the start of a session or a new task, call the recall tool with a short
  description of what you are about to do ("auth refactor", "flaky CI on macOS").
- When you hit a decision that smells previously-made ("why is this configured
  this way?"), recall before re-deriving it.
- Prefer recall over re-reading large files when the answer is a remembered
  decision, gotcha, or convention.

## When to REMEMBER (write)

Store a memory when something is worth carrying across sessions — not routine
chatter:

- A non-obvious **decision** and its rationale ("we pin tokio to X because Y").
- A **bug fix** and its root cause, so the same trap is not re-sprung.
- A **convention** the project follows that is not written down elsewhere.
- A **gotcha** that cost real time to discover.

Set an importance (1–10) honestly: a one-off note is low; an architectural
invariant is high. Add a short context string and tags so future recall is sharp.

## What NOT to remember

- Secrets, tokens, or credentials of any kind.
- Transient state (a value true only for this one run).
- Anything you would not want a teammate to read months from now.

Memory is shared and durable. Treat it like a project wiki, not a scratchpad.
```

- [ ] **Step 4: run it.** Run: `cargo test -p rusty-brain --test apm_manifest bundled_instructions_file_exists` — Expected: PASS.

- [ ] **Step 5: lint+format.** Run: `cargo fmt --all --check` (Expected: no diff) then `cargo clippy -p rusty-brain --test apm_manifest -- -D warnings` (Expected: no warnings).

- [ ] **Step 6: commit.** Run: `git add apm/instructions/using-memory.md crates/rusty-brain/tests/apm_manifest.rs && git commit -m "feat(apm): bundle using-memory instructions artifact"` — Expected: one commit.

---

### Task A3: `apm/skills/memory/` — the remember/recall skill

Bundle the memory skill (an APM `skills/` directory). A skill is a directory with a `SKILL.md` describing the remember/recall/context workflow.

**Files:**
- Create: apm/skills/memory/SKILL.md
- Modify: crates/rusty-brain/tests/apm_manifest.rs

**Test:** the fixture test asserts the skill directory + `SKILL.md` exist and name the rusty-brain tools.

- [ ] **Step 1 RED: extend the fixture test.** Append this test to `crates/rusty-brain/tests/apm_manifest.rs`:

```rust
#[test]
fn bundled_memory_skill_exists() {
    let dir = apm_path("skills/memory");
    assert!(dir.is_dir(), "skill dir {} must exist", dir.display());
    let skill = dir.join("SKILL.md");
    assert!(skill.is_file(), "{} must exist", skill.display());
    let text = std::fs::read_to_string(&skill).unwrap();
    // The skill must teach the three core memory operations.
    assert!(text.contains("remember"), "SKILL.md must describe remember:\n{text}");
    assert!(text.contains("recall"), "SKILL.md must describe recall:\n{text}");
    assert!(text.contains("context"), "SKILL.md must describe context:\n{text}");
}
```

- [ ] **Step 2: run it.** Run: `cargo test -p rusty-brain --test apm_manifest bundled_memory_skill_exists` — Expected: FAIL — the directory/file do not exist.

- [ ] **Step 3 GREEN: create the skill.** Create `apm/skills/memory/SKILL.md` with this exact content:

```markdown
---
name: memory
description: Store and retrieve durable project knowledge via rusty-brain shared memory. Use when a decision, bug fix, convention, or gotcha is worth carrying across sessions, or when starting a task that prior work may already inform.
---

# Memory skill (rusty-brain)

This skill drives the rusty-brain MCP tools to keep durable, shared project
knowledge. Memory is namespaced to the current project and shared across
sessions and teammates; the daemon auto-starts on first use.

## Workflow

1. **Recall first.** At the start of a task, call the recall tool with a short
   query describing the work. Skim the results before acting — a remembered
   decision or gotcha can save a full investigation.
2. **Work.** Do the task, noting anything non-obvious you discover.
3. **Remember selectively.** When you learn something durable — a decision and
   its rationale, a bug's root cause, a project convention, a costly gotcha —
   call the remember tool. Pick an honest importance (1–10), add a one-line
   context, and tag it.
4. **Load context on demand.** Use the context tool to pull the project's recent
   and most-important memories when you need a fast orientation.

## Operations

- **recall(query, [type], [tags], [limit])** — semantic search over project
  memory. Returns ranked notes.
- **remember(content, [type], [importance], [context], [tags])** — store a new
  memory. `type` is one of the memory types (e.g. `insight`, `bug_fix`,
  `decision`); `importance` is 1–10.
- **context()** — the project context payload: recent + most-important memories.

## Rules

- Never store secrets, tokens, or credentials.
- Don't remember transient state or routine chatter — only durable knowledge.
- Prefer recall over re-deriving a previously-made decision.
```

- [ ] **Step 4: run it.** Run: `cargo test -p rusty-brain --test apm_manifest bundled_memory_skill_exists` — Expected: PASS.

- [ ] **Step 5: lint+format.** Run: `cargo fmt --all --check` (Expected: no diff) then `cargo clippy -p rusty-brain --test apm_manifest -- -D warnings` (Expected: no warnings).

- [ ] **Step 6: commit.** Run: `git add apm/skills/memory/SKILL.md crates/rusty-brain/tests/apm_manifest.rs && git commit -m "feat(apm): bundle memory skill (remember/recall/context workflow)"` — Expected: one commit.

---

### Task A4: `apm/prompts/recall-context.prompt.md` — one-shot recall prompt

Bundle the prompt artifact (APM `.prompt.md` form): a one-shot "load project memory" prompt.

**Files:**
- Create: apm/prompts/recall-context.prompt.md
- Modify: crates/rusty-brain/tests/apm_manifest.rs

**Test:** the fixture test asserts the prompt file exists at the manifest-referenced path.

- [ ] **Step 1 RED: extend the fixture test.** Append this test to `crates/rusty-brain/tests/apm_manifest.rs`:

```rust
#[test]
fn bundled_recall_prompt_exists() {
    let path = apm_path("prompts/recall-context.prompt.md");
    assert!(path.is_file(), "{} must exist", path.display());
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("recall"), "prompt must invoke recall:\n{text}");
}
```

- [ ] **Step 2: run it.** Run: `cargo test -p rusty-brain --test apm_manifest bundled_recall_prompt_exists` — Expected: FAIL — file does not exist.

- [ ] **Step 3 GREEN: create the prompt.** Create `apm/prompts/recall-context.prompt.md` with this exact content:

```markdown
---
name: recall-context
description: Load the current project's shared memory before starting work.
---

Before we begin, orient yourself using rusty-brain shared memory for this
project:

1. Call the rusty-brain **context** tool to load the project's recent and
   most-important memories.
2. Call **recall** with a short query describing what we are about to work on:
   "{{task}}".
3. Summarize, in 3–5 bullets, the decisions, conventions, and gotchas that are
   relevant to "{{task}}". Cite the memory ids you used.

If memory is empty or nothing is relevant, say so plainly and proceed — do not
invent context.
```

- [ ] **Step 4: run it.** Run: `cargo test -p rusty-brain --test apm_manifest bundled_recall_prompt_exists` — Expected: PASS.

- [ ] **Step 5: lint+format.** Run: `cargo fmt --all --check` (Expected: no diff) then `cargo clippy -p rusty-brain --test apm_manifest -- -D warnings` (Expected: no warnings).

- [ ] **Step 6: commit.** Run: `git add apm/prompts/recall-context.prompt.md crates/rusty-brain/tests/apm_manifest.rs && git commit -m "feat(apm): bundle recall-context one-shot prompt"` — Expected: one commit.

---

### Task A5: `apm/hooks/` — optional fail-open capture hook descriptor

Bundle the optional capture-hook artifact. It documents and declares the fail-open `rusty-brain-hooks` capture hook (the P4 binary) so harnesses that support hooks can install it via APM. The hook is fail-open: it never blocks or breaks a session.

**Files:**
- Create: apm/hooks/capture.md
- Modify: crates/rusty-brain/tests/apm_manifest.rs

**Test:** the fixture test asserts the hooks artifact exists and documents fail-open behavior.

- [ ] **Step 1 RED: extend the fixture test.** Append this test to `crates/rusty-brain/tests/apm_manifest.rs`:

```rust
#[test]
fn bundled_capture_hook_exists_and_is_fail_open() {
    let path = apm_path("hooks/capture.md");
    assert!(path.is_file(), "{} must exist", path.display());
    let text = std::fs::read_to_string(&path).unwrap().to_lowercase();
    assert!(
        text.contains("fail-open") || text.contains("fail open"),
        "the bundled capture hook must document fail-open behavior:\n{text}"
    );
    assert!(text.contains("rusty-brain-hooks"), "hook must name the rusty-brain-hooks binary:\n{text}");
}
```

- [ ] **Step 2: run it.** Run: `cargo test -p rusty-brain --test apm_manifest bundled_capture_hook_exists_and_is_fail_open` — Expected: FAIL — file does not exist.

- [ ] **Step 3 GREEN: create the hook descriptor.** Create `apm/hooks/capture.md` with this exact content:

```markdown
---
name: capture
description: Optional fail-open capture hook that records agent activity into rusty-brain shared memory.
---

# Capture hook (rusty-brain)

This hook wires the `rusty-brain-hooks` binary into a harness's post-tool /
session events so notable agent activity is captured into shared memory
automatically. It is **optional** — the MCP server alone gives full
remember/recall — and **fail-open**: if the hook binary is missing, the daemon
is unreachable, or anything goes wrong, the hook returns success and the agent
session continues unblocked. A capture hook MUST NEVER block, slow, or break a
session.

## Prerequisite

The `rusty-brain-hooks` binary must be on PATH (installed alongside `rusty-brain`
via cargo / Homebrew / release artifact). If it is absent, this hook is inert and
sessions are unaffected.

## Behavior

- Reads the harness's hook JSON on stdin, normalizes it, and forwards a
  best-effort observation to the running daemon.
- On ANY error (parse failure, missing binary, daemon down, timeout) it exits
  successfully and emits no blocking output — fail-open by construction.
- Captures nothing sensitive: it records activity metadata, never secrets.

## Installation

`apm install` wires this hook into harnesses that support a JSON hooks block.
Harnesses without a JSON hooks surface (e.g. plugin-only CLIs) skip it; the MCP
server still provides full memory access there.
```

- [ ] **Step 4: run it.** Run: `cargo test -p rusty-brain --test apm_manifest bundled_capture_hook_exists_and_is_fail_open` — Expected: PASS.

- [ ] **Step 5: lint+format.** Run: `cargo fmt --all --check` (Expected: no diff) then `cargo clippy -p rusty-brain --test apm_manifest -- -D warnings` (Expected: no warnings).

- [ ] **Step 6: commit.** Run: `git add apm/hooks/capture.md crates/rusty-brain/tests/apm_manifest.rs && git commit -m "feat(apm): bundle optional fail-open capture hook descriptor"` — Expected: one commit.

---

### Task A6: Part A gate

**Files:**
- (none — verification only)

- [ ] **Step 1: workspace tests — Run:** `cargo test --workspace` — Expected: PASS, 0 failures (the new `apm_manifest` fixture test runs as part of the `rusty-brain` test target).

- [ ] **Step 2: clippy — Run:** `cargo clippy --workspace --all-targets --all-features -- -D warnings` — Expected: no warnings.

- [ ] **Step 3: format — Run:** `cargo fmt --all --check` — Expected: no diff.

- [ ] **Step 4: supply chain — Run:** `cargo deny check` — Expected: ok (`licenses ok`, `advisories ok`, `sources ok`; `bans` may `warn` only). Part A added no dep, so this is unchanged from main.

- [ ] **Step 5: commit (only if any formatting touch-up was needed) — Run:** `git add -A && git commit -m "chore(apm): part A gate green"` — Expected: a commit only if a fixup was needed; otherwise nothing to commit.

---

## Part B — `rusty-brain apm` CLI (emit / validate / doctor)

This Part adds the read-only `apm` subcommand group to the `rusty-brain` binary. `emit` prints the canonical stdio MCP descriptor for *this* binary (command/args + `ContractVersion`), so the published manifest is generated, not hand-drifted. `validate [path]` parses an `apm.yml` and rejects embedded secrets, a missing `ref`, and a malformed/absent MCP entry, exiting non-zero on a problem. `doctor` reports prerequisites (`rusty-brain` on PATH, `apm` present, harnesses detectable) read-only. This Part adds the one new dependency, `serde_yaml_ng` (the maintained fork of the unmaintained `serde_yaml`/RUSTSEC-2024-0320), used only by `validate` to parse the manifest.

HARD RULES honored throughout: `apm` commands NEVER mutate config, write the daemon, or spawn `apm install`; the descriptor mirrors the spec's stdio contract exactly; `validate` is the security checkpoint that rejects literal secrets; no `.unwrap()`/`.expect()`/`panic!` in non-test code.

---

### Task B1: `serde_yaml_ng` dependency

Add `serde_yaml_ng` to the workspace and to the `rusty-brain` crate. It is the maintained drop-in fork of `serde_yaml` (same API), chosen because the original `serde_yaml` is unmaintained (RUSTSEC-2024-0320) and would fail the repo's `cargo deny check` advisories gate. It is MIT/Apache-2.0 (deny-clean) and used only by `apm validate` to parse the manifest. The committed `apm.yml` is generated/authored, not parsed by `emit`, so `emit` needs no new dep.

**Files:**
- Modify: Cargo.toml (workspace `[workspace.dependencies]`)
- Modify: crates/rusty-brain/Cargo.toml (`[dependencies]`)

**Test:** a compile-level smoke test in the `apm` module references `serde_yaml_ng::Value` (added in Task B3); for this task the gate is `cargo build -p rusty-brain` succeeding with the new dep wired.

- [ ] **Step 1 RED: prove the dep is missing.** Run: `cargo tree -p rusty-brain 2>/dev/null | grep -i serde_yaml_ng; echo "exit=$?"` — Expected: `exit=1` (grep found nothing) — `serde_yaml_ng` is not yet a dependency.

- [ ] **Step 2 GREEN: add the workspace dependency.** Edit `Cargo.toml` (workspace root) and add this line to `[workspace.dependencies]`, immediately after the existing `toml = "0.8"` line:

```toml
serde_yaml_ng = "0.10"
```

- [ ] **Step 3 GREEN: wire it into `rusty-brain`.** Edit `crates/rusty-brain/Cargo.toml` and add this line to `[dependencies]`, immediately after the existing `serde_json = { workspace = true }` line:

```toml
serde_yaml_ng = { workspace = true }
```

- [ ] **Step 4: run it.** Run: `cargo build -p rusty-brain` (Expected: PASS — `serde_yaml_ng` resolves and the crate builds) then `cargo tree -p rusty-brain 2>/dev/null | grep -i serde_yaml_ng; echo "exit=$?"` (Expected: `exit=0` — `serde_yaml_ng v0.10.x` now appears).

- [ ] **Step 5: supply-chain check (Part adds a dep) — Run:** `cargo deny check 2>&1 | tail -20; echo "exit=${PIPESTATUS[0]}"` — Expected: `exit=0`, `licenses ok`, `advisories ok`, `sources ok` — `serde_yaml_ng` (and its `unsafe-libyaml`/`indexmap` tree) are MIT/Apache-2.0 with NO open advisory, already in the allow-list. (The maintained `serde_yaml_ng` is used precisely because the original `serde_yaml` is unmaintained — RUSTSEC-2024-0320 — and would FAIL this `cargo deny check advisories` gate.) If a NEW permissive SPDX id is flagged, add only that id to the `allow` array in `deny.toml` (never a copyleft license) and re-run; if a copyleft license appears, STOP and flag it.

- [ ] **Step 6: lint+format.** Run: `cargo fmt --all --check` (Expected: no diff — TOML-only) then `cargo clippy -p rusty-brain -- -D warnings` (Expected: no warnings).

- [ ] **Step 7: commit.** Run: `git add Cargo.toml crates/rusty-brain/Cargo.toml Cargo.lock && git commit -m "build(rusty-brain): add serde_yaml_ng for apm manifest validation"` — Expected: one commit (include `Cargo.lock` if the repo tracks it; if `deny.toml` changed, add it too with a separate `chore:` commit per the Part B gate).

---

### Task B2: `apm` module — the canonical MCP descriptor (`emit`)

Create the `apm` module with the descriptor type and the `emit` function that renders the canonical stdio MCP entry for this binary, carrying `rb_proto::CONTRACT_VERSION`. The descriptor is a serde struct so `emit` round-trips through `validate` (Task B4).

**Files:**
- Create: crates/rusty-brain/src/apm.rs
- Modify: crates/rusty-brain/src/lib.rs

**Test:** unit tests in `apm.rs` assert the emitted descriptor names `rusty-brain`, uses stdio/`registry: false`/`command: rusty-brain`/`args: ["mcp"]`, and embeds the live `CONTRACT_VERSION`.

- [ ] **Step 1 RED: write the failing test + skeleton.** Create `crates/rusty-brain/src/apm.rs` with this exact content (the impl is a stub that fails the tests until Step 3):

```rust
//! `rusty-brain apm` — read-only manifest emit / validate / doctor.
//!
//! These commands keep the published APM package in lockstep with this binary's
//! actual MCP surface and `ContractVersion`. They are STRICTLY read-only: they
//! never mutate any harness config, never write the daemon, and never spawn
//! `apm install` (mutation is APM's job, or `rb-install`'s fallback).

use rb_proto::CONTRACT_VERSION;
use serde::Serialize;

/// The canonical stdio MCP descriptor for THIS rusty-brain binary. Serialized by
/// `apm emit` so the published `apm.yml` MCP entry is generated, not drifted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct McpDescriptor {
    /// MCP server name. Stable: `rusty-brain`.
    pub name: String,
    /// Local stdio server, not a registry reference.
    pub registry: bool,
    /// Transport: always `stdio`.
    pub transport: String,
    /// The binary to launch.
    pub command: String,
    /// The subcommand args: `["mcp"]`.
    pub args: Vec<String>,
    /// The wire contract version this binary speaks (informational; surfaced so
    /// the package can detect binary/manifest drift).
    pub contract_version: u32,
}

/// Build the canonical descriptor for this binary.
#[must_use]
pub fn descriptor() -> McpDescriptor {
    // STUB — replaced in Step 3.
    McpDescriptor {
        name: String::new(),
        registry: true,
        transport: String::new(),
        command: String::new(),
        args: Vec::new(),
        contract_version: 0,
    }
}

/// Render the descriptor as a human-readable YAML-shaped MCP entry suitable for
/// pasting under `dependencies.mcp` in an `apm.yml`.
#[must_use]
pub fn emit() -> String {
    // STUB — replaced in Step 3.
    String::new()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn descriptor_matches_the_stdio_contract() {
        let d = descriptor();
        assert_eq!(d.name, "rusty-brain");
        assert!(!d.registry, "rusty-brain is a local stdio server, not a registry ref");
        assert_eq!(d.transport, "stdio");
        assert_eq!(d.command, "rusty-brain");
        assert_eq!(d.args, vec!["mcp".to_string()]);
        assert_eq!(d.contract_version, CONTRACT_VERSION);
    }

    #[test]
    fn emit_renders_the_stdio_entry() {
        let out = emit();
        assert!(out.contains("name: rusty-brain"), "{out}");
        assert!(out.contains("registry: false"), "{out}");
        assert!(out.contains("transport: stdio"), "{out}");
        assert!(out.contains("command: rusty-brain"), "{out}");
        assert!(out.contains("mcp"), "{out}");
        // The live contract version is surfaced as a comment, not a manifest field
        // (APM's MCP schema has no contract_version key), so it never breaks parse.
        assert!(out.contains(&format!("ContractVersion {CONTRACT_VERSION}")), "{out}");
    }
}
```

- [ ] **Step 2: run it.** Run: `cargo test -p rusty-brain --lib apm::tests` — Expected: FAIL — the stub `descriptor()`/`emit()` return empties, so every assertion fails (the module is not yet declared in `lib.rs`, so this first fails to compile with an unresolved-module error; the assertions fail once Step 3a wires the module).

- [ ] **Step 3a GREEN: wire the module.** Edit `crates/rusty-brain/src/lib.rs` to declare the `apm` module. Replace the module list so it reads:

```rust
pub mod apm;
pub mod cli;
pub mod client;
pub mod logging;
pub mod mcp;
pub mod namespace_detect;
pub mod output;
pub mod paths;
pub mod run;
pub mod serve;
```

- [ ] **Step 3b GREEN: implement `descriptor` and `emit`.** In `crates/rusty-brain/src/apm.rs`, replace the stub `descriptor()` and `emit()` bodies with:

```rust
/// Build the canonical descriptor for this binary.
#[must_use]
pub fn descriptor() -> McpDescriptor {
    McpDescriptor {
        name: "rusty-brain".to_string(),
        registry: false,
        transport: "stdio".to_string(),
        command: "rusty-brain".to_string(),
        args: vec!["mcp".to_string()],
        contract_version: CONTRACT_VERSION,
    }
}

/// Render the descriptor as a human-readable YAML-shaped MCP entry suitable for
/// pasting under `dependencies.mcp` in an `apm.yml`. The contract version is
/// emitted as a COMMENT (APM's MCP schema has no `contract_version` key), so the
/// output stays a valid manifest fragment while still surfacing drift.
#[must_use]
pub fn emit() -> String {
    let d = descriptor();
    let args = d
        .args
        .iter()
        .map(|a| format!("\"{a}\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "# rusty-brain MCP descriptor (ContractVersion {cv})\n\
         - name: {name}\n  \
           registry: {registry}\n  \
           transport: {transport}\n  \
           command: {command}\n  \
           args: [{args}]\n",
        cv = d.contract_version,
        name = d.name,
        registry = d.registry,
        transport = d.transport,
        command = d.command,
        args = args,
    )
}
```

- [ ] **Step 4: run it.** Run: `cargo test -p rusty-brain --lib apm::tests` — Expected: PASS (2 tests: `descriptor_matches_the_stdio_contract`, `emit_renders_the_stdio_entry`).

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rusty-brain --all-targets -- -D warnings` (Expected: no warnings) then `cargo fmt --all` (Expected: no diff).

- [ ] **Step 6: commit.** Run: `git add crates/rusty-brain/src/apm.rs crates/rusty-brain/src/lib.rs && git commit -m "feat(rusty-brain): add apm module with canonical MCP descriptor + emit"` — Expected: one commit.

---

### Task B3: `validate` — reject secrets, missing ref, malformed MCP entry

Add a `validate(yaml_text) -> Result<(), ValidationError>` that parses an `apm.yml` (via `serde_yaml_ng::Value`) and enforces the package's expectations: the stdio `rusty-brain` MCP entry must be present and well-formed, every `dependencies.apm` entry must carry a `ref`, and NO literal secret may appear.

**Files:**
- Modify: crates/rusty-brain/src/apm.rs

**Test:** unit tests assert a good manifest validates, and that secret / missing-ref / malformed-MCP / non-mapping-apm manifests are each rejected with a distinct error.

- [ ] **Step 1 RED: write the failing tests.** Append this to the `tests` module in `crates/rusty-brain/src/apm.rs` (inside the existing `#[cfg(test)] mod tests { ... }`):

```rust
    const GOOD: &str = r#"
name: rusty-brain
version: 0.1.0
dependencies:
  mcp:
    - name: rusty-brain
      registry: false
      transport: stdio
      command: rusty-brain
      args: ["mcp"]
  apm:
    rusty-brain-memory-skill:
      git: brianluby/rusty-brain
      path: apm/skills/memory
      ref: v0.1.0
"#;

    #[test]
    fn validate_accepts_a_good_manifest() {
        assert!(validate(GOOD).is_ok(), "the canonical manifest must validate");
    }

    #[test]
    fn validate_rejects_a_literal_secret() {
        let bad = GOOD.replace(
            "version: 0.1.0",
            "version: 0.1.0\nenv:\n  VOYAGE_API_KEY: voy-abcdef0123456789abcdef",
        );
        let err = validate(&bad).unwrap_err();
        assert!(matches!(err, ValidationError::EmbeddedSecret { .. }), "{err}");
    }

    #[test]
    fn validate_rejects_a_missing_ref() {
        let bad = GOOD.replace("      ref: v0.1.0\n", "");
        let err = validate(&bad).unwrap_err();
        assert!(matches!(err, ValidationError::MissingRef { .. }), "{err}");
    }

    #[test]
    fn validate_rejects_a_missing_mcp_entry() {
        let bad = r#"
name: rusty-brain
dependencies:
  apm:
    x:
      git: brianluby/rusty-brain
      path: apm/skills/memory
      ref: v0.1.0
"#;
        let err = validate(bad).unwrap_err();
        assert!(matches!(err, ValidationError::MissingMcpEntry), "{err}");
    }

    #[test]
    fn validate_rejects_a_malformed_mcp_entry() {
        // transport http with no stdio rusty-brain entry => malformed for our package.
        let bad = r#"
name: rusty-brain
dependencies:
  mcp:
    - name: rusty-brain
      registry: false
      transport: http
      url: http://example
"#;
        let err = validate(bad).unwrap_err();
        assert!(matches!(err, ValidationError::MalformedMcpEntry { .. }), "{err}");
    }

    #[test]
    fn validate_rejects_flat_apm_list() {
        // APM requires the apm dependencies to be a MAPPING; a flat list is invalid.
        let bad = r#"
name: rusty-brain
dependencies:
  mcp:
    - name: rusty-brain
      registry: false
      transport: stdio
      command: rusty-brain
      args: ["mcp"]
  apm:
    - brianluby/rusty-brain/skills/memory
"#;
        let err = validate(bad).unwrap_err();
        assert!(matches!(err, ValidationError::ApmNotMapping), "{err}");
    }

    #[test]
    fn validate_rejects_unparseable_yaml() {
        let err = validate(": this is not: valid: yaml: [").unwrap_err();
        assert!(matches!(err, ValidationError::Parse { .. }), "{err}");
    }
```

- [ ] **Step 2: run it.** Run: `cargo test -p rusty-brain --lib apm::tests::validate` — Expected: FAIL — `validate` and `ValidationError` do not exist yet (`error[E0425]`/`error[E0433]`).

- [ ] **Step 3a GREEN: add the error type + imports.** In `crates/rusty-brain/src/apm.rs`, add `use serde::Serialize;` (already present) and add this error enum just below the `McpDescriptor` definition:

```rust
/// A manifest-validation failure. Each variant maps to a distinct, stable
/// failure mode `apm validate` reports; the CLI maps any variant to a non-zero
/// exit.
#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    #[error("manifest is not valid YAML: {message}")]
    Parse { message: String },
    #[error("manifest declares no MCP server (expected a stdio `rusty-brain` entry under dependencies.mcp)")]
    MissingMcpEntry,
    #[error("the `rusty-brain` MCP entry is malformed: {reason}")]
    MalformedMcpEntry { reason: String },
    #[error("dependencies.apm must be a mapping (object), not a flat list")]
    ApmNotMapping,
    #[error("dependencies.apm entry `{alias}` is missing a `ref` (hash-pinning requires a ref)")]
    MissingRef { alias: String },
    #[error("manifest contains what looks like a literal secret at `{location}` (use env interpolation instead)")]
    EmbeddedSecret { location: String },
}
```

Add `thiserror` to `crates/rusty-brain/Cargo.toml` `[dependencies]` (it is a workspace dep already used elsewhere) — add the line `thiserror = { workspace = true }` after `serde_yaml_ng = { workspace = true }`.

- [ ] **Step 3b GREEN: implement `validate`.** Append these functions to `crates/rusty-brain/src/apm.rs` (module level, before the `tests` module):

```rust
/// Validate an `apm.yml` against the rusty-brain package's expectations.
///
/// Enforces: (1) a stdio `rusty-brain` MCP entry is present and well-formed;
/// (2) `dependencies.apm` is a mapping and every entry carries a `ref`; (3) no
/// literal secret appears anywhere. Returns the first failure found.
///
/// # Errors
/// Returns a [`ValidationError`] describing the first problem detected.
pub fn validate(yaml_text: &str) -> Result<(), ValidationError> {
    // (3) Secret scan first — a literal secret is the highest-severity failure.
    scan_for_secrets(yaml_text)?;

    let doc: serde_yaml_ng::Value = serde_yaml_ng::from_str(yaml_text)
        .map_err(|e| ValidationError::Parse { message: e.to_string() })?;

    let deps = doc.get("dependencies");

    // (1) stdio `rusty-brain` MCP entry.
    let mcp = deps
        .and_then(|d| d.get("mcp"))
        .and_then(serde_yaml_ng::Value::as_sequence);
    let rusty = mcp
        .into_iter()
        .flatten()
        .find(|e| e.get("name").and_then(serde_yaml_ng::Value::as_str) == Some("rusty-brain"));
    let Some(entry) = rusty else {
        return Err(ValidationError::MissingMcpEntry);
    };
    validate_mcp_entry(entry)?;

    // (2) dependencies.apm must be a mapping with a ref on each entry.
    if let Some(apm) = deps.and_then(|d| d.get("apm")) {
        let Some(map) = apm.as_mapping() else {
            return Err(ValidationError::ApmNotMapping);
        };
        for (alias, spec) in map {
            let alias = alias.as_str().unwrap_or("<non-string key>").to_string();
            let has_ref = spec
                .get("ref")
                .and_then(serde_yaml_ng::Value::as_str)
                .is_some_and(|s| !s.trim().is_empty());
            if !has_ref {
                return Err(ValidationError::MissingRef { alias });
            }
        }
    }

    Ok(())
}

/// The stdio `rusty-brain` entry must set `registry: false`, `transport: stdio`,
/// `command: rusty-brain`, and `args` containing `mcp`.
fn validate_mcp_entry(entry: &serde_yaml_ng::Value) -> Result<(), ValidationError> {
    let bad = |reason: &str| ValidationError::MalformedMcpEntry { reason: reason.to_string() };
    if entry.get("registry").and_then(serde_yaml_ng::Value::as_bool) != Some(false) {
        return Err(bad("registry must be false (rusty-brain is a local stdio server)"));
    }
    if entry.get("transport").and_then(serde_yaml_ng::Value::as_str) != Some("stdio") {
        return Err(bad("transport must be stdio"));
    }
    if entry.get("command").and_then(serde_yaml_ng::Value::as_str) != Some("rusty-brain") {
        return Err(bad("command must be rusty-brain"));
    }
    let args_ok = entry
        .get("args")
        .and_then(serde_yaml_ng::Value::as_sequence)
        .is_some_and(|a| a.iter().any(|v| v.as_str() == Some("mcp")));
    if !args_ok {
        return Err(bad("args must invoke the `mcp` subcommand"));
    }
    Ok(())
}

/// Reject anything that looks like a literal secret. Env interpolation
/// (`${VAR}` / `$VAR`) is always allowed; a bare token is not. Heuristics: a
/// `voy-`/`sk-` prefixed token, a `Bearer <token>` literal, or a value assigned
/// to a `*_API_KEY`/`*_TOKEN`/`*_SECRET` key that is NOT an interpolation.
fn scan_for_secrets(text: &str) -> Result<(), ValidationError> {
    for (lineno, line) in text.lines().enumerate() {
        let loc = || format!("line {}", lineno + 1);
        // Prefix-token heuristics: a real Voyage/OpenAI key.
        for prefix in ["voy-", "sk-", "Bearer "] {
            if let Some(pos) = line.find(prefix) {
                let tail = &line[pos + prefix.len()..];
                // A token: at least 12 of [A-Za-z0-9_-] right after the prefix.
                let token_len = tail
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                    .count();
                if token_len >= 12 {
                    return Err(ValidationError::EmbeddedSecret { location: loc() });
                }
            }
        }
        // Sensitive-key heuristic: `<KEY>: <value>` where value is a literal.
        if let Some((key, value)) = line.split_once(':') {
            let key_up = key.trim().to_ascii_uppercase();
            let sensitive = key_up.ends_with("_API_KEY")
                || key_up.ends_with("_TOKEN")
                || key_up.ends_with("_SECRET")
                || key_up.ends_with("_PASSWORD");
            if sensitive {
                let v = value.trim().trim_matches('"').trim_matches('\'');
                let is_interpolation = v.is_empty() || v.starts_with('$') || v.starts_with("${");
                let is_comment = v.starts_with('#');
                if !is_interpolation && !is_comment && v.len() >= 12 {
                    return Err(ValidationError::EmbeddedSecret { location: loc() });
                }
            }
        }
    }
    Ok(())
}
```

- [ ] **Step 4: run it.** Run: `cargo test -p rusty-brain --lib apm::tests` — Expected: PASS (all `validate_*` tests plus the earlier `descriptor`/`emit` tests).

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rusty-brain --all-targets -- -D warnings` (Expected: no warnings) then `cargo fmt --all` (Expected: no diff).

- [ ] **Step 6: commit.** Run: `git add crates/rusty-brain/src/apm.rs crates/rusty-brain/Cargo.toml && git commit -m "feat(rusty-brain): add apm validate rejecting secrets, missing ref, malformed mcp"` — Expected: one commit.

---

### Task B4: emit→validate round-trip + committed-manifest validation

Prove the two pillars hold together: `emit()`'s descriptor satisfies `validate` when embedded in a minimal manifest, and the committed `apm/apm.yml` validates clean.

**Files:**
- Modify: crates/rusty-brain/src/apm.rs

**Test:** a round-trip test wraps `emit()` into a manifest and asserts `validate` accepts it; a second test validates the real committed `apm/apm.yml`.

- [ ] **Step 1 RED: write the failing tests.** Append to the `tests` module in `crates/rusty-brain/src/apm.rs`:

```rust
    #[test]
    fn emit_round_trips_through_validate() {
        // Wrap the emitted MCP entry into a minimal manifest and validate it.
        // emit() yields a leading comment + a `- name: …` list item; nest it
        // under dependencies.mcp.
        let entry = emit();
        let indented = entry
            .lines()
            .map(|l| format!("    {l}"))
            .collect::<Vec<_>>()
            .join("\n");
        let manifest = format!("name: rusty-brain\ndependencies:\n  mcp:\n{indented}\n");
        assert!(
            validate(&manifest).is_ok(),
            "emit() output must satisfy validate(); manifest was:\n{manifest}"
        );
    }

    #[test]
    fn committed_manifest_validates() {
        let crate_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let path = crate_dir
            .parent()
            .and_then(|p| p.parent())
            .map(|root| root.join("apm").join("apm.yml"))
            .expect("repo root");
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
        assert!(
            validate(&text).is_ok(),
            "the committed apm/apm.yml must validate: {:?}",
            validate(&text)
        );
    }
```

- [ ] **Step 2: run it.** Run: `cargo test -p rusty-brain --lib apm::tests::emit_round_trips_through_validate apm::tests::committed_manifest_validates` — Expected: PASS (both). If `emit_round_trips_through_validate` fails, the indentation produced by `emit()` does not nest cleanly under `dependencies.mcp`; fix `emit()`'s rendering (the descriptor values are correct — only the leading-whitespace shape would need adjusting) until both pass. If `committed_manifest_validates` fails, the failure prints the `ValidationError`; correct `apm/apm.yml` to satisfy it (it should already validate from Part A).

- [ ] **Step 3: (no impl change expected).** These tests assert existing behavior. If Step 2 was already green, record "no impl change needed". Only adjust `emit()` rendering if Step 2 flagged a nesting problem.

- [ ] **Step 4: run it.** Run: `cargo test -p rusty-brain --lib apm::tests` — Expected: PASS (full `apm` test module green).

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rusty-brain --all-targets -- -D warnings` (Expected: no warnings) then `cargo fmt --all` (Expected: no diff).

- [ ] **Step 6: commit.** Run: `git add crates/rusty-brain/src/apm.rs && git commit -m "test(rusty-brain): assert emit/validate round-trip and committed manifest validates"` — Expected: one commit.

---

### Task B5: `doctor` — read-only prerequisites probe

Add `doctor() -> DoctorReport`: a read-only check of `rusty-brain` on PATH, `apm` presence, and which harnesses are detectable. It writes nothing and spawns no installer. Detection reuses an executable-bit PATH scan equivalent to P4's (re-implemented locally so the `rusty-brain` binary stays free of the `rb-install` dependency).

**Files:**
- Modify: crates/rusty-brain/src/apm.rs

**Test:** unit tests assert the report serializes, reports `rusty-brain` found when a fake `rusty-brain` is on PATH, and `apm` absent when it is not on the constructed PATH.

- [ ] **Step 1 RED: write the failing tests.** Append to the `tests` module in `crates/rusty-brain/src/apm.rs`:

```rust
    #[cfg(unix)]
    #[test]
    fn doctor_finds_binaries_on_a_constructed_path() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        // A fake `rusty-brain` + `apm` on PATH (executable); no `claude`/`apm` else.
        for name in ["rusty-brain", "apm"] {
            let p = dir.path().join(name);
            std::fs::write(&p, "#!/bin/sh\necho ok\n").unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        // bin_on_path scans a caller-provided PATH string (no process-global env
        // mutation) so this test is parallel-safe.
        assert!(bin_on_path("rusty-brain", dir.path().to_str().unwrap()).is_some());
        assert!(bin_on_path("apm", dir.path().to_str().unwrap()).is_some());
        assert!(bin_on_path("definitely-not-real-xyz", dir.path().to_str().unwrap()).is_none());

        let report = doctor_with_path(dir.path().to_str().unwrap());
        assert!(report.rusty_brain_on_path, "rusty-brain must be detected: {report:?}");
        assert!(report.apm_present, "apm must be detected: {report:?}");
        // Serializes to JSON for `--json`.
        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains("rusty_brain_on_path"), "{json}");
    }

    #[cfg(unix)]
    #[test]
    fn doctor_reports_apm_absent_when_not_on_path() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        // Only rusty-brain, no apm.
        let p = dir.path().join("rusty-brain");
        std::fs::write(&p, "#!/bin/sh\necho ok\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        let report = doctor_with_path(dir.path().to_str().unwrap());
        assert!(report.rusty_brain_on_path);
        assert!(!report.apm_present, "apm must be absent: {report:?}");
    }

    #[cfg(unix)]
    #[test]
    fn non_executable_binary_is_not_detected() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("rusty-brain");
        std::fs::write(&p, "#!/bin/sh\necho ok\n").unwrap();
        // 0o644 — present but NOT executable.
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(
            bin_on_path("rusty-brain", dir.path().to_str().unwrap()).is_none(),
            "a non-executable file must not count as installed"
        );
    }
```

- [ ] **Step 2: run it.** Run: `cargo test -p rusty-brain --lib apm::tests::doctor` — Expected: FAIL — `doctor_with_path`, `bin_on_path`, and `DoctorReport` do not exist yet.

- [ ] **Step 3 GREEN: implement the report + PATH scan.** Append to `crates/rusty-brain/src/apm.rs` (module level, before `tests`):

```rust
use std::path::{Path, PathBuf};

/// Read-only prerequisites report for `apm doctor`. Serializes for `--json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DoctorReport {
    /// `rusty-brain` resolves on PATH (an executable file).
    pub rusty_brain_on_path: bool,
    /// `apm` resolves on PATH (an executable file).
    pub apm_present: bool,
    /// Harness CLIs detected on PATH (the subset of known agent CLIs found).
    pub harnesses_detected: Vec<String>,
}

/// The harness CLI binary names APM would wire (probed read-only by `doctor`).
const KNOWN_HARNESS_BINARIES: &[&str] = &["claude", "copilot", "cursor", "codex", "gemini"];

/// Run the doctor probe against the process PATH.
#[must_use]
pub fn doctor() -> DoctorReport {
    let path = std::env::var("PATH").unwrap_or_default();
    doctor_with_path(&path)
}

/// Run the doctor probe against an explicit PATH string (testable; no env read).
#[must_use]
pub fn doctor_with_path(path_var: &str) -> DoctorReport {
    let harnesses_detected = KNOWN_HARNESS_BINARIES
        .iter()
        .filter(|name| bin_on_path(name, path_var).is_some())
        .map(|s| (*s).to_string())
        .collect();
    DoctorReport {
        rusty_brain_on_path: bin_on_path("rusty-brain", path_var).is_some(),
        apm_present: bin_on_path("apm", path_var).is_some(),
        harnesses_detected,
    }
}

/// Scan an explicit `PATH` string for an executable file named `name`. Mirrors
/// `rb_install::detect::find_binary_on_path` but takes the PATH explicitly (so it
/// is parallel-test-safe and the `rusty-brain` binary needs no `rb-install` dep).
/// On unix a candidate must be a regular file with at least one execute bit.
#[must_use]
pub fn bin_on_path(name: &str, path_var: &str) -> Option<PathBuf> {
    for dir in std::env::split_paths(path_var) {
        let candidate = dir.join(name);
        if is_executable_file(&candidate) {
            return Some(candidate);
        }
        #[cfg(target_os = "windows")]
        {
            for ext in ["exe", "cmd", "bat"] {
                let with_ext = dir.join(format!("{name}.{ext}"));
                if with_ext.is_file() {
                    return Some(with_ext);
                }
            }
        }
    }
    None
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    match std::fs::metadata(path) {
        Ok(meta) => meta.is_file() && (meta.permissions().mode() & 0o111 != 0),
        Err(_) => false,
    }
}

#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

/// Render a `DoctorReport` as a human-readable summary.
#[must_use]
pub fn render_doctor(report: &DoctorReport) -> String {
    let mark = |b: bool| if b { "[ok]" } else { "[--]" };
    let mut out = String::from("rusty-brain apm doctor\n");
    out.push_str(&format!("  {} rusty-brain on PATH\n", mark(report.rusty_brain_on_path)));
    out.push_str(&format!("  {} apm present\n", mark(report.apm_present)));
    if report.harnesses_detected.is_empty() {
        out.push_str("  [--] no harness CLIs detected on PATH\n");
    } else {
        out.push_str(&format!(
            "  [ok] harnesses detected: {}\n",
            report.harnesses_detected.join(", ")
        ));
    }
    if !report.rusty_brain_on_path {
        out.push_str(
            "\nNOTE: install the rusty-brain binary first (cargo install / Homebrew / release);\n\
             APM wires agent context, not the binary itself.\n",
        );
    }
    out
}
```

Add `tempfile` to `crates/rusty-brain/Cargo.toml` `[dev-dependencies]` if not present (it already is — confirm the line `tempfile = { workspace = true }` exists under `[dev-dependencies]`; it does in the current crate, so no change).

- [ ] **Step 4: run it.** Run: `cargo test -p rusty-brain --lib apm::tests` — Expected: PASS (doctor tests + all earlier `apm` tests).

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rusty-brain --all-targets -- -D warnings` (Expected: no warnings) then `cargo fmt --all` (Expected: no diff).

- [ ] **Step 6: commit.** Run: `git add crates/rusty-brain/src/apm.rs && git commit -m "feat(rusty-brain): add read-only apm doctor prerequisites probe"` — Expected: one commit.

---

### Task B6: clap `Apm` subcommand + `run` dispatch

Wire the `apm` subcommand group into the CLI (`emit`/`validate [path]`/`doctor`) and dispatch it in `run` BEFORE the daemon client connect (these commands never touch the daemon). `validate` exits non-zero on a problem; `emit`/`doctor` exit 0.

**Files:**
- Modify: crates/rusty-brain/src/cli.rs
- Modify: crates/rusty-brain/src/run.rs

**Test:** clap parse tests for `apm emit`/`apm validate <path>`/`apm doctor`; a `run`-level integration test that `apm validate` over a bad manifest returns an error (non-zero) and over a good manifest returns Ok.

- [ ] **Step 1 RED: write the failing clap tests.** Append to the `tests` module in `crates/rusty-brain/src/cli.rs`:

```rust
    #[test]
    fn parses_apm_emit() {
        let cli = Cli::parse_from(["rusty-brain", "apm", "emit"]);
        assert!(matches!(cli.command, Command::Apm { command: ApmCommand::Emit }));
    }

    #[test]
    fn parses_apm_validate_with_path() {
        let cli = Cli::parse_from(["rusty-brain", "apm", "validate", "apm/apm.yml"]);
        match cli.command {
            Command::Apm { command: ApmCommand::Validate { path } } => {
                assert_eq!(path.as_deref(), Some("apm/apm.yml"));
            }
            other => panic!("expected apm validate, got {other:?}"),
        }
    }

    #[test]
    fn parses_apm_validate_without_path() {
        let cli = Cli::parse_from(["rusty-brain", "apm", "validate"]);
        match cli.command {
            Command::Apm { command: ApmCommand::Validate { path } } => assert!(path.is_none()),
            other => panic!("expected apm validate, got {other:?}"),
        }
    }

    #[test]
    fn parses_apm_doctor() {
        let cli = Cli::parse_from(["rusty-brain", "apm", "doctor"]);
        assert!(matches!(cli.command, Command::Apm { command: ApmCommand::Doctor }));
    }
```

- [ ] **Step 2: run it.** Run: `cargo test -p rusty-brain --lib cli::tests::parses_apm` — Expected: FAIL — `Command::Apm` and `ApmCommand` do not exist (`error[E0599]`/unresolved).

- [ ] **Step 3a GREEN: add the subcommand to the clap enum.** In `crates/rusty-brain/src/cli.rs`, add this variant to the `Command` enum (after the `Evolve { … }` variant):

```rust
    /// APM (Agent Package Manager) package helpers — read-only.
    Apm {
        #[command(subcommand)]
        command: ApmCommand,
    },
```

Then add this new enum just below the `Command` enum definition:

```rust
/// Read-only `rusty-brain apm` subcommands. None mutate config or the daemon.
#[derive(Subcommand, Debug)]
pub enum ApmCommand {
    /// Print the canonical stdio MCP descriptor for this binary (for apm.yml).
    Emit,
    /// Validate an apm.yml (default `apm/apm.yml`): stdio entry, refs, no secrets.
    /// Exits non-zero on a problem.
    Validate {
        /// Path to the manifest (default: `apm/apm.yml` in the current dir).
        path: Option<String>,
    },
    /// Report prerequisites: rusty-brain on PATH, apm present, harnesses detected.
    Doctor,
}
```

- [ ] **Step 3b GREEN: write the failing dispatch test.** Append to the `tests` module in `crates/rusty-brain/src/run.rs`:

```rust
    #[tokio::test]
    async fn apm_validate_good_manifest_is_ok_and_touches_no_daemon() {
        use crate::cli::{ApmCommand, Cli, Command};
        // A valid manifest written to a temp file. `apm validate` must NOT connect
        // to a daemon: we pass socket/db paths that would fail if dialed.
        let dir = tempfile::tempdir().unwrap();
        let manifest = dir.path().join("apm.yml");
        std::fs::write(
            &manifest,
            "name: rusty-brain\ndependencies:\n  mcp:\n    - name: rusty-brain\n      registry: false\n      transport: stdio\n      command: rusty-brain\n      args: [\"mcp\"]\n",
        )
        .unwrap();
        let cli = Cli {
            json: false,
            command: Command::Apm {
                command: ApmCommand::Validate { path: Some(manifest.display().to_string()) },
            },
        };
        // Point socket/db at non-existent paths; if apm dispatch tried to connect
        // it would error. A clean Ok proves it never dialed the daemon.
        std::env::set_var(crate::paths::SOCKET_ENV, dir.path().join("nope.sock"));
        std::env::set_var(crate::paths::DB_ENV, dir.path().join("nope.db"));
        let ns = rb_types::Namespace::Global;
        let result = run(cli, ns).await;
        std::env::remove_var(crate::paths::SOCKET_ENV);
        std::env::remove_var(crate::paths::DB_ENV);
        assert!(result.is_ok(), "apm validate over a good manifest must succeed: {result:?}");
    }

    #[tokio::test]
    async fn apm_validate_bad_manifest_errors_nonzero() {
        use crate::cli::{ApmCommand, Cli, Command};
        let dir = tempfile::tempdir().unwrap();
        let manifest = dir.path().join("apm.yml");
        // No MCP entry => validation failure => Err (mapped to a non-zero exit).
        std::fs::write(&manifest, "name: rusty-brain\ndependencies:\n  apm: {}\n").unwrap();
        let cli = Cli {
            json: false,
            command: Command::Apm {
                command: ApmCommand::Validate { path: Some(manifest.display().to_string()) },
            },
        };
        let ns = rb_types::Namespace::Global;
        let result = run(cli, ns).await;
        assert!(result.is_err(), "apm validate over a manifest with no MCP entry must error");
    }
```

- [ ] **Step 3c GREEN: dispatch `Apm` in `run` before the client connect.** In `crates/rusty-brain/src/run.rs`, add an `Apm` arm to the top-level `match cli.command` in `run`, BEFORE the catch-all `other => run_client(...)` arm (so `apm` never reaches `connect_or_start`):

```rust
        Command::Apm { command } => run_apm(command, cli.json),
```

Then add this function to `crates/rusty-brain/src/run.rs` (after `run`):

```rust
/// Dispatch a read-only `apm` subcommand. NEVER connects to the daemon and
/// NEVER mutates any config. `validate` returns `Err` (non-zero exit) on a
/// problem; `emit`/`doctor` print and return `Ok`.
fn run_apm(command: crate::cli::ApmCommand, json: bool) -> anyhow::Result<()> {
    use crate::apm;
    use crate::cli::ApmCommand;
    match command {
        ApmCommand::Emit => {
            println!("{}", apm::emit());
            Ok(())
        }
        ApmCommand::Validate { path } => {
            let path = path.unwrap_or_else(|| "apm/apm.yml".to_string());
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("reading manifest {path}"))?;
            match apm::validate(&text) {
                Ok(()) => {
                    if json {
                        println!("{{\"manifest\":{},\"valid\":true}}", json_str(&path));
                    } else {
                        println!("ok: {path} is a valid rusty-brain apm manifest");
                    }
                    Ok(())
                }
                // A validation failure is a real error the user must see: surface
                // it with a non-zero exit (anyhow::Error -> ExitCode::FAILURE).
                Err(e) => Err(anyhow::Error::new(e)).with_context(|| format!("validating {path}")),
            }
        }
        ApmCommand::Doctor => {
            let report = apm::doctor();
            if json {
                println!(
                    "{}",
                    serde_json::to_string(&report).unwrap_or_else(|_| "{}".to_string())
                );
            } else {
                print!("{}", apm::render_doctor(&report));
            }
            Ok(())
        }
    }
}

/// Encode `s` as a JSON string literal without `unwrap`.
fn json_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}
```

Also add the `Apm` arm to the `run_client` match's exhaustiveness guard: in `run_client`, the `match command` already has explicit arms; add this arm alongside the existing internal-guard arms (`Serve`/`Mcp`) so the enum stays exhaustive:

```rust
        Command::Apm { .. } => anyhow::bail!("internal: apm must be handled before run_client"),
```

- [ ] **Step 4: run it.** Run: `cargo test -p rusty-brain --lib cli::tests::parses_apm run::tests::apm_validate` — Expected: PASS (4 clap tests + 2 dispatch tests).

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rusty-brain --all-targets -- -D warnings` (Expected: no warnings) then `cargo fmt --all` (Expected: no diff).

- [ ] **Step 6: commit.** Run: `git add crates/rusty-brain/src/cli.rs crates/rusty-brain/src/run.rs && git commit -m "feat(rusty-brain): wire apm emit/validate/doctor subcommands (read-only)"` — Expected: one commit.

---

### Task B7: CLI integration test — `apm` end to end through the built binary

Prove the assembled binary behaves: `apm emit` prints the stdio entry, `apm validate apm/apm.yml` succeeds (exit 0), `apm validate <bad>` exits non-zero, and `apm doctor` runs read-only.

**Files:**
- Modify: crates/rusty-brain/tests/apm_manifest.rs (add CLI-driven cases)

**Test:** `assert_cmd`-driven cases against the built `rusty-brain` binary.

- [ ] **Step 1 RED: write the failing CLI tests.** Append to `crates/rusty-brain/tests/apm_manifest.rs`:

```rust
#[cfg(test)]
mod cli_integration {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use assert_cmd::Command;
    use predicates::str::contains;

    fn bin() -> Command {
        Command::cargo_bin("rusty-brain").unwrap()
    }

    fn repo_root() -> std::path::PathBuf {
        let crate_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        crate_dir.parent().and_then(|p| p.parent()).unwrap().to_path_buf()
    }

    #[test]
    fn apm_emit_prints_stdio_entry() {
        bin()
            .args(["apm", "emit"])
            .assert()
            .success()
            .stdout(contains("name: rusty-brain"))
            .stdout(contains("transport: stdio"))
            .stdout(contains("command: rusty-brain"));
    }

    #[test]
    fn apm_validate_committed_manifest_succeeds() {
        // Run from the repo root so the default `apm/apm.yml` path resolves.
        bin()
            .current_dir(repo_root())
            .args(["apm", "validate", "apm/apm.yml"])
            .assert()
            .success()
            .stdout(contains("valid"));
    }

    #[test]
    fn apm_validate_bad_manifest_exits_nonzero() {
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("apm.yml");
        // A literal secret => validation failure => non-zero exit.
        std::fs::write(
            &bad,
            "name: rusty-brain\nenv:\n  VOYAGE_API_KEY: voy-abcdef0123456789\ndependencies:\n  mcp:\n    - name: rusty-brain\n      registry: false\n      transport: stdio\n      command: rusty-brain\n      args: [\"mcp\"]\n",
        )
        .unwrap();
        bin()
            .args(["apm", "validate", bad.to_str().unwrap()])
            .assert()
            .failure();
    }

    #[test]
    fn apm_doctor_runs_read_only() {
        bin().args(["apm", "doctor"]).assert().success().stdout(contains("doctor"));
    }
}
```

- [ ] **Step 2: run it.** Run: `cargo test -p rusty-brain --test apm_manifest cli_integration` — Expected: FAIL initially only if the binary is stale; since Task B6 implemented the dispatch, these should PASS after a rebuild. If `apm_validate_committed_manifest_succeeds` fails, run `rusty-brain apm validate apm/apm.yml` manually from the repo root to read the error and fix the committed manifest.

- [ ] **Step 3: (no impl change expected).** These exercise Task B6's wiring through the real binary. If Step 2 is green, record "no impl change needed".

- [ ] **Step 4: run it.** Run: `cargo test -p rusty-brain --test apm_manifest` — Expected: PASS (fixture tests + CLI integration).

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rusty-brain --all-targets -- -D warnings` (Expected: no warnings) then `cargo fmt --all` (Expected: no diff).

- [ ] **Step 6: commit.** Run: `git add crates/rusty-brain/tests/apm_manifest.rs && git commit -m "test(rusty-brain): apm emit/validate/doctor through the built binary"` — Expected: one commit.

---

### Task B8: Part B gate

**Files:**
- (none — verification only)

- [ ] **Step 1: workspace tests — Run:** `cargo test --workspace` — Expected: PASS, 0 failures.

- [ ] **Step 2: clippy — Run:** `cargo clippy --workspace --all-targets --all-features -- -D warnings` — Expected: no warnings.

- [ ] **Step 3: format — Run:** `cargo fmt --all --check` — Expected: no diff.

- [ ] **Step 4: supply chain (Part B added `serde_yaml_ng`) — Run:** `cargo deny check 2>&1 | tail -20; echo "exit=${PIPESTATUS[0]}"` — Expected: `exit=0`, `licenses ok`, `advisories ok`, `sources ok` (`bans` may `warn` only). `serde_yaml_ng`/`unsafe-libyaml`/`indexmap` are MIT/Apache-2.0 with no advisory, already allow-listed. (`serde_yaml_ng` is the maintained fork; the original `serde_yaml` is unmaintained — RUSTSEC-2024-0320 — and would fail this advisories gate.)

- [ ] **Step 5: default-closure guard — Run:** `cargo tree -e no-dev -p rusty-brain | grep -E "rb-agents|rb-hooks|rb-install"; echo "exit=$?"` — Expected: `exit=1` (no match): the `apm` module pulled in NO agent crate; the default `rusty-brain` closure is unchanged.

- [ ] **Step 6: commit (only if a gate fixup was needed) — Run:** `git add -A && git commit -m "chore(rusty-brain): part B gate green"` — Expected: a commit only if a fixup was needed.

---

## Part C — `rb-install` APM-aware backend (delegation + P4 fallback + fail-open)

This Part makes `rb-install` prefer `apm install` when `apm` is on PATH, falling back to the existing P4 direct-config installer (`engine::run_install`) when it is not. Selection is by capability detection (reusing `detect::find_binary_on_path`'s executable-bit scan); delegation re-checks `apm` immediately before spawning (TOCTOU) and runs the subprocess with `env_clear()` then a minimal env; a non-zero `apm install` exit (or a missing `apm`) falls back. The whole path is fail-open: it never breaks a harness setup and never returns a non-zero process exit.

HARD RULES honored throughout: `env_clear()` then only `PATH`/`HOME`; PATH detection requires an exec bit; TOCTOU re-check before spawn; fail-open with fallback; the backend lives in `rb-install` (an isolated crate), never in the default `rusty-brain` closure.

---

### Task C1: `apm_backend` module — capability detection

Add an `apm_backend` module to `rb-install` whose `detect_apm() -> Option<PathBuf>` reuses `detect::find_binary_on_path("apm")`, plus a `Backend` enum (`Apm`/`Fallback`) chosen by detection.

**Files:**
- Create: crates/rb-install/src/apm_backend.rs
- Modify: crates/rb-install/src/lib.rs

**Test:** unit tests assert `detect_apm` returns `Some` when a fake `apm` is on PATH and `None` otherwise, and `select_backend` maps detection to the right `Backend`.

- [ ] **Step 1 RED: write the failing test + skeleton.** Create `crates/rb-install/src/apm_backend.rs` with this exact content:

```rust
//! APM-aware install backend: prefer delegating to `apm install` when `apm` is
//! on PATH (the standard, hash-pinned, multi-harness tool), and fall back to the
//! P4 per-CLI direct-config installer otherwise. Fail-open end to end: a
//! detection or delegation failure logs and falls back — it never breaks a
//! harness setup and never returns a non-zero process exit.

use std::path::PathBuf;

use crate::detect::find_binary_on_path;

/// Which install path will run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Backend {
    /// `apm` is available; delegate to `apm install`. Carries the resolved path.
    Apm(PathBuf),
    /// `apm` is absent; use the P4 per-CLI direct-config installer.
    Fallback,
}

/// Detect `apm` on PATH (an executable file). Reuses the P4 exec-bit scan.
#[must_use]
pub fn detect_apm() -> Option<PathBuf> {
    find_binary_on_path("apm")
}

/// Choose the backend from detection: `apm` present => delegate, else fall back.
#[must_use]
pub fn select_backend() -> Backend {
    match detect_apm() {
        Some(path) => Backend::Apm(path),
        None => Backend::Fallback,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt as _;
    #[cfg(unix)]
    use std::sync::Mutex;

    // Serialize the PATH-mutating tests (mirrors rb-install/detect.rs).
    #[cfg(unix)]
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[cfg(unix)]
    fn with_fake_apm_on_path(f: impl FnOnce()) {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("apm");
        std::fs::write(&bin, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let old = std::env::var_os("PATH");
        let mut paths = vec![dir.path().to_path_buf()];
        if let Some(ref p) = old {
            paths.extend(std::env::split_paths(p));
        }
        std::env::set_var("PATH", std::env::join_paths(paths).unwrap());
        f();
        match old {
            Some(p) => std::env::set_var("PATH", p),
            None => std::env::remove_var("PATH"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn detect_apm_finds_fake_on_path() {
        with_fake_apm_on_path(|| {
            assert!(detect_apm().is_some(), "a fake executable `apm` must be detected on PATH");
            assert!(matches!(select_backend(), Backend::Apm(_)));
        });
    }

    #[cfg(unix)]
    #[test]
    fn select_backend_falls_back_when_apm_absent() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // An empty PATH guarantees `apm` is not found.
        let old = std::env::var_os("PATH");
        std::env::set_var("PATH", "");
        let backend = select_backend();
        match old {
            Some(p) => std::env::set_var("PATH", p),
            None => std::env::remove_var("PATH"),
        }
        assert_eq!(backend, Backend::Fallback, "no apm on PATH must fall back to the P4 installer");
    }
}
```

- [ ] **Step 2: run it.** Run: `cargo test -p rb-install --lib apm_backend::tests` — Expected: FAIL — the module is not declared in `lib.rs`, so it does not compile (unresolved).

- [ ] **Step 3 GREEN: declare the module + re-export.** Edit `crates/rb-install/src/lib.rs`. Add `pub mod apm_backend;` to the module list (after `pub mod engine;`), and add this to the `pub use` block:

```rust
pub use apm_backend::{detect_apm, select_backend, Backend};
```

- [ ] **Step 4: run it.** Run: `cargo test -p rb-install --lib apm_backend::tests` — Expected: PASS (2 unix tests).

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rb-install --all-targets -- -D warnings` (Expected: no warnings) then `cargo fmt --all` (Expected: no diff).

- [ ] **Step 6: commit.** Run: `git add crates/rb-install/src/apm_backend.rs crates/rb-install/src/lib.rs && git commit -m "feat(rb-install): add apm capability detection + backend selection"` — Expected: one commit.

---

### Task C2: hardened `apm install` delegation (env_clear + TOCTOU)

Add `delegate_install(apm_path, scope, dry_run) -> ApmDelegation`: a hardened subprocess spawn of `apm install`. It re-checks `apm` immediately before spawning (TOCTOU), runs with `env_clear()` then a minimal env (`PATH`, `HOME`/`USERPROFILE`), sets the working directory to the project scope, and reports success/failure. A non-zero exit or spawn error is reported (not panicked) so the caller can fall back.

**Files:**
- Modify: crates/rb-install/src/apm_backend.rs

**Test:** unit tests with a fake `apm` stub that records its argv to a file (asserting `install` was the arg) and a stub that exits non-zero (asserting the delegation reports failure); a dry-run test asserts no spawn.

- [ ] **Step 1 RED: write the failing tests.** Append to the `tests` module in `crates/rb-install/src/apm_backend.rs`:

```rust
    #[cfg(unix)]
    #[test]
    fn delegate_install_spawns_apm_with_install_arg() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let argv_log = dir.path().join("argv.log");
        // A fake apm that records its args (so we can assert `install` was passed)
        // and exits 0.
        let apm = dir.path().join("apm");
        std::fs::write(
            &apm,
            format!("#!/bin/sh\necho \"$@\" > '{}'\nexit 0\n", argv_log.display()),
        )
        .unwrap();
        std::fs::set_permissions(&apm, std::fs::Permissions::from_mode(0o755)).unwrap();

        let scope = rb_agents::install::InstallScope::Project(dir.path().to_path_buf());
        let result = delegate_install(&apm, &scope, false);
        assert!(result.spawned, "delegation must spawn apm: {result:?}");
        assert!(result.success, "the fake apm exited 0, so delegation must succeed: {result:?}");
        let logged = std::fs::read_to_string(&argv_log).unwrap();
        assert!(logged.contains("install"), "apm must be invoked with `install`; got: {logged:?}");
    }

    #[cfg(unix)]
    #[test]
    fn delegate_install_reports_failure_on_nonzero_exit() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let apm = dir.path().join("apm");
        // A fake apm that fails.
        std::fs::write(&apm, "#!/bin/sh\nexit 7\n").unwrap();
        std::fs::set_permissions(&apm, std::fs::Permissions::from_mode(0o755)).unwrap();
        let scope = rb_agents::install::InstallScope::Project(dir.path().to_path_buf());
        let result = delegate_install(&apm, &scope, false);
        assert!(result.spawned, "it did spawn: {result:?}");
        assert!(!result.success, "a non-zero apm exit must report failure (caller falls back): {result:?}");
    }

    #[cfg(unix)]
    #[test]
    fn delegate_install_dry_run_does_not_spawn() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let argv_log = dir.path().join("argv.log");
        let apm = dir.path().join("apm");
        std::fs::write(
            &apm,
            format!("#!/bin/sh\necho ran > '{}'\nexit 0\n", argv_log.display()),
        )
        .unwrap();
        std::fs::set_permissions(&apm, std::fs::Permissions::from_mode(0o755)).unwrap();
        let scope = rb_agents::install::InstallScope::Project(dir.path().to_path_buf());
        let result = delegate_install(&apm, &scope, true);
        assert!(!result.spawned, "a dry-run must not spawn apm: {result:?}");
        assert!(!argv_log.exists(), "dry-run must not have run apm");
    }

    #[cfg(unix)]
    #[test]
    fn delegate_install_reports_failure_when_apm_vanished_before_spawn() {
        // TOCTOU: pass a path that does not exist (apm vanished after detection).
        let dir = tempfile::tempdir().unwrap();
        let gone = dir.path().join("apm-gone");
        let scope = rb_agents::install::InstallScope::Project(dir.path().to_path_buf());
        let result = delegate_install(&gone, &scope, false);
        assert!(!result.spawned, "a vanished apm must not be spawned: {result:?}");
        assert!(!result.success, "a vanished apm must report failure so the caller falls back");
    }
```

- [ ] **Step 2: run it.** Run: `cargo test -p rb-install --lib apm_backend::tests::delegate` — Expected: FAIL — `delegate_install` and `ApmDelegation` do not exist yet.

- [ ] **Step 3 GREEN: implement the delegation.** Append to `crates/rb-install/src/apm_backend.rs` (module level, before `tests`):

```rust
use std::path::Path;
use std::process::Command;

use rb_agents::install::InstallScope;

/// The outcome of attempting to delegate to `apm install`. Fail-open friendly:
/// the caller falls back to the P4 installer when `success` is false.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApmDelegation {
    /// Whether the `apm` subprocess was actually spawned (false for dry-run or a
    /// vanished binary).
    pub spawned: bool,
    /// Whether `apm install` exited 0.
    pub success: bool,
    /// A short human note for the report (error string or "dry-run").
    pub note: Option<String>,
}

/// Delegate to `apm install`, hardened per the global security rules.
///
/// - **TOCTOU:** re-checks `apm_path` is an executable file IMMEDIATELY before
///   spawning, not just at selection time.
/// - **Env hygiene:** `env_clear()` then sets ONLY `PATH` and `HOME`/`USERPROFILE`
///   — the parent environment is never inherited wholesale.
/// - **Scope:** runs in the project directory (so `apm` finds the project's
///   `apm.yml`); Global scope uses the current directory.
///
/// Returns a report; it NEVER panics and NEVER propagates an error — a failure
/// is encoded in `success: false` so the caller falls back fail-open.
#[must_use]
pub fn delegate_install(apm_path: &Path, scope: &InstallScope, dry_run: bool) -> ApmDelegation {
    if dry_run {
        return ApmDelegation { spawned: false, success: false, note: Some("dry-run".to_string()) };
    }
    // TOCTOU re-check: the binary must still be an executable file right now.
    if find_binary_on_path("apm").is_none() && !is_executable_now(apm_path) {
        return ApmDelegation {
            spawned: false,
            success: false,
            note: Some("apm not present at spawn time".to_string()),
        };
    }
    let cwd = match scope {
        InstallScope::Project(p) => p.clone(),
        InstallScope::Global => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    };

    let mut cmd = Command::new(apm_path);
    cmd.arg("install");
    cmd.current_dir(&cwd);
    // Subprocess hardening: never inherit the parent env wholesale.
    cmd.env_clear();
    if let Some(path) = std::env::var_os("PATH") {
        cmd.env("PATH", path);
    }
    if let Some(home) = std::env::var_os("HOME") {
        cmd.env("HOME", home);
    }
    if let Some(home) = std::env::var_os("USERPROFILE") {
        cmd.env("USERPROFILE", home);
    }

    match cmd.status() {
        Ok(status) => ApmDelegation {
            spawned: true,
            success: status.success(),
            note: status.success().then(|| "apm install ok".to_string()).or_else(|| {
                Some(format!("apm install exited with {status}"))
            }),
        },
        Err(e) => ApmDelegation {
            spawned: false,
            success: false,
            note: Some(format!("failed to spawn apm: {e}")),
        },
    }
}

/// True if `path` is, right now, an executable regular file (TOCTOU re-check).
#[cfg(unix)]
fn is_executable_now(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    match std::fs::metadata(path) {
        Ok(meta) => meta.is_file() && (meta.permissions().mode() & 0o111 != 0),
        Err(_) => false,
    }
}

#[cfg(not(unix))]
fn is_executable_now(path: &Path) -> bool {
    path.is_file()
}
```

- [ ] **Step 4: run it.** Run: `cargo test -p rb-install --lib apm_backend::tests` — Expected: PASS (detection + delegation tests).

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rb-install --all-targets -- -D warnings` (Expected: no warnings) then `cargo fmt --all` (Expected: no diff).

- [ ] **Step 6: commit.** Run: `git add crates/rb-install/src/apm_backend.rs && git commit -m "feat(rb-install): add hardened apm install delegation (env_clear + toctou)"` — Expected: one commit.

---

### Task C3: `install_with_apm_awareness` — delegate-then-fallback orchestrator

Add the orchestrator `install_with_apm_awareness(scope, dry_run) -> InstallReport`: select the backend; if `Apm`, delegate; on delegation success return an `apm`-shaped report; on delegation failure (or `Fallback`), run the P4 `engine::run_install`. Fail-open: any path yields an `InstallReport`, never an error.

**Files:**
- Modify: crates/rb-install/src/apm_backend.rs

**Test:** with a fake `apm` exiting 0, the report shows the apm delegation succeeded; with a fake `apm` exiting non-zero, the report shows the fallback ran (no panic, report present); with no `apm`, the fallback runs.

- [ ] **Step 1 RED: write the failing tests.** Append to the `tests` module in `crates/rb-install/src/apm_backend.rs`:

```rust
    #[cfg(unix)]
    #[test]
    fn install_delegates_to_apm_when_present_and_succeeds() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let apm = dir.path().join("apm");
        std::fs::write(&apm, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&apm, std::fs::Permissions::from_mode(0o755)).unwrap();
        let old = std::env::var_os("PATH");
        let mut paths = vec![dir.path().to_path_buf()];
        if let Some(ref p) = old {
            paths.extend(std::env::split_paths(p));
        }
        std::env::set_var("PATH", std::env::join_paths(paths).unwrap());

        let scope = rb_agents::install::InstallScope::Project(dir.path().to_path_buf());
        let report = install_with_apm_awareness(&scope, false);

        match old {
            Some(p) => std::env::set_var("PATH", p),
            None => std::env::remove_var("PATH"),
        }
        // The apm-delegation report carries an "apm" pseudo-agent marked configured.
        assert!(
            report.agents.iter().any(|a| a.agent == "apm"
                && matches!(a.status, crate::report::AgentStatus::Configured)),
            "a successful apm delegation must report the `apm` agent as configured: {report:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn install_falls_back_when_apm_fails() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let apm = dir.path().join("apm");
        // apm exits non-zero => orchestrator must fall back to the P4 installer.
        std::fs::write(&apm, "#!/bin/sh\nexit 9\n").unwrap();
        std::fs::set_permissions(&apm, std::fs::Permissions::from_mode(0o755)).unwrap();
        let old = std::env::var_os("PATH");
        let mut paths = vec![dir.path().to_path_buf()];
        if let Some(ref p) = old {
            paths.extend(std::env::split_paths(p));
        }
        std::env::set_var("PATH", std::env::join_paths(paths).unwrap());

        let scope = rb_agents::install::InstallScope::Project(dir.path().to_path_buf());
        let report = install_with_apm_awareness(&scope, false);

        match old {
            Some(p) => std::env::set_var("PATH", p),
            None => std::env::remove_var("PATH"),
        }
        // A failed apm delegation must NOT mark `apm` configured; the fallback ran
        // (the P4 installer reports per-CLI agents — claude-code/gemini/codex —
        // which on CI are NotFound, so the report is still well-formed and non-panicking).
        assert!(
            !report.agents.iter().any(|a| a.agent == "apm"
                && matches!(a.status, crate::report::AgentStatus::Configured)),
            "a failed apm delegation must not be reported as a configured apm agent: {report:?}"
        );
        // The fallback installer's per-CLI agents are present in the report.
        assert!(
            report.agents.iter().any(|a| a.agent == "claude-code"),
            "the P4 fallback installer must have run: {report:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn install_falls_back_when_apm_absent() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let old = std::env::var_os("PATH");
        std::env::set_var("PATH", ""); // no apm
        let scope = rb_agents::install::InstallScope::Project(dir.path().to_path_buf());
        let report = install_with_apm_awareness(&scope, false);
        match old {
            Some(p) => std::env::set_var("PATH", p),
            None => std::env::remove_var("PATH"),
        }
        // With no apm, the P4 fallback runs; its per-CLI agents appear.
        assert!(
            report.agents.iter().any(|a| a.agent == "claude-code"),
            "no apm => the P4 fallback installer must run: {report:?}"
        );
    }
```

- [ ] **Step 2: run it.** Run: `cargo test -p rb-install --lib apm_backend::tests::install` — Expected: FAIL — `install_with_apm_awareness` does not exist yet.

- [ ] **Step 3 GREEN: implement the orchestrator.** Append to `crates/rb-install/src/apm_backend.rs` (module level, before `tests`):

```rust
use crate::engine::{resolve_hooks_bin, run_install, select_installers};
use crate::report::{AgentReport, AgentStatus, InstallReport};

/// Install rusty-brain agent context, APM-aware and fail-open.
///
/// 1. Detect `apm`. If present, delegate to `apm install` (hardened spawn).
/// 2. On delegation success, return an `apm`-shaped report.
/// 3. On delegation failure OR no `apm`, fall back to the P4 direct-config
///    installer ([`run_install`]) across all built-in CLIs.
///
/// Always returns an [`InstallReport`]; never errors and never panics. A
/// `dry_run` reports what WOULD happen without spawning `apm` or writing config.
#[must_use]
pub fn install_with_apm_awareness(scope: &InstallScope, dry_run: bool) -> InstallReport {
    let scope_label = match scope {
        InstallScope::Project(_) => "project",
        InstallScope::Global => "global",
    };
    if let Backend::Apm(apm_path) = select_backend() {
        if dry_run {
            // Report the delegation we WOULD perform; do not spawn or fall back.
            let agent = AgentReport {
                agent: "apm".to_string(),
                status: AgentStatus::WouldConfigure,
                config_path: None,
                version: crate::detect::version_of(&apm_path),
                error: None,
            };
            return InstallReport::roll_up(scope_label, true, vec![agent]);
        }
        let delegation = delegate_install(&apm_path, scope, false);
        if delegation.success {
            let agent = AgentReport {
                agent: "apm".to_string(),
                status: AgentStatus::Configured,
                config_path: None,
                version: crate::detect::version_of(&apm_path),
                error: None,
            };
            return InstallReport::roll_up(scope_label, false, vec![agent]);
        }
        // Delegation failed — log and fall through to the fail-open P4 fallback.
        tracing_note(&format!(
            "apm delegation failed ({}); falling back to the direct-config installer",
            delegation.note.unwrap_or_default()
        ));
    }
    // Fallback: the P4 per-CLI direct-config installer across all built-ins.
    let hooks_bin = resolve_hooks_bin();
    match select_installers(None) {
        Ok(installers) => run_install(&installers, &hooks_bin, scope, dry_run),
        // select_installers(None) cannot error, but stay fail-open: an empty
        // report is a neutral NoChanges, never a panic.
        Err(_) => InstallReport::roll_up(scope_label, dry_run, Vec::new()),
    }
}

/// Emit a best-effort note without taking a hard `tracing` dependency surface;
/// `rb-install` already depends on nothing for logging, so write to stderr.
fn tracing_note(msg: &str) {
    eprintln!("rusty-brain-install: {msg}");
}
```

- [ ] **Step 4: run it.** Run: `cargo test -p rb-install --lib apm_backend::tests` — Expected: PASS (all detection/delegation/orchestrator tests).

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rb-install --all-targets -- -D warnings` (Expected: no warnings) then `cargo fmt --all` (Expected: no diff).

- [ ] **Step 6: commit.** Run: `git add crates/rb-install/src/apm_backend.rs && git commit -m "feat(rb-install): add apm-aware install orchestrator with p4 fallback"` — Expected: one commit.

---

### Task C4: route `Install` through the APM-aware backend

Make the `rusty-brain-install install` command use `install_with_apm_awareness` instead of going straight to `run_install`, while `uninstall`/`status` keep the existing P4 behavior (APM owns un-wiring; the bespoke uninstall only touches OUR sentinel blocks). Add an `--no-apm` escape hatch to force the P4 path.

**Files:**
- Modify: crates/rb-install/src/cli.rs

**Test:** clap parse test for `install --no-apm`; an `execute`-level test asserting the install path produces a report (the fake-`apm` behavior is covered by C3's unit tests; here we assert wiring + the flag).

- [ ] **Step 1 RED: write the failing tests.** Append to the `tests` module in `crates/rb-install/src/cli.rs`:

```rust
    #[test]
    fn parses_install_with_no_apm_flag() {
        let cli = Cli::try_parse_from(["rusty-brain-install", "install", "--no-apm"]).unwrap();
        match cli.command {
            Command::Install { no_apm, .. } => assert!(no_apm, "--no-apm must force the P4 path"),
            _ => panic!("expected install"),
        }
    }

    #[test]
    fn install_execute_returns_a_report() {
        // In CI no `apm` and no agent CLIs are present, so this exercises the
        // orchestrator's fallback path end to end and must return a report (never
        // error). We use a temp cwd so the project scope is isolated.
        let dir = tempfile::tempdir().unwrap();
        let cli = Cli::try_parse_from([
            "rusty-brain-install",
            "--json",
            "install",
            "--no-apm",
            "--dry-run",
        ])
        .unwrap();
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir.path()).unwrap();
        let result = execute(&cli);
        std::env::set_current_dir(prev).unwrap();
        let (report, json) = result.expect("install execute must return a report");
        assert!(json, "--json forces json");
        assert!(report.dry_run, "a dry-run report");
    }
```

Add `use tempfile` to the test deps if missing — `tempfile` is already in `rb-install` `[dev-dependencies]`, so no Cargo change.

- [ ] **Step 2: run it.** Run: `cargo test -p rb-install --lib cli::tests::parses_install_with_no_apm_flag cli::tests::install_execute_returns_a_report` — Expected: FAIL — the `no_apm` field does not exist on `Command::Install`.

- [ ] **Step 3a GREEN: add the `--no-apm` flag.** In `crates/rb-install/src/cli.rs`, add a `no_apm` field to the `Install` variant of `Command`:

```rust
    /// Merge our sentinel-marked hook block into each CLI's config.
    Install {
        /// Restrict to these agents (claude-code, gemini, codex; opencode is
        /// deferred — it needs a JS/TS plugin).
        #[arg(long, value_delimiter = ',')]
        agents: Option<Vec<String>>,
        /// Install into the per-user (global) config instead of the project.
        #[arg(long)]
        global: bool,
        /// Compute and print the report without writing any file.
        #[arg(long)]
        dry_run: bool,
        /// Force the P4 direct-config installer even if `apm` is on PATH.
        #[arg(long)]
        no_apm: bool,
    },
```

- [ ] **Step 3b GREEN: route `Install` through the orchestrator.** In `crates/rb-install/src/cli.rs`, change the `Command::Install` arm of `execute` to use the APM-aware orchestrator unless `--no-apm` or specific `--agents` are requested (an explicit agent subset implies the bespoke per-CLI path, which APM does not express). Replace the existing `Command::Install { agents, global, dry_run } => { … }` arm with:

```rust
        Command::Install {
            agents,
            global,
            dry_run,
            no_apm,
        } => {
            // An explicit --agents subset, or --no-apm, forces the P4 path (APM
            // wires all harnesses at once and has no per-agent subset concept).
            if *no_apm || agents.is_some() {
                let installers = select_installers(agents.as_deref()).map_err(|e| e.to_string())?;
                run_install(&installers, &hooks_bin, &scope_for(*global), *dry_run)
            } else {
                crate::apm_backend::install_with_apm_awareness(&scope_for(*global), *dry_run)
            }
        }
```

- [ ] **Step 4: run it.** Run: `cargo test -p rb-install --lib cli::tests` — Expected: PASS (new tests + existing CLI tests). Also run `cargo test -p rb-install` to confirm the binary integration tests still pass.

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rb-install --all-targets -- -D warnings` (Expected: no warnings) then `cargo fmt --all` (Expected: no diff).

- [ ] **Step 6: commit.** Run: `git add crates/rb-install/src/cli.rs && git commit -m "feat(rb-install): route install through the apm-aware backend with --no-apm escape"` — Expected: one commit.

---

### Task C5: binary integration test — fake `apm` delegation + fallback + ignored smoke

Prove the assembled `rusty-brain-install` binary delegates to a fake `apm` (asserting the recorded argv) and falls back when `apm` is absent, plus an `#[ignore]` real-`apm` smoke test.

**Files:**
- Create: crates/rb-install/tests/apm.rs

**Test:** `assert_cmd`-driven cases with a fake `apm` stub on the child's PATH; an `#[ignore]` smoke test requiring a real `apm`.

- [ ] **Step 1 RED: write the failing integration test.** Create `crates/rb-install/tests/apm.rs` with this exact content:

```rust
//! Integration tests for the `rusty-brain-install` binary's APM-aware backend.
//! A FAKE `apm` stub on the child's PATH records its argv so we can assert
//! delegation; with no `apm` the binary falls back to the P4 installer. The real
//! `apm install` smoke test is `#[ignore]` (needs the apm binary + network).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use assert_cmd::Command;
use predicates::str::contains;

/// Write a fake `apm` into `dir` that records its argv to `argv.log` and exits
/// with `exit_code`. Returns the PATH string (fake dir prepended) for the child.
#[cfg(unix)]
fn fake_apm(dir: &std::path::Path, exit_code: i32) -> String {
    use std::os::unix::fs::PermissionsExt as _;
    let bin = dir.join("apm");
    let log = dir.join("argv.log");
    std::fs::write(
        &bin,
        format!("#!/bin/sh\necho \"$@\" >> '{}'\nexit {exit_code}\n", log.display()),
    )
    .unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let existing = std::env::var("PATH").unwrap_or_default();
    format!("{}:{}", dir.display(), existing)
}

#[cfg(unix)]
#[test]
fn install_delegates_to_fake_apm() {
    let apm_dir = tempfile::tempdir().unwrap();
    let proj = tempfile::tempdir().unwrap();
    let path = fake_apm(apm_dir.path(), 0);

    Command::cargo_bin("rusty-brain-install")
        .unwrap()
        .current_dir(proj.path())
        .env("PATH", &path)
        .args(["--json", "install"])
        .assert()
        .success()
        .stdout(contains("\"agent\": \"apm\"").or(contains("apm")));

    // The fake apm recorded `install` in its argv log: delegation truly happened.
    let log = std::fs::read_to_string(apm_dir.path().join("argv.log")).unwrap();
    assert!(log.contains("install"), "the binary must have invoked `apm install`; got: {log:?}");
}

#[cfg(unix)]
#[test]
fn install_falls_back_when_apm_absent() {
    let proj = tempfile::tempdir().unwrap();
    // A PATH with NO apm (and no agent CLIs): the binary must fall back and still
    // exit successfully (fail-open) — the report lists the per-CLI agents.
    Command::cargo_bin("rusty-brain-install")
        .unwrap()
        .current_dir(proj.path())
        .env("PATH", "/nonexistent-empty-path")
        .args(["--json", "install"])
        .assert()
        .success()
        .stdout(contains("claude-code"));
}

#[cfg(unix)]
#[test]
fn failing_apm_falls_back_fail_open() {
    let apm_dir = tempfile::tempdir().unwrap();
    let proj = tempfile::tempdir().unwrap();
    // A fake apm that exits non-zero => the binary falls back and STILL exits 0.
    let path = fake_apm(apm_dir.path(), 11);
    Command::cargo_bin("rusty-brain-install")
        .unwrap()
        .current_dir(proj.path())
        .env("PATH", &path)
        .args(["--json", "install"])
        .assert()
        .success() // fail-open: never a non-zero process exit
        .stdout(contains("claude-code")); // the P4 fallback ran
}

/// REAL smoke test: requires `apm` on PATH and network. Run manually with
/// `cargo test -p rb-install --test apm -- --ignored`.
#[ignore = "requires a real apm binary + network"]
#[test]
fn real_apm_install_smoke() {
    let proj = tempfile::tempdir().unwrap();
    // Copy the committed apm/apm.yml into the project so `apm install` has a
    // manifest to act on.
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .unwrap()
        .to_path_buf();
    let src = root.join("apm").join("apm.yml");
    std::fs::copy(&src, proj.path().join("apm.yml")).unwrap();
    Command::cargo_bin("rusty-brain-install")
        .unwrap()
        .current_dir(proj.path())
        .args(["--json", "install"])
        .assert()
        .success();
}
```

- [ ] **Step 2: run it.** Run: `cargo test -p rb-install --test apm` — Expected: the three non-ignored unix tests run. They should PASS given Task C4's wiring; if `install_delegates_to_fake_apm` fails because the report shows no `apm` agent, confirm the orchestrator's success path runs (a fake apm exiting 0). The `real_apm_install_smoke` test is skipped (ignored).

- [ ] **Step 3: (no impl change expected).** These exercise C1–C4 through the real binary. If Step 2 is green, record "no impl change needed".

- [ ] **Step 4: run it.** Run: `cargo test -p rb-install` — Expected: PASS (unit + all integration tests, the smoke test ignored).

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rb-install --all-targets -- -D warnings` (Expected: no warnings) then `cargo fmt --all` (Expected: no diff).

- [ ] **Step 6: commit.** Run: `git add crates/rb-install/tests/apm.rs && git commit -m "test(rb-install): fake-apm delegation, fail-open fallback, ignored real smoke"` — Expected: one commit.

---

### Task C6: Part C gate

**Files:**
- (none — verification only)

- [ ] **Step 1: agent-crate tests — Run:** `cargo test -p rb-agents -p rb-hooks -p rb-install` — Expected: PASS, 0 failures (the smoke test stays `#[ignore]`).

- [ ] **Step 2: workspace tests — Run:** `cargo test --workspace` — Expected: PASS, 0 failures.

- [ ] **Step 3: clippy — Run:** `cargo clippy --workspace --all-targets --all-features -- -D warnings` — Expected: no warnings.

- [ ] **Step 4: format — Run:** `cargo fmt --all --check` — Expected: no diff.

- [ ] **Step 5: default-closure guard — Run:** `cargo tree -e no-dev -p rusty-brain | grep -E "rb-agents|rb-hooks|rb-install"; echo "exit=$?"` — Expected: `exit=1` (no match): the `apm_backend` work stayed inside `rb-install`; the default `rusty-brain` closure is unchanged.

- [ ] **Step 6: supply chain — Run:** `cargo deny check` — Expected: ok (`licenses ok`, `advisories ok`, `sources ok`; `bans` may `warn` only). Part C added no dep.

- [ ] **Step 7: commit (only if a gate fixup was needed) — Run:** `git add -A && git commit -m "chore(rb-install): part C gate green"` — Expected: a commit only if a fixup was needed.

---

## Part D — CI manifest validation + final gate

This Part wires the committed manifest into CI: a new `apm-manifest` job runs `rusty-brain apm validate apm/apm.yml`, so a drift between the binary's MCP surface and the published manifest, or any introduced secret, breaks the build. The Part ends with the full cross-Part gate.

HARD RULES honored throughout: the CI step is read-only validation (it never runs `apm install`); the existing default `clippy-test`/`build-agents` jobs are untouched.

---

### Task D1: CI `apm-manifest` validation job

Add a CI job that builds `rusty-brain` and runs `apm validate` over the committed manifest.

**Files:**
- Modify: .github/workflows/ci.yml

**Test:** a YAML-parse + key-presence assertion (the CI file is not Rust; we validate its structure with `python3 -c`).

- [ ] **Step 1 RED: prove the job is absent.** Run: `grep -n "apm-manifest" .github/workflows/ci.yml; echo "exit=$?"` — Expected: `exit=1` (no match) — the job does not exist yet.

- [ ] **Step 2 GREEN: add the job.** Edit `.github/workflows/ci.yml` and add this job under `jobs:` (append after the existing `build-agents` job, preserving indentation — two-space top-level job keys):

```yaml
  apm-manifest:
    name: apm manifest validation
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - name: Build rusty-brain
        run: cargo build -p rusty-brain
      - name: Validate the committed APM manifest (read-only; rejects secrets/missing ref/malformed MCP entry)
        run: cargo run -p rusty-brain -- apm validate apm/apm.yml
```

- [ ] **Step 3: run it.** Run: `grep -n "apm-manifest" .github/workflows/ci.yml` (Expected: PASS — matches the job key) then validate the YAML parses and the existing jobs are intact: **Run:** `python3 -c "import yaml; d=yaml.safe_load(open('.github/workflows/ci.yml')); j=d['jobs']; assert 'apm-manifest' in j; assert 'clippy-test' in j and j['clippy-test']['steps'][-1]['run']=='cargo test --workspace'; assert 'build-agents' in j; print('ok', sorted(j))"` (Expected: prints `ok [...]` including `apm-manifest`, confirming the default `clippy-test` job is unchanged).

- [ ] **Step 4: locally mirror the CI step — Run:** `cargo run -p rusty-brain -- apm validate apm/apm.yml; echo "exit=$?"` — Expected: prints the `ok:` line and `exit=0` (the committed manifest validates exactly as CI will check).

- [ ] **Step 5: lint+format — Run:** `cargo fmt --all --check` — Expected: no diff (the workflow edit touches no Rust).

- [ ] **Step 6: commit — Run:** `git add .github/workflows/ci.yml && git commit -m "ci: validate the committed apm manifest with rusty-brain apm validate"` — Expected: one commit.

---

### Task D2: final cross-Part gate

**Files:**
- (none — verification only)

- [ ] **Step 1: full workspace tests — Run:** `cargo test --workspace` — Expected: PASS, 0 failures. Confirms Parts A–D compose: the `apm` CLI, the committed manifest fixture, and the `rb-install` apm backend all green.

- [ ] **Step 2: clippy (all features) — Run:** `cargo clippy --workspace --all-targets --all-features -- -D warnings` — Expected: no warnings.

- [ ] **Step 3: format — Run:** `cargo fmt --all --check` — Expected: no diff.

- [ ] **Step 4: supply chain — Run:** `cargo deny check 2>&1 | tail -20; echo "exit=${PIPESTATUS[0]}"` — Expected: `exit=0`, `licenses ok`, `advisories ok`, `sources ok` (`bans` may `warn` only). Confirms `serde_yaml_ng` (the only new dep, from Part B) is accepted.

- [ ] **Step 5: default-closure guard — Run:** `cargo tree -e no-dev -p rusty-brain | grep -E "rb-agents|rb-hooks|rb-install"; echo "exit=$?"` — Expected: `exit=1` (no match): the default `rusty-brain` binary closure never pulls in an agent crate. The `apm` module added only `serde_yaml_ng`/`thiserror`, no agent crate.

- [ ] **Step 6: committed-manifest validation (mirrors CI) — Run:** `cargo run -p rusty-brain -- apm validate apm/apm.yml; echo "exit=$?"` — Expected: `exit=0`, the `ok:` line printed.

- [ ] **Step 7: emit→validate round-trip (mirrors CI intent) — Run:** `cargo run -p rusty-brain -- apm emit` — Expected: prints the canonical stdio MCP entry (with the `ContractVersion` comment), matching the committed manifest's MCP block.

- [ ] **Step 8: ignored real smoke remains opt-in — Run:** `cargo test -p rb-install --test apm -- --list 2>/dev/null | grep -i "real_apm_install_smoke"; echo "exit=$?"` — Expected: `exit=0` — the smoke test is present but `#[ignore]` (not run in the default suite; run manually with `-- --ignored` when a real `apm` is installed).

- [ ] **Step 9: commit (only if a final fixup was needed) — Run:** `git add -A && git commit -m "chore(apm): p7 final gate green"` — Expected: a commit only if a fixup was needed.
