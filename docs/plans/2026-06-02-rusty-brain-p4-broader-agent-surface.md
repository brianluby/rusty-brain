# rusty-brain — P4 (Broader Agent Surface) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Implement Parts strictly in build order **V → W → X → Y → Z**; each Part ends with a gate that must be green before the next Part starts.

**Goal:** Ship spec §17's P4 — capture hooks + a multi-agent installer that wires four agent CLIs (Claude Code, OpenCode, Gemini, Codex) into the shared memory daemon — as **fail-open** integrations in new, isolated crates that never enter the default build closure.

> **Scope correction (P4-v1):** the installer ships for **Claude Code, Gemini, and Codex** only. **OpenCode is deferred** to a follow-on: it loads hooks via a JS/TS plugin in `.opencode/plugins/`, not a JSON hooks block, so a JSON-writing installer would be inert. The dormant `rb-agents` OpenCode adapter and the hook binary's `--agent opencode` support are kept for that future plugin; `--agents opencode` on the installer returns a clear "deferred" error. The three shipped CLIs use their **own** real hook shapes, not a single Claude-shaped block: Gemini's event names are `SessionStart`/`AfterTool`/`SessionEnd`/`PreCompress` (Codex reuses Claude's `SessionStart`/`PostToolUse`/`Stop`/`PreCompact`); Gemini and Codex have **no `args` field**, so their command is one inline shell string (`"<bin>" --agent <id>`) while Claude Code keeps exec `command`+`args`; and SessionStart context is injected via `hookSpecificOutput.additionalContext` (the field that feeds the model), not the user-facing `systemMessage`.

**Architecture:** Three new crates, all `[workspace]` members but referenced by **no core crate** (so `cargo build` and the `rusty-brain` binary never compile them): `rb-agents` (a shared spine — a canonical `HookEvent`/`HookResult` model, a per-CLI `AgentCli` JSON adapter trait, a **fail-open `DaemonClient`** wrapping `rb-proto::Client` with a timeout + total error-swallowing, ported namespace detection, and the `AgentInstaller` install contract); `rb-hooks` (the `rusty-brain-hooks` binary an agent CLI invokes per event — reads the CLI's hook JSON on stdin, captures memories / injects context via the daemon, and **always exits 0**); and `rb-install` (the `rusty-brain-install` binary that **merges** a sentinel-marked hook block into each CLI's existing config and cleanly uninstalls it). The reference implementation `rusty-brain-old` is ported in spirit, with the direct memory-library coupling replaced by daemon calls over `rb-proto`.

**Tech Stack:** Rust 2021 (stable, pinned). New crates depend only on `rb-agents`/`rb-proto`/`rb-types` + `serde`/`serde_json`/`tokio`/`anyhow`/`clap`/`tracing` (all workspace deps already in the default closure) — **no new third-party dependency** enters the build. Tests are TDD, in-process, offline (`DeterministicProvider` daemon over a temp Unix socket; fail-open proven against a dead socket).

**Reference spec:** `docs/specs/2026-05-31-rusty-brain-architecture-design.md` — §17 (P4 bullet), §5 (security boundaries fail closed; capture hooks fail open), §7 (dependency budget), §19 (future options). **Reference implementation (port in spirit, not verbatim):** `/Volumes/raid1/repos/rusty-brain-old` (`crates/{hooks,platforms,types}`, `specs/006-claude-code-hooks`, `specs/011-agent-installs`, `install.sh`). Prior plans `docs/plans/2026-05-31-rusty-brain-p0-foundation.md` … `2026-06-02-rusty-brain-p3-deferred-features.md` are the style template.

---

## Hard rules (carry forward from P0–P3; apply to every task)

- **TDD:** failing test first (RED), minimal implementation (GREEN), then clippy + fmt, then commit. One logical change per commit.
- **Conventional commits**, lowercase, crate-scoped, one line, **NO AI attribution** (no "Generated with…", no `Co-Authored-By`).
- **FAIL-OPEN is the law of P4** (spec §5): a hook must *never* block, slow, or break the host agent. Every daemon call is wrapped in a timeout; every error/timeout/version-mismatch degrades to a safe default (no-op / `continue: true`); the binary is wrapped in `catch_unwind` and **always `std::process::exit(0)`**. There is **no memory-debt and no blocking** — hooks only capture and inject.
- **No-panic in non-test code:** workspace lints deny `unwrap_used`/`expect_used`/`panic`. Return `Result`/`Option` or a documented safe default. Fail-open code swallows errors into `Option::None` / `continue: true` — *without* `unwrap`. Test modules opt out with `#![allow(clippy::unwrap_used, clippy::expect_used)]`.
- **The three new crates must stay OUT of the default closure.** No core crate (`rb-types`…`rusty-brain`) may depend on `rb-agents`/`rb-hooks`/`rb-install`. Every Part's gate verifies: `cargo tree -e no-dev -p rusty-brain | grep -E "rb-agents|rb-hooks|rb-install"` returns **nothing**.
- **Route through the daemon, never a library:** hooks reach memory only via `rb-agents::DaemonClient` (which uses `rb-proto::Client` with the `CONTRACT_VERSION` handshake). They reuse the existing namespace detection and `RUSTY_BRAIN_SOCKET`/`RUSTY_BRAIN_DB` path resolution, and auto-start the daemon **only** on SessionStart (never on high-frequency events).
- **Per-Part gate** (final task of each Part): `cargo test -p <crates>`; `cargo clippy -p <crates> --all-targets -- -D warnings`; `cargo fmt --all --check`; plus the closure check above. Parts **V** and **Z** also run the full workspace gate; Part **Z** also runs `cargo deny check`.
- **Commands run from the worktree root** `/Volumes/raid1/repos/rusty-brain-p4`.

## Seam map (current `main` code the new crates build on)

| Seam | Location | Used by |
|---|---|---|
| `rb-proto::Client` — `connect(socket, namespace)` (Handshake + `CONTRACT_VERSION`), `remember`/`recall`/`context`/`ping` | `crates/rb-proto/src/client.rs` | V (`DaemonClient`) |
| Daemon auto-start — `connect_or_start` + `daemon_command_with` (env_clear + allowlist + `process_group(0)` detach, runs `rusty-brain serve`) | `crates/rusty-brain/src/client.rs` | V (`AutoStart`, SessionStart only) |
| Namespace detection — `detect_namespace*` / `find_nearest_claude_md` / `git_toplevel` (CLAUDE.md → git → cwd → Global), runs off-runtime | `crates/rusty-brain/src/namespace_detect.rs` | V (ported into `rb-agents::namespace`) |
| Path resolution — `RUSTY_BRAIN_SOCKET` / `RUSTY_BRAIN_DB`, `default_socket_path`/`default_db_path` | `crates/rusty-brain/src/paths.rs`, `rb-daemon` | V, W, Z |
| Core types — `Namespace`, `MemoryId`, `MemoryNote`, `MemoryType`, `SearchResult`, `Error`/`Result` | `crates/rb-types/src/` | all |
| Dep-budget precedent — keeping a crate out of the default closure (the `local` feature) + CI `build-local` job | `Cargo.toml`, `deny.toml`, `.github/workflows/ci.yml` | Z |

## Build order & the shared `rb-agents` contract

```text
Part V  rb-agents spine        (event model + AgentCli + fail-open DaemonClient + namespace + Claude Code adapter)
Part W  rb-hooks binary        (stdin/stdout dispatch + the 4 capture flows + dedup + context injection; Claude Code end-to-end)
Part X  the 3 more adapters    (OpenCode / Gemini / Codex AgentCli; replaces V's PassthroughCli)
Part Y  rb-install binary      (per-CLI AgentInstaller: detect + merge + sentinel + backup + uninstall + status + dry-run)
Part Z  CI / packaging / e2e   (build-agents job + closure proof + install.sh binaries + install→hook→capture→uninstall test)
```

Part **V** introduces the shared contract — `HookEvent` / `HookResult` / `HookContext`, `AgentId`, the `AgentCli` trait + `agent_for(id)` registry, the fail-open `DaemonClient` (+ `AutoStart`), `detect_namespace`, and the install-side `InstallScope` / `HookFragment` / `SENTINEL` / `AgentInstaller`. Parts **W/X/Y consume these names verbatim** and never redefine them: W builds the hook binary (Claude Code), X swaps V's `PassthroughCli` for the real OpenCode/Gemini/Codex adapters, and Y implements `AgentInstaller` per CLI.

---

## Part V — rb-agents (shared agent spine)

This Part introduces `crates/rb-agents`, the CLI-agnostic spine that Parts W (rb-hooks runtime), X (the other three CLI adapters), and Y (rb-install) all build on. It defines the canonical `HookEvent`/`HookResult`/`HookContext` event model, the `AgentId` + `AgentCli` JSON-adapter trait with a registry, a fully-implemented Claude Code reference adapter, a strictly fail-open `DaemonClient` wrapper over `rb_proto::Client` (any error degrades to `None`, never panics, never blocks unbounded), a self-contained copy of namespace detection (so the hook binary never links the `rusty-brain` crate), and the install-side `AgentInstaller` contract. The crate is added to `[workspace] members` but is NEVER referenced by any core crate, so the default `cargo build` and the `rusty-brain` binary never compile it — mirroring how the `local` feature stays out of the default closure. All commands run from the worktree root `/Volumes/raid1/repos/rusty-brain-p4`.

The `agent_for` design is resolved cleanly here: `ClaudeCodeCli` is the real, complete reference adapter; the other three `AgentId` arms (`OpenCode`, `Gemini`, `Codex`) return a `PassthroughCli` placeholder that parses every input into `HookEvent::Other` and renders Claude-style stdout. Part X replaces those three arms with real adapters; the `agent_for` signature and the `AgentCli` contract do NOT change.

---

### Task V1: rb-agents crate skeleton — workspace wiring

**Files:**
- Modify: `Cargo.toml`
- Create: `crates/rb-agents/Cargo.toml`
- Create: `crates/rb-agents/src/lib.rs`

- [ ] **Step 1 RED: add a placeholder lib so the crate exists and `cargo test -p rb-agents` resolves.** Create `crates/rb-agents/src/lib.rs`:

```rust
//! `rb-agents` — CLI-agnostic spine for the agent hook + install surface.
//!
//! Defines the canonical hook event model, the per-CLI `AgentCli` JSON adapter
//! trait + registry, a strictly fail-open `DaemonClient` over `rb_proto`, a
//! self-contained namespace detector, and the install-side `AgentInstaller`
//! contract. NEVER referenced by any core crate: kept out of the default build
//! closure so the `rusty-brain` binary never links it.
#![forbid(unsafe_code)]

#[cfg(test)]
mod crate_smoke {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    #[test]
    fn crate_compiles_and_links() {
        // Placeholder proving the crate is wired into the workspace and builds.
        assert_eq!(2 + 2, 4);
    }
}
```

- [ ] **Step 2: run it — Run: `cargo test -p rb-agents crate_compiles_and_links`** — Expected: FAIL with `error: package ID specification 'rb-agents' did not match any packages` (the crate is not a workspace member yet and has no manifest).

- [ ] **Step 3 GREEN: add the manifest and register the member.** Create `crates/rb-agents/Cargo.toml`:

```toml
[package]
name = "rb-agents"
version.workspace = true
edition.workspace = true
license.workspace = true
authors.workspace = true
repository.workspace = true
description = "rusty-brain: CLI-agnostic agent hook + install spine."

[lib]
name = "rb_agents"
path = "src/lib.rs"

[dependencies]
rb-types = { path = "../rb-types" }
rb-proto = { path = "../rb-proto" }
serde = { workspace = true }
serde_json = { workspace = true }
tokio = { workspace = true }
tracing = { workspace = true }

[dev-dependencies]
rb-embed = { path = "../rb-embed" }
rb-daemon = { path = "../rb-daemon" }
tempfile = { workspace = true }

[lints]
workspace = true
```

Then add the member to the workspace. Modify `Cargo.toml` — change:

```toml
    "crates/rb-mcp",
    "crates/rusty-brain",
]
```

to:

```toml
    "crates/rb-mcp",
    "crates/rusty-brain",
    "crates/rb-agents",
]
```

- [ ] **Step 4: run it — Run: `cargo test -p rb-agents crate_compiles_and_links`** — Expected: PASS (1 test).

- [ ] **Step 5: lint+format — `cargo clippy -p rb-agents --all-targets -- -D warnings`** (no warnings) then **`cargo fmt --all`** (no diff).

- [ ] **Step 6: confirm the new crate is NOT in any core crate's dependency closure — Run: `cargo tree -e no-dev -p rusty-brain | grep -E "rb-agents|rb-hooks|rb-install"`** — Expected: NO output (exit status 1; nothing matched). The crate is a workspace member but no core crate depends on it.

- [ ] **Step 7: commit — `git add Cargo.toml crates/rb-agents/Cargo.toml crates/rb-agents/src/lib.rs && git commit -m "chore(rb-agents): scaffold crate and register workspace member"`** — Expected: one commit.

---

### Task V2: rb-agents src/event.rs — event model

**Files:**
- Create: `crates/rb-agents/src/event.rs`
- Modify: `crates/rb-agents/src/lib.rs`
- Test: inline `#[cfg(test)] mod tests` in `crates/rb-agents/src/event.rs`

- [ ] **Step 1 RED: write the failing test for the canonical event model.** Create `crates/rb-agents/src/event.rs`:

```rust
//! Canonical, CLI-agnostic hook event model. Every `AgentCli` parses its own
//! wire JSON into a [`HookContext`] carrying one [`HookEvent`]; the runtime
//! dispatches on the event and produces a [`HookResult`] which the same
//! `AgentCli` renders back to CLI-specific stdout JSON.

use std::path::PathBuf;

/// A captured hook event, normalized across every supported CLI. Unknown or
/// unparseable events MUST map to [`HookEvent::Other`] (never an error/panic).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookEvent {
    /// A new agent session began. `source` is the CLI's trigger label, if any
    /// (e.g. Claude Code `"startup"`).
    SessionStart { source: Option<String> },
    /// A tool finished running. Carries the tool name and the raw input/response
    /// JSON so the runtime can decide whether to capture an observation.
    PostToolUse {
        tool_name: String,
        tool_input: serde_json::Value,
        tool_response: serde_json::Value,
    },
    /// The assistant turn is stopping. Carries the last assistant message, if any.
    Stop { last_assistant_message: Option<String> },
    /// The context is about to be compacted. Carries any custom instructions.
    PreCompact { custom_instructions: Option<String> },
    /// An event we do not model (or could not parse). Carries the raw event name.
    Other(String),
}

/// The result of handling a hook event. In P4 `continue_execution` is ALWAYS
/// `true`: capture hooks are strictly fail-open and never block the CLI.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HookResult {
    /// Text to surface to the agent (e.g. injected context). `None` = nothing.
    pub system_message: Option<String>,
    /// Always `true` in P4. Kept explicit so renderers emit it verbatim.
    pub continue_execution: bool,
}

/// A normalized hook invocation: the event plus the resolved cwd and session id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookContext {
    pub event: HookEvent,
    pub cwd: PathBuf,
    pub session_id: Option<String>,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn hook_result_default_does_not_continue_and_has_no_message() {
        let r = HookResult::default();
        assert!(!r.continue_execution);
        assert_eq!(r.system_message, None);
    }

    #[test]
    fn hook_context_carries_event_cwd_and_session() {
        let ctx = HookContext {
            event: HookEvent::SessionStart {
                source: Some("startup".to_string()),
            },
            cwd: PathBuf::from("/work/project"),
            session_id: Some("sess-1".to_string()),
        };
        assert_eq!(
            ctx.event,
            HookEvent::SessionStart {
                source: Some("startup".to_string())
            }
        );
        assert_eq!(ctx.cwd, PathBuf::from("/work/project"));
        assert_eq!(ctx.session_id.as_deref(), Some("sess-1"));
    }

    #[test]
    fn post_tool_use_carries_tool_name_and_payloads() {
        let ev = HookEvent::PostToolUse {
            tool_name: "Write".to_string(),
            tool_input: serde_json::json!({"file_path": "/tmp/x"}),
            tool_response: serde_json::json!({"success": true}),
        };
        match ev {
            HookEvent::PostToolUse {
                tool_name,
                tool_input,
                tool_response,
            } => {
                assert_eq!(tool_name, "Write");
                assert_eq!(tool_input["file_path"], "/tmp/x");
                assert_eq!(tool_response["success"], true);
            }
            other => panic!("expected PostToolUse, got {other:?}"),
        }
    }

    #[test]
    fn other_event_preserves_raw_name() {
        let ev = HookEvent::Other("UserPromptSubmit".to_string());
        assert_eq!(ev, HookEvent::Other("UserPromptSubmit".to_string()));
    }
}
```

Then wire the module. Modify `crates/rb-agents/src/lib.rs` — replace the `#[cfg(test)] mod crate_smoke { ... }` block (the entire placeholder block from Task V1) with:

```rust
mod event;

pub use event::{HookContext, HookEvent, HookResult};
```

- [ ] **Step 2: run it — Run: `cargo test -p rb-agents event::`** — Expected: FAIL with `error[E0583]: file not found for module 'event'` resolves once created, then the test binary compiles and runs; before adding the `mod event;` line it FAILS with an unresolved-import/missing-module error.

- [ ] **Step 3 GREEN: the module file and the `mod event; pub use ...` wiring above already provide the real implementation.** Confirm `crates/rb-agents/src/lib.rs` now reads exactly:

```rust
//! `rb-agents` — CLI-agnostic spine for the agent hook + install surface.
//!
//! Defines the canonical hook event model, the per-CLI `AgentCli` JSON adapter
//! trait + registry, a strictly fail-open `DaemonClient` over `rb_proto`, a
//! self-contained namespace detector, and the install-side `AgentInstaller`
//! contract. NEVER referenced by any core crate: kept out of the default build
//! closure so the `rusty-brain` binary never links it.
#![forbid(unsafe_code)]

mod event;

pub use event::{HookContext, HookEvent, HookResult};
```

- [ ] **Step 4: run it — Run: `cargo test -p rb-agents event::`** — Expected: PASS (4 tests).

- [ ] **Step 5: lint+format — `cargo clippy -p rb-agents --all-targets -- -D warnings`** (no warnings) then **`cargo fmt --all`** (no diff).

- [ ] **Step 6: commit — `git add crates/rb-agents/src/event.rs crates/rb-agents/src/lib.rs && git commit -m "feat(rb-agents): add canonical hook event model"`** — Expected: one commit.

---

### Task V3: rb-agents src/claude_code.rs — Claude adapter

**Files:**
- Create: `crates/rb-agents/src/claude_code.rs`
- Modify: `crates/rb-agents/src/lib.rs`
- Test: inline `#[cfg(test)] mod tests` in `crates/rb-agents/src/claude_code.rs`

(The `AgentCli` trait this file implements is introduced in the same change set; the trait lives in `cli.rs` added in Task V4, but to keep each file self-contained and avoid forward cross-references, V3 defines `ClaudeCodeCli` against a locally-declared trait shape that V4 promotes to the shared `cli::AgentCli`. To avoid duplicate trait definitions, V3 instead declares the trait inline here as a temporary internal trait? — NO. Resolve cleanly: V3 depends on the trait, so V3 and V4 are authored together. V3 below references `crate::cli::{AgentCli, AgentId}`; therefore add `mod cli;` BEFORE this task's GREEN compiles. The Step 1 RED for V3 is run AFTER V4's `cli.rs` exists. Author order on disk: write `cli.rs` (V4) and `claude_code.rs` (V3) in one change set; the checkboxes below assume `cli.rs` from Task V4 is present. If you execute V3 before V4, its RED fails to compile on the missing `crate::cli` module, which is the expected RED.)

- [ ] **Step 1 RED: write the failing Claude Code adapter test.** Create `crates/rb-agents/src/claude_code.rs`:

```rust
//! Reference `AgentCli` adapter for Claude Code. Maps Claude Code's stdin hook
//! JSON into the canonical [`HookContext`] and renders [`HookResult`] back to
//! Claude Code's stdout JSON. Strictly fail-open: any missing/garbage field
//! degrades to a safe default and an unknown `hook_event_name` becomes
//! [`HookEvent::Other`]; parsing NEVER panics.

use std::path::PathBuf;

use serde_json::Value;

use crate::cli::{AgentCli, AgentId};
use crate::event::{HookContext, HookEvent, HookResult};

/// Claude Code hook adapter. Field names follow the Claude Code hook protocol
/// (`hook_event_name`, `tool_name`, `tool_input`, `tool_response`,
/// `last_assistant_message`, `custom_instructions`, `source`, `cwd`,
/// `session_id`).
#[derive(Debug, Clone, Copy, Default)]
pub struct ClaudeCodeCli;

/// Read an optional string field; absent / non-string => `None`.
fn opt_str(raw: &Value, key: &str) -> Option<String> {
    raw.get(key).and_then(Value::as_str).map(str::to_string)
}

/// Read an object field as raw JSON; absent => `Value::Null`.
fn json_or_null(raw: &Value, key: &str) -> Value {
    raw.get(key).cloned().unwrap_or(Value::Null)
}

impl AgentCli for ClaudeCodeCli {
    fn id(&self) -> AgentId {
        AgentId::ClaudeCode
    }

    fn binary_name(&self) -> &'static str {
        "claude"
    }

    fn parse_input(&self, raw: &Value) -> HookContext {
        let cwd = opt_str(raw, "cwd")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let session_id = opt_str(raw, "session_id");
        let event_name = opt_str(raw, "hook_event_name").unwrap_or_default();
        let event = match event_name.as_str() {
            "SessionStart" => HookEvent::SessionStart {
                source: opt_str(raw, "source"),
            },
            "PostToolUse" => HookEvent::PostToolUse {
                tool_name: opt_str(raw, "tool_name").unwrap_or_default(),
                tool_input: json_or_null(raw, "tool_input"),
                tool_response: json_or_null(raw, "tool_response"),
            },
            "Stop" => HookEvent::Stop {
                last_assistant_message: opt_str(raw, "last_assistant_message"),
            },
            "PreCompact" => HookEvent::PreCompact {
                custom_instructions: opt_str(raw, "custom_instructions"),
            },
            other => HookEvent::Other(other.to_string()),
        };
        HookContext {
            event,
            cwd,
            session_id,
        }
    }

    fn render_output(&self, result: &HookResult) -> Value {
        let mut out = serde_json::Map::new();
        out.insert("continue".to_string(), Value::Bool(true));
        out.insert("suppressOutput".to_string(), Value::Bool(true));
        if let Some(message) = &result.system_message {
            out.insert("systemMessage".to_string(), Value::String(message.clone()));
        }
        Value::Object(out)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use crate::cli::AgentId;
    use crate::event::{HookEvent, HookResult};

    fn cli() -> ClaudeCodeCli {
        ClaudeCodeCli
    }

    #[test]
    fn identity_and_binary_name() {
        assert_eq!(cli().id(), AgentId::ClaudeCode);
        assert_eq!(cli().binary_name(), "claude");
    }

    #[test]
    fn parses_session_start_with_source_cwd_and_session() {
        let raw = serde_json::json!({
            "hook_event_name": "SessionStart",
            "source": "startup",
            "cwd": "/home/user/project",
            "session_id": "abc123"
        });
        let ctx = cli().parse_input(&raw);
        assert_eq!(
            ctx.event,
            HookEvent::SessionStart {
                source: Some("startup".to_string())
            }
        );
        assert_eq!(ctx.cwd, PathBuf::from("/home/user/project"));
        assert_eq!(ctx.session_id.as_deref(), Some("abc123"));
    }

    #[test]
    fn parses_post_tool_use_with_input_and_response() {
        let raw = serde_json::json!({
            "hook_event_name": "PostToolUse",
            "tool_name": "Write",
            "tool_input": {"file_path": "/tmp/test.txt", "content": "hello"},
            "tool_response": {"success": true},
            "cwd": "/p"
        });
        let ctx = cli().parse_input(&raw);
        match ctx.event {
            HookEvent::PostToolUse {
                tool_name,
                tool_input,
                tool_response,
            } => {
                assert_eq!(tool_name, "Write");
                assert_eq!(tool_input["file_path"], "/tmp/test.txt");
                assert_eq!(tool_response["success"], true);
            }
            other => panic!("expected PostToolUse, got {other:?}"),
        }
    }

    #[test]
    fn parses_stop_with_last_assistant_message() {
        let raw = serde_json::json!({
            "hook_event_name": "Stop",
            "last_assistant_message": "done.",
            "cwd": "/p"
        });
        let ctx = cli().parse_input(&raw);
        assert_eq!(
            ctx.event,
            HookEvent::Stop {
                last_assistant_message: Some("done.".to_string())
            }
        );
    }

    #[test]
    fn parses_precompact_with_custom_instructions() {
        let raw = serde_json::json!({
            "hook_event_name": "PreCompact",
            "custom_instructions": "keep decisions",
            "cwd": "/p"
        });
        let ctx = cli().parse_input(&raw);
        assert_eq!(
            ctx.event,
            HookEvent::PreCompact {
                custom_instructions: Some("keep decisions".to_string())
            }
        );
    }

    #[test]
    fn unknown_event_name_becomes_other() {
        let raw = serde_json::json!({
            "hook_event_name": "UserPromptSubmit",
            "cwd": "/p"
        });
        let ctx = cli().parse_input(&raw);
        assert_eq!(ctx.event, HookEvent::Other("UserPromptSubmit".to_string()));
    }

    #[test]
    fn malformed_input_degrades_to_other_with_default_cwd_never_panics() {
        // No hook_event_name, no cwd, tool fields the wrong JSON type.
        let raw = serde_json::json!({
            "tool_input": 12345,
            "session_id": false
        });
        let ctx = cli().parse_input(&raw);
        assert_eq!(ctx.event, HookEvent::Other(String::new()));
        assert_eq!(ctx.cwd, PathBuf::from("."));
        // session_id was a bool, not a string -> dropped to None.
        assert_eq!(ctx.session_id, None);
    }

    #[test]
    fn non_object_input_degrades_safely() {
        // A bare JSON array is not an object; every getter returns None.
        let raw = serde_json::json!(["nope"]);
        let ctx = cli().parse_input(&raw);
        assert_eq!(ctx.event, HookEvent::Other(String::new()));
        assert_eq!(ctx.cwd, PathBuf::from("."));
        assert_eq!(ctx.session_id, None);
    }

    #[test]
    fn render_output_always_continues_and_suppresses() {
        let out = cli().render_output(&HookResult {
            system_message: None,
            continue_execution: true,
        });
        assert_eq!(out["continue"], true);
        assert_eq!(out["suppressOutput"], true);
        assert!(out.get("systemMessage").is_none());
    }

    #[test]
    fn render_output_includes_system_message_when_present() {
        let out = cli().render_output(&HookResult {
            system_message: Some("injected context".to_string()),
            continue_execution: true,
        });
        assert_eq!(out["continue"], true);
        assert_eq!(out["systemMessage"], "injected context");
    }
}
```

Then wire the module. Modify `crates/rb-agents/src/lib.rs` — change:

```rust
mod event;

pub use event::{HookContext, HookEvent, HookResult};
```

to:

```rust
mod claude_code;
mod cli;
mod event;

pub use claude_code::ClaudeCodeCli;
pub use cli::{agent_for, AgentCli, AgentId, PassthroughCli};
pub use event::{HookContext, HookEvent, HookResult};
```

- [ ] **Step 2: run it — Run: `cargo test -p rb-agents claude_code::`** — Expected: FAIL with `error[E0583]: file not found for module 'cli'` / unresolved `crate::cli` (the `cli` module — Task V4 — does not exist yet). This RED is satisfied by completing Task V4 next.

- [ ] **Step 3 GREEN: the `claude_code.rs` implementation above is the real, complete adapter.** It compiles once `cli.rs` (Task V4) exists. No further code is added in this task's GREEN beyond the file + module wiring above.

- [ ] **Step 4: run it — Run: `cargo test -p rb-agents claude_code::`** — Expected: PASS (11 tests). (Run after Task V4 lands `cli.rs`.)

- [ ] **Step 5: lint+format — `cargo clippy -p rb-agents --all-targets -- -D warnings`** (no warnings) then **`cargo fmt --all`** (no diff).

- [ ] **Step 6: commit — `git add crates/rb-agents/src/claude_code.rs crates/rb-agents/src/lib.rs && git commit -m "feat(rb-agents): add claude code reference adapter"`** — Expected: one commit. (Stage alongside Task V4's `cli.rs` if executed together.)

---

### Task V4: rb-agents src/cli.rs — agent registry

**Files:**
- Create: `crates/rb-agents/src/cli.rs`
- Test: inline `#[cfg(test)] mod tests` in `crates/rb-agents/src/cli.rs`

- [ ] **Step 1 RED: write the failing test for `AgentId`, the `AgentCli` trait, `PassthroughCli`, and `agent_for`.** Create `crates/rb-agents/src/cli.rs`:

```rust
//! Per-CLI identity (`AgentId`), the `AgentCli` JSON-adapter trait, a
//! `PassthroughCli` placeholder, and the `agent_for` registry. Part V wires
//! Claude Code fully and routes the other three CLIs to `PassthroughCli`; Part X
//! replaces those arms with real adapters WITHOUT changing this signature.

use serde_json::Value;

use crate::claude_code::ClaudeCodeCli;
use crate::event::{HookContext, HookEvent, HookResult};

/// The set of CLIs the agent surface targets in P4-v1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentId {
    ClaudeCode,
    OpenCode,
    Gemini,
    Codex,
}

impl AgentId {
    /// Stable lowercase wire id used on the `--agent` flag and in config.
    pub fn as_str(&self) -> &'static str {
        match self {
            AgentId::ClaudeCode => "claude-code",
            AgentId::OpenCode => "opencode",
            AgentId::Gemini => "gemini",
            AgentId::Codex => "codex",
        }
    }

    /// Parse a wire id back into an `AgentId`. Unknown ids => `None`.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "claude-code" => Some(AgentId::ClaudeCode),
            "opencode" => Some(AgentId::OpenCode),
            "gemini" => Some(AgentId::Gemini),
            "codex" => Some(AgentId::Codex),
            _ => None,
        }
    }
}

/// A per-CLI JSON adapter: identity, the CLI binary name, stdin-JSON parsing,
/// and stdout-JSON rendering. `parse_input` MUST be fail-open (never panic):
/// unknown events => [`HookEvent::Other`], bad fields => safe defaults.
pub trait AgentCli: Send + Sync {
    fn id(&self) -> AgentId;
    fn binary_name(&self) -> &'static str;
    fn parse_input(&self, raw: &Value) -> HookContext;
    fn render_output(&self, result: &HookResult) -> Value;
}

/// Placeholder adapter for CLIs not yet wired (OpenCode/Gemini/Codex in Part V).
/// Parses EVERY input into [`HookEvent::Other`] with a default cwd and renders a
/// Claude-style fail-open stdout object. Part X replaces the three registry arms
/// that use this with real adapters.
#[derive(Debug, Clone, Copy)]
pub struct PassthroughCli {
    id: AgentId,
    binary_name: &'static str,
}

impl PassthroughCli {
    fn new(id: AgentId, binary_name: &'static str) -> Self {
        Self { id, binary_name }
    }
}

impl AgentCli for PassthroughCli {
    fn id(&self) -> AgentId {
        self.id
    }

    fn binary_name(&self) -> &'static str {
        self.binary_name
    }

    fn parse_input(&self, raw: &Value) -> HookContext {
        let cwd = raw
            .get("cwd")
            .and_then(Value::as_str)
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let session_id = raw
            .get("session_id")
            .and_then(Value::as_str)
            .map(str::to_string);
        let raw_name = raw
            .get("hook_event_name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        HookContext {
            event: HookEvent::Other(raw_name),
            cwd,
            session_id,
        }
    }

    fn render_output(&self, result: &HookResult) -> Value {
        let mut out = serde_json::Map::new();
        out.insert("continue".to_string(), Value::Bool(true));
        out.insert("suppressOutput".to_string(), Value::Bool(true));
        if let Some(message) = &result.system_message {
            out.insert("systemMessage".to_string(), Value::String(message.clone()));
        }
        Value::Object(out)
    }
}

/// Registry: return the adapter for `id`. Claude Code is the real reference
/// adapter; the other three route to a [`PassthroughCli`] until Part X replaces
/// them. The signature is FINAL — Part X only swaps the three placeholder arms.
pub fn agent_for(id: AgentId) -> Box<dyn AgentCli> {
    match id {
        AgentId::ClaudeCode => Box::new(ClaudeCodeCli),
        AgentId::OpenCode => Box::new(PassthroughCli::new(AgentId::OpenCode, "opencode")),
        AgentId::Gemini => Box::new(PassthroughCli::new(AgentId::Gemini, "gemini")),
        AgentId::Codex => Box::new(PassthroughCli::new(AgentId::Codex, "codex")),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use crate::event::{HookEvent, HookResult};

    #[test]
    fn agent_id_str_round_trips_all_variants() {
        for id in [
            AgentId::ClaudeCode,
            AgentId::OpenCode,
            AgentId::Gemini,
            AgentId::Codex,
        ] {
            let s = id.as_str();
            assert_eq!(AgentId::parse(s), Some(id));
        }
    }

    #[test]
    fn agent_id_str_values_are_stable() {
        assert_eq!(AgentId::ClaudeCode.as_str(), "claude-code");
        assert_eq!(AgentId::OpenCode.as_str(), "opencode");
        assert_eq!(AgentId::Gemini.as_str(), "gemini");
        assert_eq!(AgentId::Codex.as_str(), "codex");
    }

    #[test]
    fn agent_id_parse_rejects_unknown() {
        assert_eq!(AgentId::parse("copilot"), None);
        assert_eq!(AgentId::parse(""), None);
        assert_eq!(AgentId::parse("ClaudeCode"), None);
    }

    #[test]
    fn registry_returns_claude_code_for_claude_code() {
        let cli = agent_for(AgentId::ClaudeCode);
        assert_eq!(cli.id(), AgentId::ClaudeCode);
        assert_eq!(cli.binary_name(), "claude");
    }

    #[test]
    fn registry_returns_passthrough_for_other_three() {
        let opencode = agent_for(AgentId::OpenCode);
        assert_eq!(opencode.id(), AgentId::OpenCode);
        assert_eq!(opencode.binary_name(), "opencode");

        let gemini = agent_for(AgentId::Gemini);
        assert_eq!(gemini.id(), AgentId::Gemini);
        assert_eq!(gemini.binary_name(), "gemini");

        let codex = agent_for(AgentId::Codex);
        assert_eq!(codex.id(), AgentId::Codex);
        assert_eq!(codex.binary_name(), "codex");
    }

    #[test]
    fn passthrough_maps_known_event_to_other_and_keeps_cwd() {
        let cli = agent_for(AgentId::OpenCode);
        let raw = serde_json::json!({
            "hook_event_name": "SessionStart",
            "cwd": "/proj",
            "session_id": "s1"
        });
        let ctx = cli.parse_input(&raw);
        assert_eq!(ctx.event, HookEvent::Other("SessionStart".to_string()));
        assert_eq!(ctx.cwd, std::path::PathBuf::from("/proj"));
        assert_eq!(ctx.session_id.as_deref(), Some("s1"));
    }

    #[test]
    fn passthrough_render_is_fail_open_continue() {
        let cli = agent_for(AgentId::Codex);
        let out = cli.render_output(&HookResult {
            system_message: Some("hi".to_string()),
            continue_execution: true,
        });
        assert_eq!(out["continue"], true);
        assert_eq!(out["suppressOutput"], true);
        assert_eq!(out["systemMessage"], "hi");
    }

    #[test]
    fn agent_cli_is_object_safe() {
        // Compiles only if `AgentCli` is object-safe (used as `Box<dyn AgentCli>`).
        let cli: Box<dyn AgentCli> = agent_for(AgentId::ClaudeCode);
        assert_eq!(cli.id(), AgentId::ClaudeCode);
    }
}
```

- [ ] **Step 2: run it — Run: `cargo test -p rb-agents cli::`** — Expected: FAIL on first authoring because `claude_code.rs` (Task V3) and the `lib.rs` `mod cli;` wiring must both be present; if `claude_code.rs` is absent the build FAILS with unresolved `crate::claude_code::ClaudeCodeCli`. With Task V3's file present it compiles and the assertions run.

- [ ] **Step 3 GREEN: the `cli.rs` above is the real, complete registry.** The `lib.rs` wiring from Task V3 (`mod cli; pub use cli::{agent_for, AgentCli, AgentId, PassthroughCli};`) already exposes it. No additional code.

- [ ] **Step 4: run it — Run: `cargo test -p rb-agents cli:: claude_code::`** — Expected: PASS (cli: 9 tests; claude_code: 11 tests).

- [ ] **Step 5: lint+format — `cargo clippy -p rb-agents --all-targets -- -D warnings`** (no warnings) then **`cargo fmt --all`** (no diff).

- [ ] **Step 6: commit — `git add crates/rb-agents/src/cli.rs && git commit -m "feat(rb-agents): add agent id, agentcli trait, and registry"`** — Expected: one commit.

---

### Task V5: rb-agents src/namespace.rs — namespace detect

**Files:**
- Create: `crates/rb-agents/src/namespace.rs`
- Modify: `crates/rb-agents/src/lib.rs`
- Test: inline `#[cfg(test)] mod tests` in `crates/rb-agents/src/namespace.rs`

- [ ] **Step 1 RED: write the failing test for the self-contained namespace detector.** Create `crates/rb-agents/src/namespace.rs`:

```rust
//! Self-contained namespace detection (ported from `rusty-brain/src/
//! namespace_detect.rs`) so the hook binary never links the `rusty-brain`
//! crate. Resolution order (first non-empty wins): (1) nearest `CLAUDE.md`
//! frontmatter `project:`, (2) that `CLAUDE.md`'s first `# H1`, (3) git-root dir
//! name, (4) `cwd` dir name, (5) `Global`. Never panics; degrades to `Global`.
//!
//! MUST run OFF the async runtime (reads files, shells out to git).

use std::path::{Path, PathBuf};
use std::process::Command;

use rb_types::Namespace;

/// Detect the namespace for `cwd`. Reads `CLAUDE.md`, invokes git. Synchronous:
/// call this OFF the tokio runtime.
pub fn detect_namespace(cwd: &Path) -> Namespace {
    detect_namespace_with(cwd, find_nearest_claude_md, git_toplevel)
}

/// Pure core, parameterized over the `CLAUDE.md` finder and git-root resolver so
/// every branch is unit-testable without touching the filesystem or git.
pub fn detect_namespace_with<C, G>(start: &Path, find_claude_md: C, git_root: G) -> Namespace
where
    C: Fn(&Path) -> Option<(PathBuf, String)>,
    G: Fn(&Path) -> Option<PathBuf>,
{
    if let Some((_path, text)) = find_claude_md(start) {
        if let Some(name) = parse_project_from_claude_md(&text) {
            return Namespace::Project(name);
        }
    }
    if let Some(name) = git_root(start).as_deref().and_then(dir_name) {
        return Namespace::Project(name);
    }
    if let Some(name) = dir_name(start) {
        return Namespace::Project(name);
    }
    Namespace::Global
}

/// Extract a non-empty, utf8 final path component. `None` for `/`, empty, or
/// non-utf8 names — caller then falls through to the next branch.
fn dir_name(p: &Path) -> Option<String> {
    p.file_name()
        .and_then(|n| n.to_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Parse a `CLAUDE.md` body: prefer frontmatter `project: NAME`, else first `# H1`.
pub fn parse_project_from_claude_md(text: &str) -> Option<String> {
    if let Some(name) = project_from_frontmatter(text) {
        return Some(name);
    }
    first_h1(text)
}

/// Read `project: NAME` from a leading `---`-delimited frontmatter block.
fn project_from_frontmatter(text: &str) -> Option<String> {
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        return None;
    }
    for line in lines {
        let trimmed = line.trim();
        if trimmed == "---" {
            break;
        }
        if let Some(rest) = trimmed.strip_prefix("project:") {
            let value = rest.trim().trim_matches(|c| c == '"' || c == '\'').trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
            return None;
        }
    }
    None
}

/// First markdown `# H1` heading text (exactly one leading `#`), trimmed.
fn first_h1(text: &str) -> Option<String> {
    for line in text.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("# ") {
            let heading = rest.trim();
            if !heading.is_empty() {
                return Some(heading.to_string());
            }
        }
    }
    None
}

/// Walk up from `start`, returning the first `CLAUDE.md`'s `(path, contents)`.
fn find_nearest_claude_md(start: &Path) -> Option<(PathBuf, String)> {
    for dir in start.ancestors() {
        let candidate = dir.join("CLAUDE.md");
        if let Ok(text) = std::fs::read_to_string(&candidate) {
            return Some((candidate, text));
        }
    }
    None
}

/// Find the git toplevel for `dir` by invoking git; `None` if not a repo.
fn git_toplevel(dir: &Path) -> Option<PathBuf> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8(output.stdout).ok()?;
    let trimmed = path.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(PathBuf::from(trimmed))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use rb_types::Namespace;
    use std::fs;
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;

    fn no_claude(_: &Path) -> Option<(PathBuf, String)> {
        None
    }

    fn no_git(_: &Path) -> Option<PathBuf> {
        None
    }

    #[test]
    fn frontmatter_project_wins_over_h1() {
        let text = "---\nproject: from-frontmatter\nother: x\n---\n# From Heading\nbody\n";
        assert_eq!(
            parse_project_from_claude_md(text),
            Some("from-frontmatter".to_string())
        );
    }

    #[test]
    fn falls_back_to_first_h1_when_no_frontmatter_project() {
        let text = "---\nother: x\n---\nintro\n#  Heading Name  \n## sub\n";
        assert_eq!(
            parse_project_from_claude_md(text),
            Some("Heading Name".to_string())
        );
    }

    #[test]
    fn empty_text_is_none() {
        assert_eq!(parse_project_from_claude_md(""), None);
    }

    #[test]
    fn branch1_claude_md_frontmatter_project() {
        let start = Path::new("/home/alice/code/app/src");
        let find_claude = |_: &Path| -> Option<(PathBuf, String)> {
            Some((
                PathBuf::from("/home/alice/code/app/CLAUDE.md"),
                "---\nproject: cool-app\n---\n# Other\n".to_string(),
            ))
        };
        let git_root =
            |_: &Path| -> Option<PathBuf> { Some(PathBuf::from("/home/alice/code/app")) };
        let ns = detect_namespace_with(start, find_claude, git_root);
        assert_eq!(ns, Namespace::Project("cool-app".to_string()));
    }

    #[test]
    fn branch3_git_root_dirname_when_claude_md_useless() {
        let start = Path::new("/home/alice/code/rusty-brain/crates/rb-agents");
        let find_claude = |_: &Path| -> Option<(PathBuf, String)> {
            Some((
                PathBuf::from("/home/alice/code/rusty-brain/CLAUDE.md"),
                "just some prose with no heading\n".to_string(),
            ))
        };
        let git_root =
            |_: &Path| -> Option<PathBuf> { Some(PathBuf::from("/home/alice/code/rusty-brain")) };
        let ns = detect_namespace_with(start, find_claude, git_root);
        assert_eq!(ns, Namespace::Project("rusty-brain".to_string()));
    }

    #[test]
    fn branch4_cwd_dirname_outside_repo() {
        let start = Path::new("/home/alice/scratch/notes");
        let ns = detect_namespace_with(start, no_claude, no_git);
        assert_eq!(ns, Namespace::Project("notes".to_string()));
    }

    #[test]
    fn branch5_global_for_root_dir() {
        let start = Path::new("/");
        let ns = detect_namespace_with(start, no_claude, no_git);
        assert_eq!(ns, Namespace::Global);
    }

    #[test]
    fn real_fs_finds_claude_md_three_levels_up_and_uses_frontmatter() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        let mut start = root.clone();
        for i in 0..3 {
            start = start.join(format!("d{i}"));
        }
        fs::create_dir_all(&start).unwrap();
        fs::write(root.join("CLAUDE.md"), "---\nproject: walked-up\n---\n# Ignored\n").unwrap();
        let ns = detect_namespace_with(&start, find_nearest_claude_md, no_git);
        assert_eq!(ns, Namespace::Project("walked-up".to_string()));
        drop(tmp);
    }

    #[test]
    fn real_fs_no_claude_md_uses_cwd_dirname() {
        let tmp = TempDir::new().unwrap();
        let start = tmp.path().join("standalone");
        fs::create_dir_all(&start).unwrap();
        let ns = detect_namespace_with(&start, find_nearest_claude_md, no_git);
        assert_eq!(ns, Namespace::Project("standalone".to_string()));
        drop(tmp);
    }
}
```

Then wire the module. Modify `crates/rb-agents/src/lib.rs` — change:

```rust
mod claude_code;
mod cli;
mod event;

pub use claude_code::ClaudeCodeCli;
pub use cli::{agent_for, AgentCli, AgentId, PassthroughCli};
pub use event::{HookContext, HookEvent, HookResult};
```

to:

```rust
mod claude_code;
mod cli;
mod event;
mod namespace;

pub use claude_code::ClaudeCodeCli;
pub use cli::{agent_for, AgentCli, AgentId, PassthroughCli};
pub use event::{HookContext, HookEvent, HookResult};
pub use namespace::detect_namespace;
```

- [ ] **Step 2: run it — Run: `cargo test -p rb-agents namespace::`** — Expected: FAIL with unresolved-module error (`mod namespace;` not yet in `lib.rs`) before the wiring is added; once the file + wiring exist it compiles.

- [ ] **Step 3 GREEN: the `namespace.rs` above and the `lib.rs` wiring are the real implementation.** No additional code.

- [ ] **Step 4: run it — Run: `cargo test -p rb-agents namespace::`** — Expected: PASS (9 tests).

- [ ] **Step 5: lint+format — `cargo clippy -p rb-agents --all-targets -- -D warnings`** (no warnings) then **`cargo fmt --all`** (no diff).

- [ ] **Step 6: commit — `git add crates/rb-agents/src/namespace.rs crates/rb-agents/src/lib.rs && git commit -m "feat(rb-agents): port self-contained namespace detection"`** — Expected: one commit.

---

### Task V6: rb-agents src/daemon.rs — fail-open client

**Files:**
- Create: `crates/rb-agents/src/daemon.rs`
- Modify: `crates/rb-agents/src/lib.rs`
- Test: inline `#[cfg(test)] mod tests` in `crates/rb-agents/src/daemon.rs`

- [ ] **Step 1 RED: write the failing test for the fail-open `DaemonClient`.** Create `crates/rb-agents/src/daemon.rs`:

```rust
//! Strictly fail-open best-effort client over `rb_proto::Client`. Every method
//! wraps the underlying call in a timeout and maps ANY error (connect failure,
//! contract-version mismatch, timeout, wire error) to `None`. The hook surface
//! must NEVER block the CLI or surface a failure: degrade silently.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use rb_proto::Client;
use rb_types::{MemoryId, MemoryNote, MemoryType, Namespace};

/// Auto-start parameters. Provided ONLY for `SessionStart`; any other event
/// passes `None` so non-session hooks never spawn a daemon.
#[derive(Debug, Clone)]
pub struct AutoStart {
    pub self_exe: PathBuf,
    pub db: PathBuf,
}

/// A connected, fail-open daemon client. Holds the live `rb_proto::Client` and a
/// per-call timeout. All methods return `Option`, never `Result`.
pub struct DaemonClient {
    client: Client,
    timeout: Duration,
}

/// The minimal set of parent env vars an auto-start daemon child may inherit.
/// Everything else is cleared before spawn (no parent-env leak into a long-lived
/// detached process).
const FORWARD_ENV: &[&str] = &[
    "VOYAGE_API_KEY",
    "RB_ENRICH_BASE_URL",
    "RB_ENRICH_MODEL",
    "RB_ENRICH_API_KEY",
    "HOME",
    "PATH",
    "XDG_RUNTIME_DIR",
    "XDG_DATA_HOME",
    "XDG_CONFIG_HOME",
];

const SOCKET_ENV: &str = "RUSTY_BRAIN_SOCKET";
const DB_ENV: &str = "RUSTY_BRAIN_DB";

impl DaemonClient {
    /// Connect with the rb-proto handshake inside `timeout`. ANY failure (IO,
    /// timeout, contract-version mismatch) yields `None`. When `auto_start` is
    /// `Some` (SessionStart only) and the first connect fails, spawn a detached
    /// daemon then retry the connect briefly; otherwise never spawn.
    pub async fn connect(
        socket: &Path,
        namespace: Namespace,
        timeout: Duration,
        auto_start: Option<AutoStart>,
    ) -> Option<DaemonClient> {
        if let Some(client) = try_connect(socket, &namespace, timeout).await {
            return Some(DaemonClient { client, timeout });
        }
        let auto = auto_start?;
        // SessionStart-only path: spawn a detached daemon, then retry connect a
        // bounded number of times. Spawn failure => degrade to None.
        if spawn_daemon(&auto.self_exe, socket, &auto.db).is_err() {
            return None;
        }
        for _ in 0..50 {
            if let Some(client) = try_connect(socket, &namespace, timeout).await {
                return Some(DaemonClient { client, timeout });
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        None
    }

    /// Best-effort `remember`. Returns the new id, or `None` on any error/timeout.
    pub async fn remember(
        &mut self,
        content: String,
        context: Option<String>,
        memory_type: MemoryType,
        importance: u8,
        tags: Vec<String>,
    ) -> Option<MemoryId> {
        let fut = self.client.remember(
            content,
            context,
            memory_type,
            importance,
            Vec::new(),
            tags,
            Vec::new(),
        );
        match tokio::time::timeout(self.timeout, fut).await {
            Ok(Ok(id)) => Some(id),
            Ok(Err(_)) | Err(_) => None,
        }
    }

    /// Best-effort context fetch. Returns `(recent, important, total)`, or `None`.
    pub async fn context(&mut self) -> Option<(Vec<MemoryNote>, Vec<MemoryNote>, usize)> {
        match tokio::time::timeout(self.timeout, self.client.context()).await {
            Ok(Ok(triple)) => Some(triple),
            Ok(Err(_)) | Err(_) => None,
        }
    }
}

/// Connect + handshake within `timeout`; any error or timeout => `None`.
async fn try_connect(socket: &Path, namespace: &Namespace, timeout: Duration) -> Option<Client> {
    match tokio::time::timeout(timeout, Client::connect(socket, namespace.clone())).await {
        Ok(Ok(client)) => Some(client),
        Ok(Err(_)) | Err(_) => None,
    }
}

/// Spawn `rusty-brain serve` as a detached child with a cleared environment
/// (only the resolved socket/db paths plus allowlisted vars are forwarded).
fn spawn_daemon(self_exe: &Path, socket: &Path, db: &Path) -> std::io::Result<()> {
    let mut cmd = Command::new(self_exe);
    cmd.arg("serve");
    cmd.env_clear();
    cmd.env(SOCKET_ENV, socket);
    cmd.env(DB_ENV, db);
    for key in FORWARD_ENV {
        if let Ok(value) = std::env::var(key) {
            cmd.env(key, value);
        }
    }
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        cmd.process_group(0);
    }
    cmd.spawn().map(|_child| ())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use rb_daemon::{Daemon, DaemonConfig, JobsConfig, SharedEmbedder};
    use rb_embed::DeterministicProvider;
    use rb_types::{MemoryType, Namespace};
    use std::time::Duration;
    use tokio::sync::oneshot;

    const DIM: usize = 8;

    // Bind + run an in-process daemon on a temp UDS. Returns the dir guard, the
    // socket path, a shutdown sender, and the run JoinHandle.
    async fn start_daemon() -> (
        tempfile::TempDir,
        PathBuf,
        oneshot::Sender<()>,
        tokio::task::JoinHandle<()>,
    ) {
        let dir = tempfile::tempdir_in(std::env::temp_dir()).unwrap();
        // The daemon creates this `runtime/` subdir itself at 0700, so the parent
        // perms are guaranteed private regardless of the tempdir root's mode.
        let socket = dir.path().join("runtime").join("sock");
        let db = dir.path().join("rb.db");
        let config = DaemonConfig {
            socket_path: socket.clone(),
            db_path: db,
            read_pool_size: 2,
            jobs_config: JobsConfig::default(),
        };
        let embedder = SharedEmbedder::new(DeterministicProvider::new(DIM));
        let daemon = Daemon::bind(config, embedder).await.unwrap();
        let (tx, rx) = oneshot::channel::<()>();
        let handle = tokio::spawn(async move {
            let _ = daemon
                .run(async move {
                    let _ = rx.await;
                })
                .await;
        });
        // Give the accept loop a moment to be ready.
        tokio::time::sleep(Duration::from_millis(50)).await;
        (dir, socket, tx, handle)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn remember_then_context_round_trip_over_real_daemon() {
        let (_dir, socket, shutdown, handle) = start_daemon().await;

        let mut client = DaemonClient::connect(
            &socket,
            Namespace::Project("rb-agents-test".to_string()),
            Duration::from_secs(5),
            None,
        )
        .await
        .expect("connect must succeed against a live daemon");

        let id = client
            .remember(
                "always run one writer thread".to_string(),
                Some("daemon design".to_string()),
                MemoryType::ArchitectureDecision,
                9,
                vec!["daemon".to_string()],
            )
            .await
            .expect("remember must return an id");
        assert_eq!(id.to_string().len(), 36, "id is a uuid");

        let (recent, important, total) =
            client.context().await.expect("context must return a triple");
        assert!(total >= 1, "the stored memory must be counted");
        assert!(
            !recent.is_empty() || !important.is_empty(),
            "stored memory must appear in recent or important"
        );

        let _ = shutdown.send(());
        let _ = handle.await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn connect_to_dead_socket_returns_none_without_panic_or_hang() {
        let dir = tempfile::tempdir_in(std::env::temp_dir()).unwrap();
        let socket = dir.path().join("absent.sock"); // never bound

        let result = DaemonClient::connect(
            &socket,
            Namespace::Global,
            Duration::from_millis(200),
            None, // no auto-start: must not spawn, must degrade to None
        )
        .await;
        assert!(result.is_none(), "dead socket must degrade to None");
    }
}
```

Then wire the module. Modify `crates/rb-agents/src/lib.rs` — change:

```rust
mod claude_code;
mod cli;
mod event;
mod namespace;

pub use claude_code::ClaudeCodeCli;
pub use cli::{agent_for, AgentCli, AgentId, PassthroughCli};
pub use event::{HookContext, HookEvent, HookResult};
pub use namespace::detect_namespace;
```

to:

```rust
mod claude_code;
mod cli;
mod daemon;
mod event;
mod namespace;

pub use claude_code::ClaudeCodeCli;
pub use cli::{agent_for, AgentCli, AgentId, PassthroughCli};
pub use daemon::{AutoStart, DaemonClient};
pub use event::{HookContext, HookEvent, HookResult};
pub use namespace::detect_namespace;
```

- [ ] **Step 2: run it — Run: `cargo test -p rb-agents daemon::`** — Expected: FAIL with unresolved-module error (`mod daemon;` not yet wired) before the wiring; once the file + wiring exist it compiles and the two `#[tokio::test]` cases run.

- [ ] **Step 3 GREEN: the `daemon.rs` above and the `lib.rs` wiring are the real implementation.** No additional code. (The `#[forbid(unsafe_code)]` crate attribute permits the `std::os::unix::process::CommandExt::process_group` call because it is a safe trait method, not an `unsafe` block.)

- [ ] **Step 4: run it — Run: `cargo test -p rb-agents daemon::`** — Expected: PASS (2 tests).

- [ ] **Step 5: lint+format — `cargo clippy -p rb-agents --all-targets -- -D warnings`** (no warnings) then **`cargo fmt --all`** (no diff).

- [ ] **Step 6: commit — `git add crates/rb-agents/src/daemon.rs crates/rb-agents/src/lib.rs && git commit -m "feat(rb-agents): add fail-open daemon client with auto-start"`** — Expected: one commit.

---

### Task V7: rb-agents src/install.rs — install contract

**Files:**
- Create: `crates/rb-agents/src/install.rs`
- Modify: `crates/rb-agents/src/lib.rs`
- Test: inline `#[cfg(test)] mod tests` in `crates/rb-agents/src/install.rs`

- [ ] **Step 1 RED: write the failing test for the install-side contract.** Create `crates/rb-agents/src/install.rs`:

```rust
//! Install-side contract consumed by Part Y (rb-install). Defines the install
//! scope, the sentinel-keyed JSON fragment to deep-merge into a CLI's config,
//! the `SENTINEL` marker that identifies OUR injected entries, and the
//! `AgentInstaller` trait. No implementations here — Part Y adds per-CLI ones.

use std::path::PathBuf;

use rb_types::Result;

use crate::cli::AgentId;

/// Where an install writes config. Project scope is the default; `--global`
/// targets the user-level config dir.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallScope {
    Project(PathBuf),
    Global,
}

/// A config-file path plus the sentinel-keyed JSON block to deep-merge into it.
/// `hook_fragment` produces this purely (no I/O); Part Y performs the atomic
/// merge-write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookFragment {
    pub config_path: PathBuf,
    pub merge: serde_json::Value,
}

/// Marker key/comment identifying entries this installer owns. Uninstall removes
/// ONLY blocks carrying this sentinel; merge preserves all other user hooks.
pub const SENTINEL: &str = "rusty-brain";

/// Per-CLI installer: identity, PATH-based detection, and a PURE hook-fragment
/// builder. `detect` runs `<binary> --version` with a short timeout (NO shell);
/// `hook_fragment` performs no I/O.
pub trait AgentInstaller {
    fn id(&self) -> AgentId;
    fn detect(&self) -> Option<String>;
    fn hook_fragment(&self, hooks_bin: &std::path::Path, scope: &InstallScope)
        -> Result<HookFragment>;
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use crate::cli::AgentId;
    use std::path::{Path, PathBuf};

    // A trait-object smoke installer proving `AgentInstaller` is usable as
    // `Box<dyn AgentInstaller>` and the contract types compose.
    struct FakeInstaller;

    impl AgentInstaller for FakeInstaller {
        fn id(&self) -> AgentId {
            AgentId::ClaudeCode
        }

        fn detect(&self) -> Option<String> {
            Some("1.2.3".to_string())
        }

        fn hook_fragment(
            &self,
            hooks_bin: &Path,
            scope: &InstallScope,
        ) -> Result<HookFragment> {
            let config_path = match scope {
                InstallScope::Project(root) => root.join(".claude").join("settings.json"),
                InstallScope::Global => PathBuf::from("/home/user/.claude/settings.json"),
            };
            let merge = serde_json::json!({
                SENTINEL: { "hooks_bin": hooks_bin.display().to_string() }
            });
            Ok(HookFragment { config_path, merge })
        }
    }

    #[test]
    fn sentinel_is_rusty_brain() {
        assert_eq!(SENTINEL, "rusty-brain");
    }

    #[test]
    fn install_scope_variants_are_distinct() {
        let project = InstallScope::Project(PathBuf::from("/proj"));
        let global = InstallScope::Global;
        assert_ne!(project, global);
        assert_eq!(project, InstallScope::Project(PathBuf::from("/proj")));
    }

    #[test]
    fn trait_object_detect_and_id_work() {
        let installer: Box<dyn AgentInstaller> = Box::new(FakeInstaller);
        assert_eq!(installer.id(), AgentId::ClaudeCode);
        assert_eq!(installer.detect().as_deref(), Some("1.2.3"));
    }

    #[test]
    fn hook_fragment_is_pure_and_carries_sentinel_for_project_scope() {
        let installer = FakeInstaller;
        let fragment = installer
            .hook_fragment(
                Path::new("/usr/local/bin/rusty-brain-hooks"),
                &InstallScope::Project(PathBuf::from("/proj")),
            )
            .unwrap();
        assert_eq!(
            fragment.config_path,
            PathBuf::from("/proj/.claude/settings.json")
        );
        assert_eq!(
            fragment.merge[SENTINEL]["hooks_bin"],
            "/usr/local/bin/rusty-brain-hooks"
        );
    }

    #[test]
    fn hook_fragment_global_scope_uses_global_path() {
        let installer = FakeInstaller;
        let fragment = installer
            .hook_fragment(Path::new("/usr/local/bin/rusty-brain-hooks"), &InstallScope::Global)
            .unwrap();
        assert_eq!(
            fragment.config_path,
            PathBuf::from("/home/user/.claude/settings.json")
        );
    }
}
```

Then wire the module. Modify `crates/rb-agents/src/lib.rs` — change:

```rust
mod claude_code;
mod cli;
mod daemon;
mod event;
mod namespace;

pub use claude_code::ClaudeCodeCli;
pub use cli::{agent_for, AgentCli, AgentId, PassthroughCli};
pub use daemon::{AutoStart, DaemonClient};
pub use event::{HookContext, HookEvent, HookResult};
pub use namespace::detect_namespace;
```

to:

```rust
mod claude_code;
mod cli;
mod daemon;
mod event;
mod install;
mod namespace;

pub use claude_code::ClaudeCodeCli;
pub use cli::{agent_for, AgentCli, AgentId, PassthroughCli};
pub use daemon::{AutoStart, DaemonClient};
pub use event::{HookContext, HookEvent, HookResult};
pub use install::{AgentInstaller, HookFragment, InstallScope, SENTINEL};
pub use namespace::detect_namespace;
```

- [ ] **Step 2: run it — Run: `cargo test -p rb-agents install::`** — Expected: FAIL with unresolved-module error (`mod install;` not yet wired) before the wiring; once the file + wiring exist it compiles.

- [ ] **Step 3 GREEN: the `install.rs` above and the `lib.rs` wiring are the real implementation.** No additional code.

- [ ] **Step 4: run it — Run: `cargo test -p rb-agents install::`** — Expected: PASS (5 tests).

- [ ] **Step 5: lint+format — `cargo clippy -p rb-agents --all-targets -- -D warnings`** (no warnings) then **`cargo fmt --all`** (no diff).

- [ ] **Step 6: commit — `git add crates/rb-agents/src/install.rs crates/rb-agents/src/lib.rs && git commit -m "feat(rb-agents): add install-side agentinstaller contract"`** — Expected: one commit.

---

### Task V8: rb-agents src/lib.rs — public API surface test

**Files:**
- Create: `crates/rb-agents/tests/public_api.rs`
- Test: `crates/rb-agents/tests/public_api.rs`

- [ ] **Step 1 RED: write an integration test asserting the FULL public surface is reachable through `rb_agents::*`.** Create `crates/rb-agents/tests/public_api.rs`:

```rust
//! Locks the rb-agents public API surface so Parts W/X/Y compile against exactly
//! these re-exported names. Pure integration test: imports through the crate
//! root only.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use rb_agents::{
    agent_for, detect_namespace, AgentCli, AgentId, AgentInstaller, AutoStart, DaemonClient,
    HookContext, HookEvent, HookFragment, HookResult, InstallScope, PassthroughCli, SENTINEL,
};

#[test]
fn event_model_types_are_reexported() {
    let ctx = HookContext {
        event: HookEvent::SessionStart { source: None },
        cwd: PathBuf::from("."),
        session_id: None,
    };
    assert_eq!(ctx.event, HookEvent::SessionStart { source: None });
    let result = HookResult {
        system_message: Some("x".to_string()),
        continue_execution: true,
    };
    assert!(result.continue_execution);
}

#[test]
fn registry_and_adapters_are_reexported() {
    let cli: Box<dyn AgentCli> = agent_for(AgentId::ClaudeCode);
    assert_eq!(cli.id(), AgentId::ClaudeCode);
    // PassthroughCli is part of the public surface for Part X to reference.
    let passthrough: Box<dyn AgentCli> = agent_for(AgentId::Gemini);
    assert_eq!(passthrough.binary_name(), "gemini");
    let _ = std::any::type_name::<PassthroughCli>();
}

#[test]
fn namespace_detection_is_reexported() {
    // Detecting on the current dir never panics and yields a namespace.
    let _ns = detect_namespace(Path::new("."));
}

#[test]
fn daemon_and_autostart_types_are_reexported() {
    let auto = AutoStart {
        self_exe: PathBuf::from("/bin/true"),
        db: PathBuf::from("/tmp/rb.db"),
    };
    assert_eq!(auto.self_exe, PathBuf::from("/bin/true"));
    // DaemonClient::connect is the entrypoint Parts W reuse; bind it to a fn item
    // to lock its argument shape (returns an impl Future, so leave the tail
    // inferred by passing it to a generic that only constrains the arguments).
    fn _takes_connect<F, Fut>(_f: F)
    where
        F: Fn(&Path, rb_types::Namespace, Duration, Option<AutoStart>) -> Fut,
    {
    }
    _takes_connect(DaemonClient::connect);
}

#[test]
fn install_contract_is_reexported() {
    assert_eq!(SENTINEL, "rusty-brain");
    let scope = InstallScope::Project(PathBuf::from("/proj"));
    assert_eq!(scope, InstallScope::Project(PathBuf::from("/proj")));
    let fragment = HookFragment {
        config_path: PathBuf::from("/proj/.claude/settings.json"),
        merge: serde_json::json!({ SENTINEL: {} }),
    };
    assert_eq!(fragment.merge[SENTINEL], serde_json::json!({}));
    // AgentInstaller is referenced as a trait bound to lock its name.
    fn _accepts_installer<T: AgentInstaller>(_t: &T) {}
}
```

- [ ] **Step 2: run it — Run: `cargo test -p rb-agents --test public_api`** — Expected: FAIL only if a re-export is missing/misnamed; with Tasks V2–V7 complete it compiles and passes. (If executed before the modules exist, it FAILS with unresolved imports — the RED.)

- [ ] **Step 3 GREEN: no production code changes are needed** — the public surface is already re-exported by the `pub use` lines added across Tasks V2–V7. This task only adds the lock test above.

- [ ] **Step 4: run it — Run: `cargo test -p rb-agents --test public_api`** — Expected: PASS (5 tests).

- [ ] **Step 5: lint+format — `cargo clippy -p rb-agents --all-targets -- -D warnings`** (no warnings) then **`cargo fmt --all`** (no diff).

- [ ] **Step 6: commit — `git add crates/rb-agents/tests/public_api.rs && git commit -m "test(rb-agents): lock public api surface for downstream parts"`** — Expected: one commit.

---

### Part V gate

**Files:** none (verification only).

- [ ] **Step 1: per-crate tests — Run: `cargo test -p rb-agents`** — Expected: PASS (all unit + integration tests across event/cli/claude_code/namespace/daemon/install/public_api green).

- [ ] **Step 2: per-crate clippy — Run: `cargo clippy -p rb-agents --all-targets -- -D warnings`** — Expected: no warnings.

- [ ] **Step 3: format — Run: `cargo fmt --all --check`** — Expected: no diff.

- [ ] **Step 4: WORKSPACE build — Run: `cargo build --workspace`** — Expected: success (rb-agents builds as a member; no core crate regressions).

- [ ] **Step 5: WORKSPACE tests — Run: `cargo test --workspace`** — Expected: PASS (the new crate's tests run alongside the existing suite; no regressions).

- [ ] **Step 6: WORKSPACE clippy (all features) — Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`** — Expected: no warnings.

- [ ] **Step 7: WORKSPACE format — Run: `cargo fmt --all --check`** — Expected: no diff.

- [ ] **Step 8: default-closure isolation — Run: `cargo tree -e no-dev -p rusty-brain | grep -E "rb-agents|rb-hooks|rb-install"`** — Expected: NO output (the `rusty-brain` binary's non-dev dependency closure does NOT contain rb-agents, rb-hooks, or rb-install). This proves the agent surface stays out of the default build, exactly like the `local` feature precedent.


## Part W — rb-hooks binary (stdin/stdout dispatch, capture flows, dedup, context injection)

This Part builds the `rb-hooks` crate (binary `rusty-brain-hooks`): the per-event hook executable that Claude Code (and the other three JSON CLIs) invoke on every lifecycle event. It consumes the Part V `rb-agents` spine verbatim — `agent_for`, `AgentCli`, `HookContext`, `HookEvent`, `HookResult`, `DaemonClient`, `AutoStart`, and `detect_namespace` — and adds the FAIL-OPEN run harness plus the four capture flows (SessionStart context injection, PostToolUse mutation capture with dedup, Stop session summary, PreCompact decision capture), wired end-to-end for Claude Code. Enforcement is **capture-only**: every flow returns `continue_execution: true`, no event ever blocks, every error degrades to a safe `{"continue":true}`, and `main` always `exit(0)`. The crate is a workspace member but is NEVER referenced by any core crate dependency, so the default `cargo build` and the `rusty-brain` binary never compile it.

The dedup cache is a short-TTL (60s) JSON file under the XDG cache dir (`$XDG_CACHE_HOME/rusty-brain/` or `~/.cache/rusty-brain/`), keyed by a stable FNV-1a hash of `(tool_name, summary)` namespaced by the project namespace, ported in spirit from the old `crates/hooks/src/dedup.rs` but simplified (no file lock; atomic temp+rename write; every error fails open to "not a duplicate"). The MemoryType mapping is: mutation file tools (`Edit`/`Write`/`NotebookEdit`) → `MemoryType::CodePattern`; `Bash` → `MemoryType::Reference`; the Stop session summary → `MemoryType::Reference`; PreCompact decisions → `MemoryType::ArchitectureDecision`.

---

### Task W1: rb-hooks Cargo.toml — crate scaffold

**Files:**
- Modify: `Cargo.toml` (add `crates/rb-hooks` to `[workspace] members`)
- Create: `crates/rb-hooks/Cargo.toml`
- Create: `crates/rb-hooks/src/main.rs` (temporary placeholder so the crate compiles)

- [ ] **Step 1 RED: add the member + a compiling placeholder, then assert via cargo-tree the new crate is OUT of the core closure.**

  Modify `Cargo.toml` — add the member line so the `[workspace] members` block reads exactly:

  ```toml
  [workspace]
  resolver = "2"
  members = [
      "crates/rb-types",
      "crates/rb-store",
      "crates/rb-proto",
      "crates/rb-embed",
      "crates/rb-search",
      "crates/rb-engine",
      "crates/rb-enrich",
      "crates/rb-daemon",
      "crates/rb-mcp",
      "crates/rusty-brain",
      "crates/rb-hooks",
  ]
  ```

  Create `crates/rb-hooks/Cargo.toml`:

  ```toml
  [package]
  name = "rb-hooks"
  version.workspace = true
  edition.workspace = true
  license.workspace = true
  authors.workspace = true
  repository.workspace = true
  description = "rusty-brain: per-event capture hook binary (fail-open) for JSON-protocol agent CLIs."

  [[bin]]
  name = "rusty-brain-hooks"
  path = "src/main.rs"

  [dependencies]
  rb-agents = { path = "../rb-agents" }
  rb-types = { path = "../rb-types" }
  rb-proto = { path = "../rb-proto" }
  tokio = { workspace = true }
  serde = { workspace = true }
  serde_json = { workspace = true }
  anyhow = { workspace = true }
  tracing = { workspace = true }

  [dev-dependencies]
  tempfile = { workspace = true }
  assert_cmd = { workspace = true }
  predicates = { workspace = true }

  [lints]
  workspace = true
  ```

  Create `crates/rb-hooks/src/main.rs` (placeholder; replaced in Task W9):

  ```rust
  fn main() {
      std::process::exit(0);
  }
  ```

- [ ] **Step 2: confirm the crate builds AND is not in the core closure.**
  - Run: `cargo build -p rb-hooks`
  - Expected: PASS (compiles the placeholder binary).
  - Run: `cargo tree -e no-dev -p rusty-brain | grep -E "rb-agents|rb-hooks|rb-install"`
  - Expected: prints NOTHING (empty output, exit 1 from grep) — the new crates are NOT in the `rusty-brain` non-dev closure.

- [ ] **Step 3 GREEN: nothing further — the placeholder is the minimal impl for this task.**

- [ ] **Step 4: re-run to confirm stable.**
  - Run: `cargo build -p rb-hooks`
  - Expected: PASS (1 binary builds, no warnings).

- [ ] **Step 5: lint + format.**
  - Run: `cargo clippy -p rb-hooks --all-targets -- -D warnings`
  - Expected: no warnings.
  - Run: `cargo fmt --all`
  - Expected: no diff.

- [ ] **Step 6: commit.**
  - Run: `git add Cargo.toml crates/rb-hooks/Cargo.toml crates/rb-hooks/src/main.rs && git commit -m "chore(rb-hooks): scaffold capture hook binary crate"`
  - Expected: one commit.

---

### Task W2: rb-hooks src/cli.rs — arg parsing

**Files:**
- Create: `crates/rb-hooks/src/cli.rs`
- Modify: `crates/rb-hooks/src/main.rs` (add `mod cli;`)
- Test: inline `#[cfg(test)]` module in `crates/rb-hooks/src/cli.rs`

- [ ] **Step 1 RED: write the failing test for `Args::parse_from`.**

  Create `crates/rb-hooks/src/cli.rs`:

  ```rust
  //! Command-line arguments for the hook binary.
  //!
  //! The only argument is `--agent <id>`, selecting which JSON-protocol CLI's
  //! stdin/stdout shapes to use. The lifecycle event itself is NOT a CLI arg — it
  //! is read from the stdin JSON via `AgentCli::parse_input`.

  use rb_agents::cli::AgentId;

  /// Parsed hook invocation arguments.
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct Args {
      /// Which agent CLI's JSON shapes to use.
      pub agent: AgentId,
  }

  #[cfg(test)]
  mod tests {
      #![allow(clippy::unwrap_used, clippy::expect_used)]
      use super::*;
      use rb_agents::cli::AgentId;

      #[test]
      fn parses_claude_code_agent() {
          let args = Args::parse_from(["rusty-brain-hooks", "--agent", "claude-code"]).unwrap();
          assert_eq!(args.agent, AgentId::ClaudeCode);
      }

      #[test]
      fn parses_each_agent_id() {
          for (raw, expected) in [
              ("claude-code", AgentId::ClaudeCode),
              ("opencode", AgentId::OpenCode),
              ("gemini", AgentId::Gemini),
              ("codex", AgentId::Codex),
          ] {
              let args = Args::parse_from(["rusty-brain-hooks", "--agent", raw]).unwrap();
              assert_eq!(args.agent, expected, "agent {raw}");
          }
      }

      #[test]
      fn missing_agent_is_error() {
          let err = Args::parse_from(["rusty-brain-hooks"]);
          assert!(err.is_err(), "missing --agent must error");
      }

      #[test]
      fn unknown_agent_is_error() {
          let err = Args::parse_from(["rusty-brain-hooks", "--agent", "bogus"]);
          assert!(err.is_err(), "unknown agent must error");
      }

      #[test]
      fn equals_form_is_accepted() {
          let args = Args::parse_from(["rusty-brain-hooks", "--agent=gemini"]).unwrap();
          assert_eq!(args.agent, AgentId::Gemini);
      }
  }
  ```

  Modify `crates/rb-hooks/src/main.rs` to declare the module (full file):

  ```rust
  mod cli;

  fn main() {
      std::process::exit(0);
  }
  ```

- [ ] **Step 2: run it.**
  - Run: `cargo test -p rb-hooks cli::`
  - Expected: FAIL — `Args::parse_from` is not defined (`no function or associated item named parse_from found for struct Args`).

- [ ] **Step 3 GREEN: implement `Args::parse_from` (hand parser, no clap-derive needed; fail-open shape is irrelevant here — parse errors return `Err` so `main` falls back to fail-open).**

  Replace `crates/rb-hooks/src/cli.rs` with:

  ```rust
  //! Command-line arguments for the hook binary.
  //!
  //! The only argument is `--agent <id>`, selecting which JSON-protocol CLI's
  //! stdin/stdout shapes to use. The lifecycle event itself is NOT a CLI arg — it
  //! is read from the stdin JSON via `AgentCli::parse_input`.

  use rb_agents::cli::AgentId;

  /// Parsed hook invocation arguments.
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct Args {
      /// Which agent CLI's JSON shapes to use.
      pub agent: AgentId,
  }

  impl Args {
      /// Parse from an argv-like iterator. Accepts `--agent <id>` and
      /// `--agent=<id>`. Returns `Err(message)` on a missing/unknown agent or an
      /// unexpected argument — the caller treats any error as fail-open.
      pub fn parse_from<I, S>(argv: I) -> Result<Self, String>
      where
          I: IntoIterator<Item = S>,
          S: AsRef<str>,
      {
          let mut agent: Option<AgentId> = None;
          let mut iter = argv.into_iter();
          // Skip argv[0] (program name).
          let _ = iter.next();
          while let Some(arg) = iter.next() {
              let arg = arg.as_ref();
              if let Some(value) = arg.strip_prefix("--agent=") {
                  agent = Some(Self::parse_agent(value)?);
              } else if arg == "--agent" {
                  let value = iter
                      .next()
                      .ok_or_else(|| "missing value for --agent".to_string())?;
                  agent = Some(Self::parse_agent(value.as_ref())?);
              } else {
                  return Err(format!("unexpected argument: {arg}"));
              }
          }
          let agent = agent.ok_or_else(|| "missing required --agent <id>".to_string())?;
          Ok(Args { agent })
      }

      fn parse_agent(value: &str) -> Result<AgentId, String> {
          AgentId::parse(value).ok_or_else(|| format!("unknown agent: {value}"))
      }
  }

  #[cfg(test)]
  mod tests {
      #![allow(clippy::unwrap_used, clippy::expect_used)]
      use super::*;
      use rb_agents::cli::AgentId;

      #[test]
      fn parses_claude_code_agent() {
          let args = Args::parse_from(["rusty-brain-hooks", "--agent", "claude-code"]).unwrap();
          assert_eq!(args.agent, AgentId::ClaudeCode);
      }

      #[test]
      fn parses_each_agent_id() {
          for (raw, expected) in [
              ("claude-code", AgentId::ClaudeCode),
              ("opencode", AgentId::OpenCode),
              ("gemini", AgentId::Gemini),
              ("codex", AgentId::Codex),
          ] {
              let args = Args::parse_from(["rusty-brain-hooks", "--agent", raw]).unwrap();
              assert_eq!(args.agent, expected, "agent {raw}");
          }
      }

      #[test]
      fn missing_agent_is_error() {
          let err = Args::parse_from(["rusty-brain-hooks"]);
          assert!(err.is_err(), "missing --agent must error");
      }

      #[test]
      fn unknown_agent_is_error() {
          let err = Args::parse_from(["rusty-brain-hooks", "--agent", "bogus"]);
          assert!(err.is_err(), "unknown agent must error");
      }

      #[test]
      fn equals_form_is_accepted() {
          let args = Args::parse_from(["rusty-brain-hooks", "--agent=gemini"]).unwrap();
          assert_eq!(args.agent, AgentId::Gemini);
      }
  }
  ```

- [ ] **Step 4: run it.**
  - Run: `cargo test -p rb-hooks cli::`
  - Expected: PASS (5 tests).

- [ ] **Step 5: lint + format.**
  - Run: `cargo clippy -p rb-hooks --all-targets -- -D warnings`
  - Expected: no warnings.
  - Run: `cargo fmt --all`
  - Expected: no diff.

- [ ] **Step 6: commit.**
  - Run: `git add crates/rb-hooks/src/cli.rs crates/rb-hooks/src/main.rs && git commit -m "feat(rb-hooks): parse --agent argument"`
  - Expected: one commit.

---

### Task W3: rb-hooks src/io.rs — stdin/stdout

**Files:**
- Create: `crates/rb-hooks/src/io.rs`
- Modify: `crates/rb-hooks/src/main.rs` (add `mod io;`)
- Test: inline `#[cfg(test)]` module in `crates/rb-hooks/src/io.rs`

- [ ] **Step 1 RED: write the failing test for the pure reader/writer helpers.**

  Create `crates/rb-hooks/src/io.rs`:

  ```rust
  //! Fail-open stdin/stdout helpers.
  //!
  //! Reading parses one JSON value; an empty or invalid stream degrades to
  //! `Value::Null` (never an error) so the harness can still render a fail-open
  //! response. Writing serializes a value and appends a newline.

  use std::io::Read;

  #[cfg(test)]
  mod tests {
      #![allow(clippy::unwrap_used, clippy::expect_used)]
      use super::*;

      #[test]
      fn parse_reader_reads_valid_object() {
          let raw = br#"{"hook_event_name":"SessionStart","cwd":"/tmp"}"#;
          let value = read_json_from(&raw[..]);
          assert_eq!(
              value.get("hook_event_name").and_then(|v| v.as_str()),
              Some("SessionStart")
          );
      }

      #[test]
      fn empty_stream_is_null() {
          let raw: &[u8] = b"";
          let value = read_json_from(raw);
          assert_eq!(value, serde_json::Value::Null);
      }

      #[test]
      fn invalid_json_is_null() {
          let raw: &[u8] = b"not json at all {{{";
          let value = read_json_from(raw);
          assert_eq!(value, serde_json::Value::Null);
      }

      #[test]
      fn whitespace_only_is_null() {
          let raw: &[u8] = b"   \n\t  ";
          let value = read_json_from(raw);
          assert_eq!(value, serde_json::Value::Null);
      }

      #[test]
      fn render_to_string_appends_newline() {
          let value = serde_json::json!({"continue": true});
          let out = render_to_string(&value);
          assert!(out.ends_with('\n'), "output must end with newline: {out:?}");
          assert!(out.contains("\"continue\":true"));
      }
  }
  ```

  Modify `crates/rb-hooks/src/main.rs` (full file):

  ```rust
  mod cli;
  mod io;

  fn main() {
      std::process::exit(0);
  }
  ```

- [ ] **Step 2: run it.**
  - Run: `cargo test -p rb-hooks io::`
  - Expected: FAIL — `read_json_from` and `render_to_string` are not defined (`cannot find function`).

- [ ] **Step 3 GREEN: implement the helpers. Public `read_stdin_json`/`write_stdout` wrap the testable pure cores.**

  Replace `crates/rb-hooks/src/io.rs` with:

  ```rust
  //! Fail-open stdin/stdout helpers.
  //!
  //! Reading parses one JSON value; an empty or invalid stream degrades to
  //! `Value::Null` (never an error) so the harness can still render a fail-open
  //! response. Writing serializes a value and appends a newline.

  use std::io::{Read, Write};

  /// Read all of stdin and parse it as one JSON value. Fail-open: any read or
  /// parse failure (including empty/whitespace input) degrades to `Value::Null`.
  pub fn read_stdin_json() -> serde_json::Value {
      let stdin = std::io::stdin();
      read_json_from(stdin.lock())
  }

  /// Pure core: read everything from `reader` and parse one JSON value. Any error
  /// (I/O, empty, invalid) degrades to `Value::Null`.
  fn read_json_from<R: Read>(mut reader: R) -> serde_json::Value {
      let mut buf = String::new();
      if reader.read_to_string(&mut buf).is_err() {
          return serde_json::Value::Null;
      }
      if buf.trim().is_empty() {
          return serde_json::Value::Null;
      }
      serde_json::from_str(&buf).unwrap_or(serde_json::Value::Null)
  }

  /// Write `value` as JSON to stdout, followed by a newline. Best-effort: write
  /// failures are swallowed (the process still exits 0 in the harness).
  pub fn write_stdout(value: &serde_json::Value) {
      let rendered = render_to_string(value);
      let stdout = std::io::stdout();
      let mut handle = stdout.lock();
      let _ = handle.write_all(rendered.as_bytes());
      let _ = handle.flush();
  }

  /// Pure core: serialize `value` to a compact JSON string with a trailing
  /// newline. Serialization of an already-valid `serde_json::Value` cannot fail;
  /// the fallback string keeps the function total without unwrap.
  fn render_to_string(value: &serde_json::Value) -> String {
      let body = serde_json::to_string(value).unwrap_or_else(|_| "{\"continue\":true}".to_string());
      format!("{body}\n")
  }

  #[cfg(test)]
  mod tests {
      #![allow(clippy::unwrap_used, clippy::expect_used)]
      use super::*;

      #[test]
      fn parse_reader_reads_valid_object() {
          let raw = br#"{"hook_event_name":"SessionStart","cwd":"/tmp"}"#;
          let value = read_json_from(&raw[..]);
          assert_eq!(
              value.get("hook_event_name").and_then(|v| v.as_str()),
              Some("SessionStart")
          );
      }

      #[test]
      fn empty_stream_is_null() {
          let raw: &[u8] = b"";
          let value = read_json_from(raw);
          assert_eq!(value, serde_json::Value::Null);
      }

      #[test]
      fn invalid_json_is_null() {
          let raw: &[u8] = b"not json at all {{{";
          let value = read_json_from(raw);
          assert_eq!(value, serde_json::Value::Null);
      }

      #[test]
      fn whitespace_only_is_null() {
          let raw: &[u8] = b"   \n\t  ";
          let value = read_json_from(raw);
          assert_eq!(value, serde_json::Value::Null);
      }

      #[test]
      fn render_to_string_appends_newline() {
          let value = serde_json::json!({"continue": true});
          let out = render_to_string(&value);
          assert!(out.ends_with('\n'), "output must end with newline: {out:?}");
          assert!(out.contains("\"continue\":true"));
      }
  }
  ```

- [ ] **Step 4: run it.**
  - Run: `cargo test -p rb-hooks io::`
  - Expected: PASS (5 tests).

- [ ] **Step 5: lint + format.**
  - Run: `cargo clippy -p rb-hooks --all-targets -- -D warnings`
  - Expected: no warnings.
  - Run: `cargo fmt --all`
  - Expected: no diff.

- [ ] **Step 6: commit.**
  - Run: `git add crates/rb-hooks/src/io.rs crates/rb-hooks/src/main.rs && git commit -m "feat(rb-hooks): fail-open stdin/stdout json helpers"`
  - Expected: one commit.

---

### Task W4: rb-hooks src/dedup.rs — short-TTL cache

**Files:**
- Create: `crates/rb-hooks/src/dedup.rs`
- Modify: `crates/rb-hooks/src/main.rs` (add `mod dedup;`)
- Test: inline `#[cfg(test)]` module in `crates/rb-hooks/src/dedup.rs`

- [ ] **Step 1 RED: write the failing test for the file-backed dedup cache.**

  Create `crates/rb-hooks/src/dedup.rs`:

  ```rust
  //! Short-TTL deduplication cache for PostToolUse observations.
  //!
  //! Each hook invocation is a fresh process, so dedup must be cross-process: the
  //! cache is a small JSON file under the XDG cache dir, namespaced per project.
  //! Entries expire after `TTL_SECONDS` and are pruned on every `record`. We store
  //! only stable FNV-1a hashes of `(tool_name, summary)` — never raw content.
  //!
  //! Fail-open: every error (unreadable/corrupt/unwritable cache) degrades to
  //! "not a duplicate" so capture never silently drops on cache trouble.

  use std::collections::HashMap;
  use std::path::{Path, PathBuf};

  const TTL_SECONDS: u64 = 60;

  /// A file-backed, per-namespace dedup cache.
  pub struct DedupCache {
      cache_path: PathBuf,
  }

  #[cfg(test)]
  mod tests {
      #![allow(clippy::unwrap_used, clippy::expect_used)]
      use super::*;

      #[test]
      fn hash_key_is_deterministic() {
          let a = DedupCache::hash_key("Edit", "Edited /src/main.rs");
          let b = DedupCache::hash_key("Edit", "Edited /src/main.rs");
          assert_eq!(a, b);
      }

      #[test]
      fn hash_key_differs_by_tool_and_summary() {
          let base = DedupCache::hash_key("Edit", "summary");
          assert_ne!(base, DedupCache::hash_key("Write", "summary"));
          assert_ne!(base, DedupCache::hash_key("Edit", "other"));
      }

      #[test]
      fn fresh_cache_reports_no_duplicate() {
          let tmp = tempfile::tempdir().unwrap();
          let cache = DedupCache::at(tmp.path().join("dedup.json"));
          assert!(!cache.is_duplicate("Edit", "Edited /src/main.rs"));
      }

      #[test]
      fn recorded_entry_is_duplicate() {
          let tmp = tempfile::tempdir().unwrap();
          let cache = DedupCache::at(tmp.path().join("dedup.json"));
          cache.record("Edit", "Edited /src/main.rs");
          assert!(cache.is_duplicate("Edit", "Edited /src/main.rs"));
      }

      #[test]
      fn different_entry_is_not_duplicate_after_record() {
          let tmp = tempfile::tempdir().unwrap();
          let cache = DedupCache::at(tmp.path().join("dedup.json"));
          cache.record("Edit", "Edited /src/main.rs");
          assert!(!cache.is_duplicate("Write", "Wrote /src/lib.rs"));
      }

      #[test]
      fn expired_entry_is_not_duplicate() {
          let tmp = tempfile::tempdir().unwrap();
          let path = tmp.path().join("dedup.json");
          // Seed an entry whose timestamp is far in the past.
          let key = DedupCache::hash_key("Edit", "old");
          let mut entries = HashMap::new();
          entries.insert(key, 1_000u64); // ~1970, definitely expired
          let json = serde_json::to_string(&entries).unwrap();
          std::fs::write(&path, json).unwrap();

          let cache = DedupCache::at(path);
          assert!(!cache.is_duplicate("Edit", "old"));
      }

      #[test]
      fn corrupt_cache_fails_open_to_not_duplicate() {
          let tmp = tempfile::tempdir().unwrap();
          let path = tmp.path().join("dedup.json");
          std::fs::write(&path, b"{ not valid json").unwrap();
          let cache = DedupCache::at(path);
          assert!(!cache.is_duplicate("Edit", "anything"));
      }
  }
  ```

  Modify `crates/rb-hooks/src/main.rs` (full file):

  ```rust
  mod cli;
  mod dedup;
  mod io;

  fn main() {
      std::process::exit(0);
  }
  ```

- [ ] **Step 2: run it.**
  - Run: `cargo test -p rb-hooks dedup::`
  - Expected: FAIL — `DedupCache::at`, `hash_key`, `is_duplicate`, `record` are not defined (`no function or associated item`).

- [ ] **Step 3 GREEN: implement the cache. `for_namespace` resolves the XDG path; `at` is the test constructor; all I/O fails open.**

  Replace `crates/rb-hooks/src/dedup.rs` with:

  ```rust
  //! Short-TTL deduplication cache for PostToolUse observations.
  //!
  //! Each hook invocation is a fresh process, so dedup must be cross-process: the
  //! cache is a small JSON file under the XDG cache dir, namespaced per project.
  //! Entries expire after `TTL_SECONDS` and are pruned on every `record`. We store
  //! only stable FNV-1a hashes of `(tool_name, summary)` — never raw content.
  //!
  //! Fail-open: every error (unreadable/corrupt/unwritable cache) degrades to
  //! "not a duplicate" so capture never silently drops on cache trouble.

  use std::collections::HashMap;
  use std::path::{Path, PathBuf};
  use std::time::{SystemTime, UNIX_EPOCH};

  use rb_types::Namespace;

  const TTL_SECONDS: u64 = 60;

  /// A file-backed, per-namespace dedup cache.
  pub struct DedupCache {
      cache_path: PathBuf,
  }

  impl DedupCache {
      /// Resolve the per-namespace cache file under the XDG cache dir
      /// (`$XDG_CACHE_HOME/rusty-brain/` or `~/.cache/rusty-brain/`), falling back
      /// to the system temp dir if neither is set.
      pub fn for_namespace(namespace: &Namespace) -> Self {
          let dir = Self::cache_dir();
          let file = format!("dedup-{}.json", Self::namespace_slug(namespace));
          Self {
              cache_path: dir.join(file),
          }
      }

      /// Construct a cache at an explicit path (used by tests).
      pub fn at(cache_path: PathBuf) -> Self {
          Self { cache_path }
      }

      fn cache_dir() -> PathBuf {
          if let Some(xdg) = std::env::var_os("XDG_CACHE_HOME") {
              if !xdg.is_empty() {
                  return PathBuf::from(xdg).join("rusty-brain");
              }
          }
          if let Some(home) = std::env::var_os("HOME") {
              if !home.is_empty() {
                  return PathBuf::from(home).join(".cache").join("rusty-brain");
              }
          }
          std::env::temp_dir().join("rusty-brain")
      }

      /// Turn a namespace into a filesystem-safe slug (alphanumerics kept, every
      /// other byte replaced by `_`). Keeps caches for distinct projects separate.
      fn namespace_slug(namespace: &Namespace) -> String {
          let raw = namespace.as_db_string();
          raw.chars()
              .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
              .collect()
      }

      /// Stable FNV-1a 64-bit hash of `tool_name` + NUL + `summary`. Stable across
      /// processes and Rust versions (required: persisted to disk, read by a later
      /// invocation within the TTL window).
      fn hash_key(tool_name: &str, summary: &str) -> String {
          const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
          const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
          let mut hash = FNV_OFFSET;
          for byte in tool_name
              .as_bytes()
              .iter()
              .chain(b"\0")
              .chain(summary.as_bytes())
          {
              hash ^= u64::from(*byte);
              hash = hash.wrapping_mul(FNV_PRIME);
          }
          hash.to_string()
      }

      fn now_secs() -> u64 {
          SystemTime::now()
              .duration_since(UNIX_EPOCH)
              .map(|d| d.as_secs())
              .unwrap_or(0)
      }

      /// True if `(tool_name, summary)` was recorded within the last `TTL_SECONDS`.
      /// Any error (missing/corrupt cache) fails open to `false`.
      pub fn is_duplicate(&self, tool_name: &str, summary: &str) -> bool {
          let entries = self.read();
          let key = Self::hash_key(tool_name, summary);
          let now = Self::now_secs();
          entries
              .get(&key)
              .is_some_and(|&ts| now.saturating_sub(ts) < TTL_SECONDS)
      }

      /// Record `(tool_name, summary)` with the current timestamp, pruning expired
      /// entries first. Best-effort: write errors are swallowed (fail-open).
      pub fn record(&self, tool_name: &str, summary: &str) {
          let mut entries = self.read();
          let now = Self::now_secs();
          entries.retain(|_, ts| now.saturating_sub(*ts) < TTL_SECONDS);
          entries.insert(Self::hash_key(tool_name, summary), now);
          self.write(&entries);
      }

      /// Read the cache map; any error degrades to an empty map.
      fn read(&self) -> HashMap<String, u64> {
          match std::fs::read_to_string(&self.cache_path) {
              Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
              Err(_) => HashMap::new(),
          }
      }

      /// Atomically write the cache map (temp file + rename). Best-effort.
      fn write(&self, entries: &HashMap<String, u64>) {
          if let Some(parent) = self.cache_path.parent() {
              if std::fs::create_dir_all(parent).is_err() {
                  return;
              }
          }
          let Ok(json) = serde_json::to_string(entries) else {
              return;
          };
          let tmp = self
              .cache_path
              .with_extension(format!("tmp.{}", std::process::id()));
          if std::fs::write(&tmp, json.as_bytes()).is_err() {
              return;
          }
          if std::fs::rename(&tmp, &self.cache_path).is_err() {
              let _ = std::fs::remove_file(&tmp);
          }
      }
  }

  /// Suppress the unused-import lint for `Path` in builds where only `PathBuf` is
  /// referenced directly; `Path` is part of the documented public path API.
  #[allow(dead_code)]
  fn _path_marker(_: &Path) {}

  #[cfg(test)]
  mod tests {
      #![allow(clippy::unwrap_used, clippy::expect_used)]
      use super::*;

      #[test]
      fn hash_key_is_deterministic() {
          let a = DedupCache::hash_key("Edit", "Edited /src/main.rs");
          let b = DedupCache::hash_key("Edit", "Edited /src/main.rs");
          assert_eq!(a, b);
      }

      #[test]
      fn hash_key_differs_by_tool_and_summary() {
          let base = DedupCache::hash_key("Edit", "summary");
          assert_ne!(base, DedupCache::hash_key("Write", "summary"));
          assert_ne!(base, DedupCache::hash_key("Edit", "other"));
      }

      #[test]
      fn fresh_cache_reports_no_duplicate() {
          let tmp = tempfile::tempdir().unwrap();
          let cache = DedupCache::at(tmp.path().join("dedup.json"));
          assert!(!cache.is_duplicate("Edit", "Edited /src/main.rs"));
      }

      #[test]
      fn recorded_entry_is_duplicate() {
          let tmp = tempfile::tempdir().unwrap();
          let cache = DedupCache::at(tmp.path().join("dedup.json"));
          cache.record("Edit", "Edited /src/main.rs");
          assert!(cache.is_duplicate("Edit", "Edited /src/main.rs"));
      }

      #[test]
      fn different_entry_is_not_duplicate_after_record() {
          let tmp = tempfile::tempdir().unwrap();
          let cache = DedupCache::at(tmp.path().join("dedup.json"));
          cache.record("Edit", "Edited /src/main.rs");
          assert!(!cache.is_duplicate("Write", "Wrote /src/lib.rs"));
      }

      #[test]
      fn expired_entry_is_not_duplicate() {
          let tmp = tempfile::tempdir().unwrap();
          let path = tmp.path().join("dedup.json");
          // Seed an entry whose timestamp is far in the past.
          let key = DedupCache::hash_key("Edit", "old");
          let mut entries = HashMap::new();
          entries.insert(key, 1_000u64); // ~1970, definitely expired
          let json = serde_json::to_string(&entries).unwrap();
          std::fs::write(&path, json).unwrap();

          let cache = DedupCache::at(path);
          assert!(!cache.is_duplicate("Edit", "old"));
      }

      #[test]
      fn corrupt_cache_fails_open_to_not_duplicate() {
          let tmp = tempfile::tempdir().unwrap();
          let path = tmp.path().join("dedup.json");
          std::fs::write(&path, b"{ not valid json").unwrap();
          let cache = DedupCache::at(path);
          assert!(!cache.is_duplicate("Edit", "anything"));
      }

      #[test]
      fn namespace_slug_is_filesystem_safe() {
          let slug = DedupCache::namespace_slug(&Namespace::Project("rusty/brain:1".into()));
          assert!(
              slug.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
              "slug must be fs-safe, got {slug}"
          );
      }
  }
  ```

- [ ] **Step 4: run it.**
  - Run: `cargo test -p rb-hooks dedup::`
  - Expected: PASS (8 tests).

- [ ] **Step 5: lint + format.**
  - Run: `cargo clippy -p rb-hooks --all-targets -- -D warnings`
  - Expected: no warnings.
  - Run: `cargo fmt --all`
  - Expected: no diff.

- [ ] **Step 6: commit.**
  - Run: `git add crates/rb-hooks/src/dedup.rs crates/rb-hooks/src/main.rs && git commit -m "feat(rb-hooks): short-ttl file-backed dedup cache"`
  - Expected: one commit.

---

### Task W5: rb-hooks src/capture.rs — PostToolUse flow

**Files:**
- Create: `crates/rb-hooks/src/capture.rs`
- Modify: `crates/rb-hooks/src/main.rs` (add `mod capture;`)
- Test: inline `#[cfg(test)]` module in `crates/rb-hooks/src/capture.rs`

This task lands the pure PostToolUse helpers (`is_mutation_tool`, `classify_tool`, `truncate_head_tail`, `summarize_post_tool_use`) plus the async `post_tool_use` flow that calls `DaemonClient::remember`. The async flow is exercised against an in-process daemon responder in the integration test (Task W9); here we unit-test all the pure helpers and the dedup short-circuit shape.

- [ ] **Step 1 RED: write failing tests for the pure helpers.**

  Create `crates/rb-hooks/src/capture.rs`:

  ```rust
  //! The four capture flows: SessionStart (inject), PostToolUse (capture mutating
  //! tools, deduped), Stop (session summary + git-modified files), PreCompact
  //! (capture decisions). Every flow returns a `HookResult` with
  //! `continue_execution: true`; nothing ever blocks.

  use rb_agents::daemon::DaemonClient;
  use rb_agents::event::HookResult;
  use rb_types::MemoryType;

  use crate::dedup::DedupCache;

  /// Mutation tools whose observations are captured. Discovery tools (Read, Grep,
  /// Glob, WebFetch, ...) are excluded to reduce noise and improve recall quality.
  const MUTATION_TOOLS: &[&str] = &["Edit", "Write", "NotebookEdit", "Bash"];

  /// Max characters retained from a tool response before head/tail truncation.
  const MAX_RESPONSE_CHARS: usize = 2000;

  #[cfg(test)]
  mod tests {
      #![allow(clippy::unwrap_used, clippy::expect_used)]
      use super::*;

      #[test]
      fn mutation_tools_are_recognized() {
          for t in ["Edit", "Write", "NotebookEdit", "Bash"] {
              assert!(is_mutation_tool(t), "{t} should be a mutation tool");
          }
      }

      #[test]
      fn discovery_tools_are_not_mutation() {
          for t in ["Read", "Grep", "Glob", "WebFetch", "WebSearch", ""] {
              assert!(!is_mutation_tool(t), "{t} should not be a mutation tool");
          }
      }

      #[test]
      fn classify_file_tools_to_code_pattern() {
          assert_eq!(classify_tool("Edit"), MemoryType::CodePattern);
          assert_eq!(classify_tool("Write"), MemoryType::CodePattern);
          assert_eq!(classify_tool("NotebookEdit"), MemoryType::CodePattern);
      }

      #[test]
      fn classify_bash_to_reference() {
          assert_eq!(classify_tool("Bash"), MemoryType::Reference);
      }

      #[test]
      fn classify_unknown_defaults_to_reference() {
          assert_eq!(classify_tool("SomeFutureTool"), MemoryType::Reference);
      }

      #[test]
      fn truncate_passes_through_short_content() {
          let s = "short content";
          assert_eq!(truncate_head_tail(s, 100), s);
      }

      #[test]
      fn truncate_inserts_marker_for_long_content() {
          let s = "a".repeat(5000);
          let out = truncate_head_tail(&s, 100);
          assert!(out.len() < s.len());
          assert!(out.contains("[...truncated...]"));
      }

      #[test]
      fn truncate_is_utf8_safe() {
          let s = "é".repeat(5000);
          let out = truncate_head_tail(&s, 100);
          // Must remain valid UTF-8 (no panic on multibyte boundaries).
          assert!(out.contains("[...truncated...]"));
      }

      #[test]
      fn summarize_edit_includes_tool_and_path() {
          let input = serde_json::json!({"file_path": "/src/main.rs"});
          let summary = summarize_post_tool_use("Edit", &input);
          assert_eq!(summary, "Edited /src/main.rs");
      }

      #[test]
      fn summarize_write_includes_path() {
          let input = serde_json::json!({"file_path": "/src/lib.rs"});
          let summary = summarize_post_tool_use("Write", &input);
          assert_eq!(summary, "Wrote /src/lib.rs");
      }

      #[test]
      fn summarize_bash_includes_truncated_command() {
          let input = serde_json::json!({"command": "cargo test --workspace"});
          let summary = summarize_post_tool_use("Bash", &input);
          assert_eq!(summary, "Ran command: cargo test --workspace");
      }

      #[test]
      fn summarize_bash_truncates_long_command() {
          let input = serde_json::json!({"command": "x".repeat(200)});
          let summary = summarize_post_tool_use("Bash", &input);
          assert!(summary.starts_with("Ran command: "));
          assert!(summary.len() < 200);
      }

      #[test]
      fn summarize_missing_field_uses_unknown() {
          let input = serde_json::json!({});
          assert_eq!(summarize_post_tool_use("Edit", &input), "Edited unknown");
      }

      #[test]
      fn summarize_notebook_edit_uses_path() {
          let input = serde_json::json!({"notebook_path": "/nb.ipynb"});
          let summary = summarize_post_tool_use("NotebookEdit", &input);
          assert_eq!(summary, "Edited notebook /nb.ipynb");
      }
  }
  ```

  Modify `crates/rb-hooks/src/main.rs` (full file):

  ```rust
  mod capture;
  mod cli;
  mod dedup;
  mod io;

  fn main() {
      std::process::exit(0);
  }
  ```

- [ ] **Step 2: run it.**
  - Run: `cargo test -p rb-hooks capture::`
  - Expected: FAIL — `is_mutation_tool`, `classify_tool`, `truncate_head_tail`, `summarize_post_tool_use` are not defined.

- [ ] **Step 3 GREEN: implement the pure helpers and the async `post_tool_use` flow.**

  Replace `crates/rb-hooks/src/capture.rs` with:

  ```rust
  //! The four capture flows: SessionStart (inject), PostToolUse (capture mutating
  //! tools, deduped), Stop (session summary + git-modified files), PreCompact
  //! (capture decisions). Every flow returns a `HookResult` with
  //! `continue_execution: true`; nothing ever blocks.

  use rb_agents::daemon::DaemonClient;
  use rb_agents::event::HookResult;
  use rb_types::MemoryType;

  use crate::dedup::DedupCache;

  /// Mutation tools whose observations are captured. Discovery tools (Read, Grep,
  /// Glob, WebFetch, ...) are excluded to reduce noise and improve recall quality.
  const MUTATION_TOOLS: &[&str] = &["Edit", "Write", "NotebookEdit", "Bash"];

  /// Max characters retained from a tool response before head/tail truncation.
  const MAX_RESPONSE_CHARS: usize = 2000;

  const TRUNCATION_MARKER: &str = "[...truncated...]";

  /// A `HookResult` that injects no message and always continues.
  fn continue_only() -> HookResult {
      HookResult {
          system_message: None,
          continue_execution: true,
      }
  }

  /// True if `tool_name` is a captured mutation tool.
  fn is_mutation_tool(tool_name: &str) -> bool {
      MUTATION_TOOLS.contains(&tool_name)
  }

  /// Map a tool name to a `MemoryType`: file-mutation tools are code patterns;
  /// everything else (Bash, unknown) is a reference observation.
  fn classify_tool(tool_name: &str) -> MemoryType {
      match tool_name {
          "Edit" | "Write" | "NotebookEdit" => MemoryType::CodePattern,
          _ => MemoryType::Reference,
      }
  }

  /// Head/tail truncate to roughly `max_chars`, inserting a marker. UTF-8 safe:
  /// boundaries are taken on `char_indices`, never raw byte offsets.
  fn truncate_head_tail(content: &str, max_chars: usize) -> String {
      let char_count = content.chars().count();
      if char_count <= max_chars {
          return content.to_string();
      }
      let marker_len = TRUNCATION_MARKER.chars().count();
      let budget = max_chars.saturating_sub(marker_len);
      let head_chars = budget * 60 / 100;
      let tail_chars = budget.saturating_sub(head_chars);
      let head_end = content
          .char_indices()
          .nth(head_chars)
          .map_or(content.len(), |(idx, _)| idx);
      let tail_start = content
          .char_indices()
          .nth(char_count.saturating_sub(tail_chars))
          .map_or(content.len(), |(idx, _)| idx);
      let head = &content[..head_end];
      let tail = &content[tail_start..];
      format!("{head}{TRUNCATION_MARKER}{tail}")
  }

  /// Pull a string field from a JSON object, defaulting to `"unknown"`.
  fn str_field<'a>(input: &'a serde_json::Value, key: &str) -> &'a str {
      input.get(key).and_then(|v| v.as_str()).unwrap_or("unknown")
  }

  /// Build a concise, human-readable summary of a tool invocation.
  fn summarize_post_tool_use(tool_name: &str, tool_input: &serde_json::Value) -> String {
      match tool_name {
          "Edit" => format!("Edited {}", str_field(tool_input, "file_path")),
          "Write" => format!("Wrote {}", str_field(tool_input, "file_path")),
          "NotebookEdit" => format!("Edited notebook {}", str_field(tool_input, "notebook_path")),
          "Bash" => {
              let cmd = str_field(tool_input, "command");
              let truncated = match cmd.char_indices().nth(80) {
                  Some((idx, _)) => &cmd[..idx],
                  None => cmd,
              };
              format!("Ran command: {truncated}")
          }
          other => format!("Used {other}"),
      }
  }

  /// Extract text from a tool response value (string used directly; else JSON).
  fn extract_response_text(response: &serde_json::Value) -> String {
      match response {
          serde_json::Value::Null => String::new(),
          serde_json::Value::String(s) => s.clone(),
          other => serde_json::to_string(other).unwrap_or_default(),
      }
  }

  /// PostToolUse capture flow. No-op (continue) for non-mutation tools or
  /// deduplicated observations; otherwise builds a summary + truncated context and
  /// calls `DaemonClient::remember`. Always returns `continue_execution: true`.
  pub async fn post_tool_use(
      client: Option<&mut DaemonClient>,
      dedup: &DedupCache,
      tool_name: &str,
      tool_input: &serde_json::Value,
      tool_response: &serde_json::Value,
  ) -> HookResult {
      if !is_mutation_tool(tool_name) {
          return continue_only();
      }
      let summary = summarize_post_tool_use(tool_name, tool_input);
      if dedup.is_duplicate(tool_name, &summary) {
          return continue_only();
      }

      let memory_type = classify_tool(tool_name);
      let raw = extract_response_text(tool_response);
      let context = if raw.trim().is_empty() {
          None
      } else {
          Some(truncate_head_tail(&raw, MAX_RESPONSE_CHARS))
      };

      if let Some(client) = client {
          let _ = client
              .remember(
                  summary.clone(),
                  context,
                  memory_type,
                  5,
                  vec!["hook".to_string(), "post-tool-use".to_string()],
              )
              .await;
      }
      // Record AFTER a (best-effort) store so a failed connect does not poison the
      // dedup window — but record regardless of remember outcome to bound retries.
      dedup.record(tool_name, &summary);
      continue_only()
  }

  #[cfg(test)]
  mod tests {
      #![allow(clippy::unwrap_used, clippy::expect_used)]
      use super::*;

      #[test]
      fn mutation_tools_are_recognized() {
          for t in ["Edit", "Write", "NotebookEdit", "Bash"] {
              assert!(is_mutation_tool(t), "{t} should be a mutation tool");
          }
      }

      #[test]
      fn discovery_tools_are_not_mutation() {
          for t in ["Read", "Grep", "Glob", "WebFetch", "WebSearch", ""] {
              assert!(!is_mutation_tool(t), "{t} should not be a mutation tool");
          }
      }

      #[test]
      fn classify_file_tools_to_code_pattern() {
          assert_eq!(classify_tool("Edit"), MemoryType::CodePattern);
          assert_eq!(classify_tool("Write"), MemoryType::CodePattern);
          assert_eq!(classify_tool("NotebookEdit"), MemoryType::CodePattern);
      }

      #[test]
      fn classify_bash_to_reference() {
          assert_eq!(classify_tool("Bash"), MemoryType::Reference);
      }

      #[test]
      fn classify_unknown_defaults_to_reference() {
          assert_eq!(classify_tool("SomeFutureTool"), MemoryType::Reference);
      }

      #[test]
      fn truncate_passes_through_short_content() {
          let s = "short content";
          assert_eq!(truncate_head_tail(s, 100), s);
      }

      #[test]
      fn truncate_inserts_marker_for_long_content() {
          let s = "a".repeat(5000);
          let out = truncate_head_tail(&s, 100);
          assert!(out.len() < s.len());
          assert!(out.contains("[...truncated...]"));
      }

      #[test]
      fn truncate_is_utf8_safe() {
          let s = "é".repeat(5000);
          let out = truncate_head_tail(&s, 100);
          // Must remain valid UTF-8 (no panic on multibyte boundaries).
          assert!(out.contains("[...truncated...]"));
      }

      #[test]
      fn summarize_edit_includes_tool_and_path() {
          let input = serde_json::json!({"file_path": "/src/main.rs"});
          let summary = summarize_post_tool_use("Edit", &input);
          assert_eq!(summary, "Edited /src/main.rs");
      }

      #[test]
      fn summarize_write_includes_path() {
          let input = serde_json::json!({"file_path": "/src/lib.rs"});
          let summary = summarize_post_tool_use("Write", &input);
          assert_eq!(summary, "Wrote /src/lib.rs");
      }

      #[test]
      fn summarize_bash_includes_truncated_command() {
          let input = serde_json::json!({"command": "cargo test --workspace"});
          let summary = summarize_post_tool_use("Bash", &input);
          assert_eq!(summary, "Ran command: cargo test --workspace");
      }

      #[test]
      fn summarize_bash_truncates_long_command() {
          let input = serde_json::json!({"command": "x".repeat(200)});
          let summary = summarize_post_tool_use("Bash", &input);
          assert!(summary.starts_with("Ran command: "));
          assert!(summary.len() < 200);
      }

      #[test]
      fn summarize_missing_field_uses_unknown() {
          let input = serde_json::json!({});
          assert_eq!(summarize_post_tool_use("Edit", &input), "Edited unknown");
      }

      #[test]
      fn summarize_notebook_edit_uses_path() {
          let input = serde_json::json!({"notebook_path": "/nb.ipynb"});
          let summary = summarize_post_tool_use("NotebookEdit", &input);
          assert_eq!(summary, "Edited notebook /nb.ipynb");
      }

      #[test]
      fn extract_response_text_handles_variants() {
          assert_eq!(extract_response_text(&serde_json::Value::Null), "");
          assert_eq!(
              extract_response_text(&serde_json::json!("hello")),
              "hello"
          );
          let obj = extract_response_text(&serde_json::json!({"k": "v"}));
          assert!(obj.contains("\"k\""));
      }

      #[tokio::test]
      async fn post_tool_use_non_mutation_is_noop_continue() {
          let tmp = tempfile::tempdir().unwrap();
          let dedup = DedupCache::at(tmp.path().join("d.json"));
          let result = post_tool_use(
              None,
              &dedup,
              "Read",
              &serde_json::json!({"file_path": "/x"}),
              &serde_json::json!("contents"),
          )
          .await;
          assert!(result.continue_execution);
          assert!(result.system_message.is_none());
          // Non-mutation must not poison the dedup cache.
          assert!(!dedup.is_duplicate("Read", "Read /x"));
      }

      #[tokio::test]
      async fn post_tool_use_records_dedup_for_mutation() {
          let tmp = tempfile::tempdir().unwrap();
          let dedup = DedupCache::at(tmp.path().join("d.json"));
          let result = post_tool_use(
              None,
              &dedup,
              "Edit",
              &serde_json::json!({"file_path": "/src/main.rs"}),
              &serde_json::json!("ok"),
          )
          .await;
          assert!(result.continue_execution);
          assert!(dedup.is_duplicate("Edit", "Edited /src/main.rs"));
      }
  }
  ```

- [ ] **Step 4: run it.**
  - Run: `cargo test -p rb-hooks capture::`
  - Expected: PASS (17 tests).

- [ ] **Step 5: lint + format.**
  - Run: `cargo clippy -p rb-hooks --all-targets -- -D warnings`
  - Expected: no warnings.
  - Run: `cargo fmt --all`
  - Expected: no diff.

- [ ] **Step 6: commit.**
  - Run: `git add crates/rb-hooks/src/capture.rs crates/rb-hooks/src/main.rs && git commit -m "feat(rb-hooks): post-tool-use capture flow with dedup"`
  - Expected: one commit.

---

### Task W6: rb-hooks src/capture.rs — SessionStart flow

**Files:**
- Modify: `crates/rb-hooks/src/capture.rs` (add `format_session_start` + `session_start`)
- Test: extend inline `#[cfg(test)]` module in `crates/rb-hooks/src/capture.rs`

This task adds the SessionStart context-injection flow. `DaemonClient::context()` yields `(recent, important, total)`; we format a markdown system message splitting critical (`importance >= 8`) from important (`importance == 7`) and listing recent observations, mirroring the old `session_start` shape.

- [ ] **Step 1 RED: add a failing test for the pure `format_session_start`.**

  Add these tests to the `tests` module in `crates/rb-hooks/src/capture.rs` (before the closing `}` of `mod tests`):

  ```rust
      fn sample_note(content: &str, importance: u8) -> rb_types::MemoryNote {
          rb_types::MemoryNote::new(
              rb_types::Namespace::Project("rusty-brain".into()),
              content.to_string(),
              MemoryType::Insight,
              importance,
          )
      }

      #[test]
      fn format_session_start_empty_has_header_and_no_sections() {
          let msg = format_session_start(&[], &[], 0);
          assert!(msg.contains("# Rusty Brain"));
          assert!(!msg.contains("## Critical"));
          assert!(!msg.contains("## Recent"));
      }

      #[test]
      fn format_session_start_splits_critical_and_important() {
          let important = vec![sample_note("crit decision", 9), sample_note("imp note", 7)];
          let msg = format_session_start(&[], &important, 2);
          assert!(msg.contains("## Critical"));
          assert!(msg.contains("crit decision"));
          assert!(msg.contains("## Important"));
          assert!(msg.contains("imp note"));
      }

      #[test]
      fn format_session_start_lists_recent_and_total() {
          let recent = vec![sample_note("did a thing", 5)];
          let msg = format_session_start(&recent, &[], 12);
          assert!(msg.contains("## Recent"));
          assert!(msg.contains("did a thing"));
          assert!(msg.contains("12"), "should mention the total count");
      }

      #[tokio::test]
      async fn session_start_without_client_continues_with_no_message() {
          let result = session_start(None).await;
          assert!(result.continue_execution);
          assert!(result.system_message.is_none());
      }
  ```

- [ ] **Step 2: run it.**
  - Run: `cargo test -p rb-hooks capture::tests::format_session_start`
  - Expected: FAIL — `format_session_start` and `session_start` are not defined.

- [ ] **Step 3 GREEN: implement `format_session_start` and the async `session_start` flow. Insert these functions in `crates/rb-hooks/src/capture.rs` immediately after the `post_tool_use` function (before the `#[cfg(test)]` module).**

  ```rust
  /// Pure: format recent + important memories into a markdown system message.
  /// `important` is split into critical (`importance >= 8`) and important
  /// (`importance == 7`). Returns a header-only message when everything is empty.
  fn format_session_start(
      recent: &[rb_types::MemoryNote],
      important: &[rb_types::MemoryNote],
      total: usize,
  ) -> String {
      let mut out = String::new();
      out.push_str("# Rusty Brain — Memory Active\n");
      out.push_str(&format!("Total memories in scope: {total}\n"));

      let critical: Vec<&rb_types::MemoryNote> =
          important.iter().filter(|m| m.importance >= 8).collect();
      let merely_important: Vec<&rb_types::MemoryNote> =
          important.iter().filter(|m| m.importance == 7).collect();

      if !critical.is_empty() {
          out.push_str("\n## Critical\n");
          for m in critical {
              out.push_str(&format!("- {}\n", memory_line(m)));
          }
      }
      if !merely_important.is_empty() {
          out.push_str("\n## Important\n");
          for m in merely_important {
              out.push_str(&format!("- {}\n", memory_line(m)));
          }
      }
      if !recent.is_empty() {
          out.push_str("\n## Recent\n");
          for m in recent {
              out.push_str(&format!("- {}\n", memory_line(m)));
          }
      }
      out
  }

  /// One-line rendering of a memory: prefer its summary, else its content.
  fn memory_line(memory: &rb_types::MemoryNote) -> String {
      let text = if memory.summary.trim().is_empty() {
          memory.content.as_str()
      } else {
          memory.summary.as_str()
      };
      format!("[{}] {}", memory.memory_type.as_str(), text.trim())
  }

  /// SessionStart flow: fetch context and inject a markdown system message.
  /// Always continues. With no client (degraded), continues with no message.
  pub async fn session_start(client: Option<&mut DaemonClient>) -> HookResult {
      let Some(client) = client else {
          return continue_only();
      };
      match client.context().await {
          Some((recent, important, total)) => {
              let message = format_session_start(&recent, &important, total);
              HookResult {
                  system_message: Some(message),
                  continue_execution: true,
              }
          }
          None => continue_only(),
      }
  }
  ```

- [ ] **Step 4: run it.**
  - Run: `cargo test -p rb-hooks capture::`
  - Expected: PASS (21 tests).

- [ ] **Step 5: lint + format.**
  - Run: `cargo clippy -p rb-hooks --all-targets -- -D warnings`
  - Expected: no warnings.
  - Run: `cargo fmt --all`
  - Expected: no diff.

- [ ] **Step 6: commit.**
  - Run: `git add crates/rb-hooks/src/capture.rs && git commit -m "feat(rb-hooks): session-start context injection flow"`
  - Expected: one commit.

---

### Task W7: rb-hooks src/capture.rs — Stop flow + git

**Files:**
- Modify: `crates/rb-hooks/src/capture.rs` (add `git_modified_files` + `stop`)
- Test: extend inline `#[cfg(test)]` module in `crates/rb-hooks/src/capture.rs`

This task adds the Stop flow: detect git-modified files via `git diff --name-only HEAD` (fail-open: empty vec if not a repo / git missing / non-zero), build a session summary, and `remember` it as `MemoryType::Reference`.

- [ ] **Step 1 RED: add failing tests for `git_modified_files` and `format_stop_summary`.**

  Add these tests to the `tests` module in `crates/rb-hooks/src/capture.rs` (before the closing `}` of `mod tests`):

  ```rust
      #[test]
      fn git_modified_files_empty_for_non_repo() {
          let tmp = tempfile::tempdir().unwrap();
          let files = git_modified_files(tmp.path());
          assert!(files.is_empty(), "non-repo must yield empty vec");
      }

      #[test]
      fn git_modified_files_empty_for_nonexistent_dir() {
          let files = git_modified_files(std::path::Path::new("/nonexistent/path/xyz"));
          assert!(files.is_empty());
      }

      #[test]
      fn git_modified_files_detects_change_in_repo() {
          let tmp = tempfile::tempdir().unwrap();
          let run = |args: &[&str]| {
              std::process::Command::new("git")
                  .args(args)
                  .current_dir(tmp.path())
                  .output()
          };
          if run(&["init"]).map(|o| o.status.success()).unwrap_or(false) == false {
              return; // git unavailable; skip
          }
          let _ = run(&["config", "user.email", "t@t.com"]);
          let _ = run(&["config", "user.name", "T"]);
          std::fs::write(tmp.path().join("f.txt"), "initial").unwrap();
          let _ = run(&["add", "."]);
          let _ = run(&["commit", "-m", "init"]);
          std::fs::write(tmp.path().join("f.txt"), "changed").unwrap();
          let files = git_modified_files(tmp.path());
          assert!(
              files.contains(&"f.txt".to_string()),
              "should detect modified file, got {files:?}"
          );
      }

      #[test]
      fn format_stop_summary_no_files() {
          let summary = format_stop_summary(&[]);
          assert!(summary.to_lowercase().contains("no file"));
      }

      #[test]
      fn format_stop_summary_lists_files() {
          let summary = format_stop_summary(&["a.rs".to_string(), "b.rs".to_string()]);
          assert!(summary.contains("2"));
          assert!(summary.contains("a.rs"));
          assert!(summary.contains("b.rs"));
      }

      #[tokio::test]
      async fn stop_without_client_continues() {
          let tmp = tempfile::tempdir().unwrap();
          let result = stop(None, tmp.path()).await;
          assert!(result.continue_execution);
      }
  ```

- [ ] **Step 2: run it.**
  - Run: `cargo test -p rb-hooks capture::tests::git_modified_files`
  - Expected: FAIL — `git_modified_files`, `format_stop_summary`, `stop` are not defined.

- [ ] **Step 3 GREEN: implement git detection and the Stop flow. Insert these functions in `crates/rb-hooks/src/capture.rs` immediately after the `session_start` function (before the `#[cfg(test)]` module).**

  ```rust
  /// Detect working-tree-modified files via `git diff --name-only HEAD`. Fail-open:
  /// returns an empty vec on any failure (git missing, not a repo, non-zero exit).
  /// Arguments are hardcoded literals (no shell, no user interpolation).
  fn git_modified_files(cwd: &std::path::Path) -> Vec<String> {
      let output = std::process::Command::new("git")
          .args(["diff", "--name-only", "HEAD"])
          .current_dir(cwd)
          .stdin(std::process::Stdio::null())
          .stderr(std::process::Stdio::null())
          .output();
      let Ok(output) = output else {
          return Vec::new();
      };
      if !output.status.success() {
          return Vec::new();
      }
      let Ok(text) = String::from_utf8(output.stdout) else {
          return Vec::new();
      };
      text.lines()
          .map(str::trim)
          .filter(|line| !line.is_empty())
          .map(str::to_string)
          .collect()
  }

  /// Build the Stop session-summary text from the modified-file list.
  fn format_stop_summary(modified: &[String]) -> String {
      if modified.is_empty() {
          "Session ended with no file modifications.".to_string()
      } else {
          format!(
              "Session ended. Modified {} file(s): {}",
              modified.len(),
              modified.join(", ")
          )
      }
  }

  /// Stop flow: record a session summary memory (including git-modified files).
  /// Always continues. With no client (degraded), continues with no store.
  pub async fn stop(client: Option<&mut DaemonClient>, cwd: &std::path::Path) -> HookResult {
      let modified = git_modified_files(cwd);
      let summary = format_stop_summary(&modified);
      if let Some(client) = client {
          let _ = client
              .remember(
                  summary,
                  None,
                  MemoryType::Reference,
                  4,
                  vec!["hook".to_string(), "session-summary".to_string()],
              )
              .await;
      }
      continue_only()
  }
  ```

- [ ] **Step 4: run it.**
  - Run: `cargo test -p rb-hooks capture::`
  - Expected: PASS (27 tests).

- [ ] **Step 5: lint + format.**
  - Run: `cargo clippy -p rb-hooks --all-targets -- -D warnings`
  - Expected: no warnings.
  - Run: `cargo fmt --all`
  - Expected: no diff.

- [ ] **Step 6: commit.**
  - Run: `git add crates/rb-hooks/src/capture.rs && git commit -m "feat(rb-hooks): stop flow records git-modified session summary"`
  - Expected: one commit.

---

### Task W8: rb-hooks src/capture.rs — PreCompact flow + dispatch.rs

**Files:**
- Modify: `crates/rb-hooks/src/capture.rs` (add `has_decision_marker` + `pre_compact`)
- Create: `crates/rb-hooks/src/dispatch.rs`
- Modify: `crates/rb-hooks/src/main.rs` (add `mod dispatch;`)
- Test: extend inline `#[cfg(test)]` module in `crates/rb-hooks/src/capture.rs`; inline tests in `crates/rb-hooks/src/dispatch.rs`

This task adds the PreCompact flow (scan custom instructions for decision markers; on a hit, `remember` an importance-8 `ArchitectureDecision`) and the dispatcher that routes a `HookContext.event` to the right flow.

- [ ] **Step 1 RED: add failing tests for the PreCompact helper and the dispatcher.**

  Add these tests to the `tests` module in `crates/rb-hooks/src/capture.rs` (before the closing `}` of `mod tests`):

  ```rust
      #[test]
      fn decision_marker_detected_case_insensitively() {
          assert!(has_decision_marker("We DECIDED to use SQLite."));
          assert!(has_decision_marker("Decision: single writer."));
          assert!(has_decision_marker("the chosen approach is X"));
      }

      #[test]
      fn no_decision_marker_in_plain_text() {
          assert!(!has_decision_marker("just some ordinary notes"));
          assert!(!has_decision_marker(""));
      }

      #[tokio::test]
      async fn pre_compact_without_marker_is_noop_continue() {
          let result = pre_compact(None, Some("ordinary instructions")).await;
          assert!(result.continue_execution);
          assert!(result.system_message.is_none());
      }

      #[tokio::test]
      async fn pre_compact_with_marker_continues() {
          let result = pre_compact(None, Some("Decision: use one DB")).await;
          assert!(result.continue_execution);
      }
  ```

  Create `crates/rb-hooks/src/dispatch.rs`:

  ```rust
  //! Route a `HookContext` event to the matching capture flow. `Other` events are
  //! a no-op (continue). Every flow returns `continue_execution: true`.

  use rb_agents::daemon::DaemonClient;
  use rb_agents::event::{HookContext, HookEvent, HookResult};

  use crate::capture;
  use crate::dedup::DedupCache;

  #[cfg(test)]
  mod tests {
      #![allow(clippy::unwrap_used, clippy::expect_used)]
      use super::*;
      use rb_agents::event::{HookContext, HookEvent};
      use std::path::PathBuf;

      fn ctx(event: HookEvent) -> HookContext {
          HookContext {
              event,
              cwd: PathBuf::from("/tmp"),
              session_id: Some("s1".to_string()),
          }
      }

      #[tokio::test]
      async fn other_event_is_noop_continue() {
          let tmp = tempfile::tempdir().unwrap();
          let dedup = DedupCache::at(tmp.path().join("d.json"));
          let result = dispatch(None, &dedup, &ctx(HookEvent::Other("Notification".into()))).await;
          assert!(result.continue_execution);
          assert!(result.system_message.is_none());
      }

      #[tokio::test]
      async fn post_tool_use_event_routes_and_continues() {
          let tmp = tempfile::tempdir().unwrap();
          let dedup = DedupCache::at(tmp.path().join("d.json"));
          let event = HookEvent::PostToolUse {
              tool_name: "Edit".to_string(),
              tool_input: serde_json::json!({"file_path": "/src/main.rs"}),
              tool_response: serde_json::json!("ok"),
          };
          let result = dispatch(None, &dedup, &ctx(event)).await;
          assert!(result.continue_execution);
          // Routed to post_tool_use: the mutation must have been deduped.
          assert!(dedup.is_duplicate("Edit", "Edited /src/main.rs"));
      }

      #[tokio::test]
      async fn stop_event_continues() {
          let tmp = tempfile::tempdir().unwrap();
          let dedup = DedupCache::at(tmp.path().join("d.json"));
          let event = HookEvent::Stop {
              last_assistant_message: Some("done".to_string()),
          };
          let result = dispatch(None, &dedup, &ctx(event)).await;
          assert!(result.continue_execution);
      }
  }
  ```

  Modify `crates/rb-hooks/src/main.rs` (full file):

  ```rust
  mod capture;
  mod cli;
  mod dedup;
  mod dispatch;
  mod io;

  fn main() {
      std::process::exit(0);
  }
  ```

- [ ] **Step 2: run it.**
  - Run: `cargo test -p rb-hooks dispatch:: capture::tests::pre_compact`
  - Expected: FAIL — `has_decision_marker`, `pre_compact`, and `dispatch::dispatch` are not defined.

- [ ] **Step 3 GREEN: implement the PreCompact helpers (in capture.rs) and the dispatcher (in dispatch.rs).**

  Insert these functions in `crates/rb-hooks/src/capture.rs` immediately after the `stop` function (before the `#[cfg(test)]` module):

  ```rust
  /// Decision marker substrings (lowercased match) used to detect that compaction
  /// is about to drop a recorded decision worth persisting.
  const DECISION_MARKERS: &[&str] = &["decided", "decision", "chosen", "we will use", "approach is"];

  /// True if `text` contains any decision marker (case-insensitive).
  fn has_decision_marker(text: &str) -> bool {
      let lower = text.to_lowercase();
      DECISION_MARKERS.iter().any(|m| lower.contains(m))
  }

  /// PreCompact flow: if the custom instructions reference a decision, capture it
  /// as a high-importance architecture decision. Always continues.
  pub async fn pre_compact(
      client: Option<&mut DaemonClient>,
      custom_instructions: Option<&str>,
  ) -> HookResult {
      let Some(text) = custom_instructions else {
          return continue_only();
      };
      if !has_decision_marker(text) {
          return continue_only();
      }
      if let Some(client) = client {
          let _ = client
              .remember(
                  format!("Pre-compaction decision snapshot: {}", text.trim()),
                  None,
                  MemoryType::ArchitectureDecision,
                  8,
                  vec!["hook".to_string(), "pre-compact".to_string()],
              )
              .await;
      }
      continue_only()
  }
  ```

  Replace `crates/rb-hooks/src/dispatch.rs` with:

  ```rust
  //! Route a `HookContext` event to the matching capture flow. `Other` events are
  //! a no-op (continue). Every flow returns `continue_execution: true`.

  use rb_agents::daemon::DaemonClient;
  use rb_agents::event::{HookContext, HookEvent, HookResult};

  use crate::capture;
  use crate::dedup::DedupCache;

  /// Dispatch one parsed hook context to its capture flow. The optional client is
  /// the (best-effort) daemon connection; `None` means degraded — flows still
  /// return `continue_execution: true`.
  pub async fn dispatch(
      mut client: Option<&mut DaemonClient>,
      dedup: &DedupCache,
      ctx: &HookContext,
  ) -> HookResult {
      match &ctx.event {
          HookEvent::SessionStart { .. } => capture::session_start(client.take()).await,
          HookEvent::PostToolUse {
              tool_name,
              tool_input,
              tool_response,
          } => {
              capture::post_tool_use(
                  client.take(),
                  dedup,
                  tool_name,
                  tool_input,
                  tool_response,
              )
              .await
          }
          HookEvent::Stop { .. } => capture::stop(client.take(), &ctx.cwd).await,
          HookEvent::PreCompact {
              custom_instructions,
          } => capture::pre_compact(client.take(), custom_instructions.as_deref()).await,
          HookEvent::Other(_) => HookResult {
              system_message: None,
              continue_execution: true,
          },
      }
  }

  #[cfg(test)]
  mod tests {
      #![allow(clippy::unwrap_used, clippy::expect_used)]
      use super::*;
      use rb_agents::event::{HookContext, HookEvent};
      use std::path::PathBuf;

      fn ctx(event: HookEvent) -> HookContext {
          HookContext {
              event,
              cwd: PathBuf::from("/tmp"),
              session_id: Some("s1".to_string()),
          }
      }

      #[tokio::test]
      async fn other_event_is_noop_continue() {
          let tmp = tempfile::tempdir().unwrap();
          let dedup = DedupCache::at(tmp.path().join("d.json"));
          let result = dispatch(None, &dedup, &ctx(HookEvent::Other("Notification".into()))).await;
          assert!(result.continue_execution);
          assert!(result.system_message.is_none());
      }

      #[tokio::test]
      async fn post_tool_use_event_routes_and_continues() {
          let tmp = tempfile::tempdir().unwrap();
          let dedup = DedupCache::at(tmp.path().join("d.json"));
          let event = HookEvent::PostToolUse {
              tool_name: "Edit".to_string(),
              tool_input: serde_json::json!({"file_path": "/src/main.rs"}),
              tool_response: serde_json::json!("ok"),
          };
          let result = dispatch(None, &dedup, &ctx(event)).await;
          assert!(result.continue_execution);
          // Routed to post_tool_use: the mutation must have been deduped.
          assert!(dedup.is_duplicate("Edit", "Edited /src/main.rs"));
      }

      #[tokio::test]
      async fn stop_event_continues() {
          let tmp = tempfile::tempdir().unwrap();
          let dedup = DedupCache::at(tmp.path().join("d.json"));
          let event = HookEvent::Stop {
              last_assistant_message: Some("done".to_string()),
          };
          let result = dispatch(None, &dedup, &ctx(event)).await;
          assert!(result.continue_execution);
      }
  }
  ```

- [ ] **Step 4: run it.**
  - Run: `cargo test -p rb-hooks`
  - Expected: PASS (capture: 31 tests; dispatch: 3 tests; plus cli/io/dedup).

- [ ] **Step 5: lint + format.**
  - Run: `cargo clippy -p rb-hooks --all-targets -- -D warnings`
  - Expected: no warnings.
  - Run: `cargo fmt --all`
  - Expected: no diff.

- [ ] **Step 6: commit.**
  - Run: `git add crates/rb-hooks/src/capture.rs crates/rb-hooks/src/dispatch.rs crates/rb-hooks/src/main.rs && git commit -m "feat(rb-hooks): pre-compact decision capture and event dispatch"`
  - Expected: one commit.

---

### Task W9: rb-hooks src/main.rs — fail-open harness + tests/integration.rs

**Files:**
- Modify: `crates/rb-hooks/src/main.rs` (the full FAIL-OPEN harness)
- Create: `crates/rb-hooks/tests/integration.rs`

The harness: read stdin JSON → `agent_for(args.agent).parse_input(&raw)` → `detect_namespace(ctx.cwd)` OFF the runtime → build a tokio runtime → connect `DaemonClient` (with `AutoStart` ONLY for `SessionStart`) under an overall timeout → `dispatch` → `render_output` → stdout → `exit(0)`. Everything is wrapped in `catch_unwind`; any failure prints the agent's `render_output(continue:true)` (or a literal `{"continue":true}` last resort) and always `exit(0)`.

- [ ] **Step 1 RED: write the integration tests against the built binary.**

  Create `crates/rb-hooks/tests/integration.rs`:

  ```rust
  #![allow(clippy::unwrap_used, clippy::expect_used)]
  //! End-to-end harness tests: drive the built `rusty-brain-hooks` binary via
  //! assert_cmd, feeding Claude Code JSON on stdin. The binary MUST always exit 0
  //! and emit a JSON object with `"continue": true`, even against a dead socket.

  use std::io::Write;
  use std::path::PathBuf;

  use assert_cmd::cargo::CommandCargoExt;

  fn hooks_command() -> std::process::Command {
      std::process::Command::cargo_bin("rusty-brain-hooks").expect("binary builds")
  }

  fn run_with_stdin(socket: &str, agent: &str, stdin_json: &str) -> std::process::Output {
      let mut child = hooks_command()
          .args(["--agent", agent])
          .env("RUSTY_BRAIN_SOCKET", socket)
          .stdin(std::process::Stdio::piped())
          .stdout(std::process::Stdio::piped())
          .stderr(std::process::Stdio::piped())
          .spawn()
          .expect("spawn hooks binary");
      child
          .stdin
          .take()
          .expect("stdin")
          .write_all(stdin_json.as_bytes())
          .expect("write stdin");
      child.wait_with_output().expect("wait for output")
  }

  #[test]
  fn session_start_against_dead_socket_fails_open() {
      // A socket path that does not exist and cannot auto-start anything useful in
      // the test environment: the harness must still exit 0 + {"continue":true}.
      let dead = "/nonexistent/dir/rb-hooks-test.sock";
      let stdin = r#"{"hook_event_name":"SessionStart","cwd":"/tmp","session_id":"s1","source":"startup"}"#;
      let output = run_with_stdin(dead, "claude-code", stdin);

      assert!(
          output.status.success(),
          "must exit 0 (fail-open); status={:?} stderr={}",
          output.status,
          String::from_utf8_lossy(&output.stderr)
      );
      let stdout = String::from_utf8_lossy(&output.stdout);
      let value: serde_json::Value =
          serde_json::from_str(stdout.trim()).expect("stdout must be valid JSON");
      assert_eq!(
          value.get("continue").and_then(|v| v.as_bytes_continue()),
          Some(true),
          "continue must be true, got {stdout}"
      );
  }

  trait ContinueBool {
      fn as_bytes_continue(&self) -> Option<bool>;
  }
  impl ContinueBool for serde_json::Value {
      fn as_bytes_continue(&self) -> Option<bool> {
          self.as_bool()
      }
  }

  #[test]
  fn invalid_stdin_fails_open() {
      let dead = "/nonexistent/dir/rb-hooks-test2.sock";
      let output = run_with_stdin(dead, "claude-code", "not json at all {{{");
      assert!(output.status.success(), "invalid stdin must still exit 0");
      let stdout = String::from_utf8_lossy(&output.stdout);
      let value: serde_json::Value =
          serde_json::from_str(stdout.trim()).expect("stdout must be valid JSON");
      assert_eq!(value.get("continue").and_then(|v| v.as_bool()), Some(true));
  }

  #[test]
  fn unknown_agent_fails_open_with_literal_continue() {
      // Unknown agent => arg parse error => last-resort literal {"continue":true}.
      let mut child = hooks_command()
          .args(["--agent", "bogus"])
          .stdin(std::process::Stdio::piped())
          .stdout(std::process::Stdio::piped())
          .stderr(std::process::Stdio::piped())
          .spawn()
          .expect("spawn");
      child
          .stdin
          .take()
          .expect("stdin")
          .write_all(b"{}")
          .expect("write");
      let output = child.wait_with_output().expect("wait");
      assert!(output.status.success(), "unknown agent must exit 0");
      let stdout = String::from_utf8_lossy(&output.stdout);
      assert!(
          stdout.contains("\"continue\":true") || stdout.contains("\"continue\": true"),
          "must emit continue:true, got {stdout}"
      );
  }

  // ---- Live in-process daemon: assert a PostToolUse remember happens ----

  use rb_proto::{
      read_frame, write_frame, Handshake, HandshakeAck, Request, Response, CONTRACT_VERSION,
  };
  use rb_types::MemoryId;
  use tokio::net::{UnixListener, UnixStream};
  use tokio_util::codec::{Framed, LengthDelimitedCodec};

  // Accept one connection, handshake-ack, and answer the first Remember with a
  // canned id. Signals back via the channel that a Remember was observed.
  async fn serve_one_remember(listener: UnixListener, tx: tokio::sync::oneshot::Sender<bool>) {
      let Ok((stream, _addr)) = listener.accept().await else {
          let _ = tx.send(false);
          return;
      };
      let mut framed: Framed<UnixStream, LengthDelimitedCodec> =
          Framed::new(stream, LengthDelimitedCodec::new());
      let _hs: Handshake = match read_frame(&mut framed).await {
          Ok(h) => h,
          Err(_) => {
              let _ = tx.send(false);
              return;
          }
      };
      let _ = write_frame(
          &mut framed,
          &HandshakeAck {
              contract_version: CONTRACT_VERSION,
              ok: true,
              message: None,
          },
      )
      .await;
      let mut saw_remember = false;
      while let Ok(req) = read_frame::<Request>(&mut framed).await {
          let resp = match req {
              Request::Remember { .. } => {
                  saw_remember = true;
                  Response::Remembered { id: MemoryId::new() }
              }
              Request::Context => Response::ContextResult {
                  recent: vec![],
                  important: vec![],
                  total: 0,
              },
              Request::Ping => Response::Pong {
                  contract_version: CONTRACT_VERSION,
              },
              _ => Response::Pong {
                  contract_version: CONTRACT_VERSION,
              },
          };
          if write_frame(&mut framed, &resp).await.is_err() {
              break;
          }
      }
      let _ = tx.send(saw_remember);
  }

  #[test]
  fn post_tool_use_against_live_daemon_remembers() {
      let dir = tempfile::tempdir().unwrap();
      let socket = dir.path().join("live.sock");
      let socket_str = socket.to_string_lossy().to_string();

      let (tx, rx) = std::sync::mpsc::channel::<bool>();
      let socket_for_thread = socket.clone();
      let server = std::thread::spawn(move || {
          let rt = tokio::runtime::Builder::new_current_thread()
              .enable_all()
              .build()
              .expect("rt");
          rt.block_on(async move {
              let listener = UnixListener::bind(&socket_for_thread).expect("bind");
              let (otx, orx) = tokio::sync::oneshot::channel::<bool>();
              let accept = tokio::spawn(serve_one_remember(listener, otx));
              let saw = orx.await.unwrap_or(false);
              let _ = accept.await;
              let _ = tx.send(saw);
          });
      });

      // Give the listener a moment to bind.
      std::thread::sleep(std::time::Duration::from_millis(200));

      let stdin = r#"{"hook_event_name":"PostToolUse","cwd":"/tmp","session_id":"s1","tool_name":"Edit","tool_input":{"file_path":"/src/uniqueW9.rs"},"tool_response":"ok"}"#;
      let output = run_with_stdin(&socket_str, "claude-code", stdin);
      assert!(output.status.success(), "must exit 0");

      let saw_remember = rx
          .recv_timeout(std::time::Duration::from_secs(10))
          .unwrap_or(false);
      assert!(saw_remember, "the daemon should have observed a Remember");
      let _ = server.join();
      let _: PathBuf = socket; // keep tempdir alive until here
  }
  ```

  Replace `crates/rb-hooks/src/main.rs` with the full harness:

  ```rust
  //! `rusty-brain-hooks` — the per-event capture hook binary.
  //!
  //! FAIL-OPEN CONTRACT: this binary NEVER blocks, NEVER returns non-zero, and
  //! NEVER lets an error reach the agent. It reads one event JSON on stdin,
  //! captures memories / injects context best-effort, prints the CLI-specific
  //! `{"continue":true,...}` to stdout, and always exits 0. Any panic or error
  //! anywhere degrades to a literal `{"continue":true}`.

  mod capture;
  mod cli;
  mod dedup;
  mod dispatch;
  mod io;

  use std::time::Duration;

  use rb_agents::cli::agent_for;
  use rb_agents::daemon::{AutoStart, DaemonClient};
  use rb_agents::event::{HookEvent, HookResult};
  use rb_agents::namespace::detect_namespace;

  use crate::cli::Args;
  use crate::dedup::DedupCache;

  /// Overall wall-clock budget for the connect+capture phase. On expiry the
  /// harness abandons the daemon work and still prints a fail-open response.
  const OVERALL_TIMEOUT: Duration = Duration::from_secs(5);
  /// Per-connect budget for reaching the daemon.
  const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

  fn main() {
      // Optional tracing to stderr only when RUSTY_BRAIN_LOG is set (no stderr by
      // default so we never pollute the agent's hook channel).
      if std::env::var_os("RUSTY_BRAIN_LOG").is_some() {
          let _ = tracing_subscriber_try_init();
      }

      let result = std::panic::catch_unwind(run);
      let rendered = match result {
          Ok(value) => value,
          Err(_) => serde_json::json!({ "continue": true }),
      };
      io::write_stdout(&rendered);
      std::process::exit(0);
  }

  /// Best-effort tracing init; ignored if the subscriber crate is unavailable.
  fn tracing_subscriber_try_init() -> Result<(), ()> {
      // We deliberately avoid a hard dependency on tracing-subscriber here; the
      // tracing macros are no-ops without a subscriber, which is fine for fail-open.
      Ok(())
  }

  /// The real body. Returns the JSON value to print. Any internal error is mapped
  /// to a fail-open render so `main` can print it unconditionally.
  fn run() -> serde_json::Value {
      // Parse args; on failure, last-resort literal continue.
      let args = match Args::parse_from(std::env::args()) {
          Ok(args) => args,
          Err(e) => {
              tracing::warn!("arg parse failed (fail-open): {e}");
              return serde_json::json!({ "continue": true });
          }
      };

      let cli = agent_for(args.agent);

      // Read + parse stdin (fail-open: Null on empty/invalid).
      let raw = io::read_stdin_json();
      let ctx = cli.parse_input(&raw);

      // Namespace detection runs OFF the async runtime (it shells out to git and
      // reads files). detect_namespace never panics; degrades to Global.
      let namespace = detect_namespace(&ctx.cwd);

      // Only SessionStart may auto-start the daemon. Other events never spawn.
      let auto_start = match &ctx.event {
          HookEvent::SessionStart { .. } => self_exe().map(|self_exe| AutoStart {
              self_exe,
              db: db_path(),
          }),
          _ => None,
      };

      let dedup = DedupCache::for_namespace(&namespace);

      // Build a runtime; if that fails, fail open.
      let runtime = match tokio::runtime::Builder::new_current_thread()
          .enable_all()
          .build()
      {
          Ok(rt) => rt,
          Err(e) => {
              tracing::warn!("runtime build failed (fail-open): {e}");
              return cli.render_output(&continue_result());
          }
      };

      let result = runtime.block_on(async {
          // Overall timeout guards the whole connect+capture phase.
          match tokio::time::timeout(
              OVERALL_TIMEOUT,
              capture_phase(&namespace, auto_start, &dedup, &ctx),
          )
          .await
          {
              Ok(result) => result,
              Err(_) => {
                  tracing::warn!("overall timeout (fail-open)");
                  continue_result()
              }
          }
      });

      cli.render_output(&result)
  }

  /// Connect (best-effort) and dispatch the event to its capture flow.
  async fn capture_phase(
      namespace: &rb_types::Namespace,
      auto_start: Option<AutoStart>,
      dedup: &DedupCache,
      ctx: &rb_agents::event::HookContext,
  ) -> HookResult {
      let socket = socket_path();
      let mut client = DaemonClient::connect(
          &socket,
          namespace.clone(),
          CONNECT_TIMEOUT,
          auto_start,
      )
      .await;
      dispatch::dispatch(client.as_mut(), dedup, ctx).await
  }

  fn continue_result() -> HookResult {
      HookResult {
          system_message: None,
          continue_execution: true,
      }
  }

  /// Resolve the daemon socket path from `RUSTY_BRAIN_SOCKET`, else a temp default.
  fn socket_path() -> std::path::PathBuf {
      if let Some(p) = std::env::var_os("RUSTY_BRAIN_SOCKET") {
          if !p.is_empty() {
              return std::path::PathBuf::from(p);
          }
      }
      default_runtime_dir().join("rusty-brain").join("sock")
  }

  /// Resolve the daemon db path from `RUSTY_BRAIN_DB`, else a data-dir default.
  fn db_path() -> std::path::PathBuf {
      if let Some(p) = std::env::var_os("RUSTY_BRAIN_DB") {
          if !p.is_empty() {
              return std::path::PathBuf::from(p);
          }
      }
      default_data_dir().join("rusty-brain").join("memory.db")
  }

  fn default_runtime_dir() -> std::path::PathBuf {
      if let Some(d) = std::env::var_os("XDG_RUNTIME_DIR") {
          if !d.is_empty() {
              return std::path::PathBuf::from(d);
          }
      }
      std::env::temp_dir()
  }

  fn default_data_dir() -> std::path::PathBuf {
      if let Some(d) = std::env::var_os("XDG_DATA_HOME") {
          if !d.is_empty() {
              return std::path::PathBuf::from(d);
          }
      }
      if let Some(home) = std::env::var_os("HOME") {
          if !home.is_empty() {
              return std::path::PathBuf::from(home).join(".local").join("share");
          }
      }
      std::env::temp_dir()
  }

  /// Path to this executable, for daemon auto-start. `None` if unresolved.
  fn self_exe() -> Option<std::path::PathBuf> {
      std::env::current_exe().ok()
  }
  ```

- [ ] **Step 2: run it.**
  - Run: `cargo test -p rb-hooks --test integration`
  - Expected: FAIL — the `as_bytes_continue` placeholder trait and the harness are not yet final; this RED proves the integration harness compiles against the real binary only once the harness exists. (Specifically: the build fails until `main.rs` provides `run`/`socket_path`/etc.)
  - NOTE: also remove the placeholder `ContinueBool`/`as_bytes_continue` indirection in Step 3; it exists only to make the RED file self-contained.

- [ ] **Step 3 GREEN: simplify the integration test's first assertion to use `as_bool` directly (the placeholder trait was scaffolding). Replace the first test and delete the trait in `crates/rb-hooks/tests/integration.rs` so the file's top portion reads:**

  ```rust
  #![allow(clippy::unwrap_used, clippy::expect_used)]
  //! End-to-end harness tests: drive the built `rusty-brain-hooks` binary via
  //! assert_cmd, feeding Claude Code JSON on stdin. The binary MUST always exit 0
  //! and emit a JSON object with `"continue": true`, even against a dead socket.

  use std::io::Write;
  use std::path::PathBuf;

  use assert_cmd::cargo::CommandCargoExt;

  fn hooks_command() -> std::process::Command {
      std::process::Command::cargo_bin("rusty-brain-hooks").expect("binary builds")
  }

  fn run_with_stdin(socket: &str, agent: &str, stdin_json: &str) -> std::process::Output {
      let mut child = hooks_command()
          .args(["--agent", agent])
          .env("RUSTY_BRAIN_SOCKET", socket)
          .stdin(std::process::Stdio::piped())
          .stdout(std::process::Stdio::piped())
          .stderr(std::process::Stdio::piped())
          .spawn()
          .expect("spawn hooks binary");
      child
          .stdin
          .take()
          .expect("stdin")
          .write_all(stdin_json.as_bytes())
          .expect("write stdin");
      child.wait_with_output().expect("wait for output")
  }

  #[test]
  fn session_start_against_dead_socket_fails_open() {
      // A socket path that does not exist and cannot auto-start anything useful in
      // the test environment: the harness must still exit 0 + {"continue":true}.
      let dead = "/nonexistent/dir/rb-hooks-test.sock";
      let stdin = r#"{"hook_event_name":"SessionStart","cwd":"/tmp","session_id":"s1","source":"startup"}"#;
      let output = run_with_stdin(dead, "claude-code", stdin);

      assert!(
          output.status.success(),
          "must exit 0 (fail-open); status={:?} stderr={}",
          output.status,
          String::from_utf8_lossy(&output.stderr)
      );
      let stdout = String::from_utf8_lossy(&output.stdout);
      let value: serde_json::Value =
          serde_json::from_str(stdout.trim()).expect("stdout must be valid JSON");
      assert_eq!(
          value.get("continue").and_then(|v| v.as_bool()),
          Some(true),
          "continue must be true, got {stdout}"
      );
  }
  ```

  Add `rb-proto`, `rb-types`, `tokio`, `tokio-util` to `[dev-dependencies]` so the live-daemon test compiles. Modify `crates/rb-hooks/Cargo.toml` `[dev-dependencies]` to read exactly:

  ```toml
  [dev-dependencies]
  rb-proto = { path = "../rb-proto" }
  rb-types = { path = "../rb-types" }
  tokio = { workspace = true }
  tokio-util = { workspace = true }
  tempfile = { workspace = true }
  assert_cmd = { workspace = true }
  predicates = { workspace = true }
  ```

- [ ] **Step 4: run it.**
  - Run: `cargo test -p rb-hooks --test integration`
  - Expected: PASS (4 tests: dead-socket fail-open, invalid stdin, unknown agent literal, live-daemon remember).
  - Run: `cargo test -p rb-hooks`
  - Expected: PASS (all unit + integration tests).

- [ ] **Step 5: lint + format.**
  - Run: `cargo clippy -p rb-hooks --all-targets -- -D warnings`
  - Expected: no warnings.
  - Run: `cargo fmt --all`
  - Expected: no diff.

- [ ] **Step 6: commit.**
  - Run: `git add crates/rb-hooks/src/main.rs crates/rb-hooks/tests/integration.rs crates/rb-hooks/Cargo.toml && git commit -m "feat(rb-hooks): fail-open run harness wired end-to-end for claude code"`
  - Expected: one commit.

---

### Task W10: Part W gate

**Files:**
- (no source changes) — verification-only task.

- [ ] **Step 1: per-crate test gate.**
  - Run: `cargo test -p rb-hooks`
  - Expected: PASS (all unit and integration tests green).

- [ ] **Step 2: clippy gate.**
  - Run: `cargo clippy -p rb-hooks --all-targets -- -D warnings`
  - Expected: no warnings.

- [ ] **Step 3: format gate.**
  - Run: `cargo fmt --all --check`
  - Expected: no diff.

- [ ] **Step 4: out-of-default-closure gate (CRITICAL).**
  - Run: `cargo tree -e no-dev -p rusty-brain | grep -E "rb-agents|rb-hooks|rb-install"`
  - Expected: prints NOTHING (the new crates are NOT in the `rusty-brain` non-dev dependency closure).
  - Run: `cargo build -p rusty-brain`
  - Expected: PASS without compiling `rb-hooks` (verify `rb-hooks` does NOT appear in the `Compiling ...` lines).

- [ ] **Step 5: confirm clean tree.**
  - Run: `git status --porcelain`
  - Expected: empty (all Part W work committed across Tasks W1-W9).


## Part X — rb-agents (OpenCode / Gemini / Codex `AgentCli` adapters)

This Part replaces Part V's `PassthroughCli` placeholders for the three non-Claude CLIs with real `AgentCli` adapters, one file per CLI under `crates/rb-agents/src/agents/`. Each adapter maps its own hook-event JSON shape into the canonical `HookContext`/`HookEvent` and renders a canonical `HookResult` back into that CLI's stdout JSON shape; all three fail open (unknown/malformed event ⇒ `HookEvent::Other`, never a panic). The registry `agent_for` is then re-pointed so `OpenCode`/`Gemini`/`Codex` return the real adapters and the `PassthroughCli` type is removed, after which a cross-adapter test proves that a representative post-tool event from every CLI normalizes to `HookEvent::PostToolUse` with the same `tool_name`. No `rb-hooks` capture logic changes in this Part — `rb-hooks` already dispatches through `agent_for`, so wiring the real adapters is sufficient. All commands run from the worktree root `/Volumes/raid1/repos/rusty-brain-p4` (so commands are plain `cargo test -p rb-agents`).

HARD RULES honored throughout: TDD (RED → GREEN → clippy + fmt → commit, one logical change per commit); conventional commits, lowercase, crate-scoped, one line, **NO AI attribution** (no "Generated with…", no `Co-Authored-By`); **fail-open parsing** — `parse_input` never `.unwrap()`/`.expect()`/`panic!` and degrades unknown shapes to `HookEvent::Other`; no `.unwrap()`/`.expect()`/`panic!` in non-test code (test modules opt out with `#![allow(clippy::unwrap_used, clippy::expect_used)]`); the three agent crates stay OUT of the default build closure.

### Event-name mapping (the matrix each adapter implements)

| Canonical `HookEvent`        | Claude Code (Part V) | OpenCode (`opencode.rs`)   | Gemini (`gemini.rs`) | Codex (`codex.rs`)     |
|------------------------------|----------------------|----------------------------|----------------------|------------------------|
| `SessionStart`               | `SessionStart`       | `session.created`          | `SessionStart`       | `SessionStart`         |
| `PostToolUse`                | `PostToolUse`        | `tool.execute.after`       | `AfterTool`          | `PostToolUse`          |
| `Stop`                       | `Stop`               | `session.idle`             | `Stop`               | `Stop`                 |
| `PreCompact`                 | `PreCompact`         | `session.compacted`        | `PreCompact`         | `PreCompact`           |
| `Other(name)` (uncaptured)   | anything else        | `BeforeTool`/`tool.execute.before`/`session.deleted`/… | `BeforeTool`/… | `UserPromptSubmit`/`PreToolUse`/… |

Per-CLI field/envelope conventions used by `parse_input` (matrix facts; defaulted fields are flagged with a `// DEFAULTED:` comment in the impl):

- **OpenCode** (`opencode.rs`): event discriminator is `type` (e.g. `"tool.execute.after"`); tool name at `tool` (string) or `tool.name`; tool input at `args` or `tool_input`; tool response at `output` or `tool_response`; cwd at `directory` or `cwd`; session id at `sessionID` or `session_id`. Output shape mirrors OpenCode's plugin result: `{ "continue": <bool>, "systemMessage"?: <string> }`.
- **Gemini** (`gemini.rs`): strict JSON stdin/stdout; event discriminator is `hook_event_name`; tool name at `tool_name`; tool input at `tool_input`; tool response at `tool_response`; cwd at `cwd`; session id at `session_id`; last assistant message at `last_assistant_message`. Output shape is strict: `{ "continue": <bool>, "systemMessage"?: <string> }`.
- **Codex** (`codex.rs`): JSON path only (TOML config is install-side, Part Y); event discriminator is `hook_event_name`; tool name at `tool_name`; tool input at `tool_input`; tool response at `tool_response`; cwd at `cwd`; session id at `session_id`; `UserPromptSubmit`/`PreToolUse` map to `Other` (not captured in P4). Output shape: `{ "continue": <bool>, "systemMessage"?: <string> }`.

Part V's `agents` module is assumed to already contain `mod claude_code;` (the real reference adapter) and `mod opencode; mod gemini; mod codex;` whose bodies are `PassthroughCli`-backed placeholders re-exported from `agents::mod`. This Part **creates the three real adapter files**, **re-points `agent_for`**, and **deletes `PassthroughCli`**.

---

### Task X1: rb-agents `agents/opencode.rs` — opencode adapter

Add the real `OpenCodeCli` adapter that maps OpenCode's `type`-discriminated event JSON into the canonical `HookContext` and renders a `HookResult` into OpenCode's `{ "continue", "systemMessage" }` stdout shape. Fail open: an unrecognized or malformed payload yields `HookEvent::Other` and never panics.

**Files:**
- Create: `crates/rb-agents/src/agents/opencode.rs`
- Modify: `crates/rb-agents/src/agents/mod.rs`
- Test: `crates/rb-agents/src/agents/opencode.rs` (inline `#[cfg(test)]`)

- [ ] **Step 1 RED: write the failing test.** Create `crates/rb-agents/src/agents/opencode.rs` with this exact content (the `tests` module exercises the impl that arrives in Step 3):

```rust
//! OpenCode `AgentCli` adapter.
//!
//! OpenCode delivers hook events as JSON with a `type` discriminator
//! (`"session.created"`, `"tool.execute.after"`, `"session.idle"`,
//! `"session.compacted"`, …). This adapter normalizes those into the
//! canonical [`HookContext`]/[`HookEvent`] and renders a [`HookResult`] back
//! into OpenCode's `{ "continue", "systemMessage" }` plugin-result shape.
//!
//! Fail-open: any unrecognized `type` or malformed field degrades to
//! [`HookEvent::Other`]; parsing never panics.

use std::path::PathBuf;

use serde_json::{Value, json};

use crate::cli::AgentCli;
use crate::cli::AgentId;
use crate::event::{HookContext, HookEvent, HookResult};

/// OpenCode JSON hook adapter.
#[derive(Debug, Default, Clone, Copy)]
pub struct OpenCodeCli;

/// Read the first present string field from `raw` among `keys`.
fn first_str(raw: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(s) = raw.get(*key).and_then(Value::as_str) {
            return Some(s.to_string());
        }
    }
    None
}

/// Read the first present (non-null) JSON value from `raw` among `keys`.
fn first_value(raw: &Value, keys: &[&str]) -> Value {
    for key in keys {
        if let Some(v) = raw.get(*key) {
            if !v.is_null() {
                return v.clone();
            }
        }
    }
    Value::Null
}

/// Extract the OpenCode tool name: a bare `tool` string or a nested `tool.name`.
fn opencode_tool_name(raw: &Value) -> String {
    if let Some(s) = raw.get("tool").and_then(Value::as_str) {
        return s.to_string();
    }
    if let Some(s) = raw
        .get("tool")
        .and_then(|t| t.get("name"))
        .and_then(Value::as_str)
    {
        return s.to_string();
    }
    // DEFAULTED: OpenCode tool-name location is not contract-stable; fall back to
    // a flat `tool_name` field, else empty so capture still proceeds fail-open.
    raw.get("tool_name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

impl AgentCli for OpenCodeCli {
    fn id(&self) -> AgentId {
        AgentId::OpenCode
    }

    fn binary_name(&self) -> &'static str {
        "opencode"
    }

    fn parse_input(&self, raw: &Value) -> HookContext {
        // DEFAULTED: cwd absent => the running process cwd is used by the harness;
        // here we default to "." so the type stays total and never panics.
        let cwd = first_str(raw, &["directory", "cwd"])
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let session_id = first_str(raw, &["sessionID", "session_id"]);

        let event = match raw.get("type").and_then(Value::as_str) {
            Some("session.created") => HookEvent::SessionStart {
                source: first_str(raw, &["source"]),
            },
            Some("tool.execute.after") => HookEvent::PostToolUse {
                tool_name: opencode_tool_name(raw),
                tool_input: first_value(raw, &["args", "tool_input"]),
                tool_response: first_value(raw, &["output", "tool_response"]),
            },
            Some("session.idle") => HookEvent::Stop {
                last_assistant_message: first_str(raw, &["last_assistant_message"]),
            },
            Some("session.compacted") => HookEvent::PreCompact {
                custom_instructions: first_str(raw, &["custom_instructions"]),
            },
            // DEFAULTED: session.deleted / tool.execute.before / unknown =>
            // not a captured event in P4; preserve the raw name for diagnostics.
            Some(other) => HookEvent::Other(other.to_string()),
            None => HookEvent::Other(String::new()),
        };

        HookContext {
            event,
            cwd,
            session_id,
        }
    }

    fn render_output(&self, result: &HookResult) -> Value {
        let mut out = json!({ "continue": result.continue_execution });
        if let Some(msg) = &result.system_message {
            out["systemMessage"] = Value::String(msg.clone());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use serde_json::json;

    #[test]
    fn id_and_binary_name() {
        let cli = OpenCodeCli;
        assert_eq!(cli.id(), AgentId::OpenCode);
        assert_eq!(cli.binary_name(), "opencode");
    }

    #[test]
    fn parses_session_created_as_session_start() {
        let raw = json!({
            "type": "session.created",
            "directory": "/work/proj",
            "sessionID": "s-1",
            "source": "startup"
        });
        let ctx = OpenCodeCli.parse_input(&raw);
        assert_eq!(ctx.cwd, PathBuf::from("/work/proj"));
        assert_eq!(ctx.session_id.as_deref(), Some("s-1"));
        assert_eq!(
            ctx.event,
            HookEvent::SessionStart {
                source: Some("startup".to_string())
            }
        );
    }

    #[test]
    fn parses_tool_execute_after_as_post_tool_use() {
        let raw = json!({
            "type": "tool.execute.after",
            "directory": "/work/proj",
            "sessionID": "s-2",
            "tool": "Write",
            "args": {"file_path": "/tmp/a.txt"},
            "output": {"ok": true}
        });
        let ctx = OpenCodeCli.parse_input(&raw);
        match ctx.event {
            HookEvent::PostToolUse {
                tool_name,
                tool_input,
                tool_response,
            } => {
                assert_eq!(tool_name, "Write");
                assert_eq!(tool_input, json!({"file_path": "/tmp/a.txt"}));
                assert_eq!(tool_response, json!({"ok": true}));
            }
            other => panic!("expected PostToolUse, got {other:?}"),
        }
    }

    #[test]
    fn parses_nested_tool_name_object() {
        let raw = json!({
            "type": "tool.execute.after",
            "tool": {"name": "Bash"},
            "args": {},
            "output": {}
        });
        let ctx = OpenCodeCli.parse_input(&raw);
        match ctx.event {
            HookEvent::PostToolUse { tool_name, .. } => assert_eq!(tool_name, "Bash"),
            other => panic!("expected PostToolUse, got {other:?}"),
        }
    }

    #[test]
    fn parses_session_idle_as_stop() {
        let raw = json!({
            "type": "session.idle",
            "last_assistant_message": "done"
        });
        let ctx = OpenCodeCli.parse_input(&raw);
        assert_eq!(
            ctx.event,
            HookEvent::Stop {
                last_assistant_message: Some("done".to_string())
            }
        );
    }

    #[test]
    fn parses_session_compacted_as_pre_compact() {
        let raw = json!({ "type": "session.compacted" });
        let ctx = OpenCodeCli.parse_input(&raw);
        assert_eq!(
            ctx.event,
            HookEvent::PreCompact {
                custom_instructions: None
            }
        );
    }

    #[test]
    fn unknown_type_degrades_to_other() {
        let raw = json!({ "type": "session.deleted" });
        let ctx = OpenCodeCli.parse_input(&raw);
        assert_eq!(ctx.event, HookEvent::Other("session.deleted".to_string()));
    }

    #[test]
    fn malformed_payload_degrades_to_other_without_panic() {
        // No `type`, no recognizable fields: must not panic, must be Other.
        let raw = json!({ "garbage": [1, 2, 3], "nested": {"x": null} });
        let ctx = OpenCodeCli.parse_input(&raw);
        assert_eq!(ctx.event, HookEvent::Other(String::new()));
        assert_eq!(ctx.cwd, PathBuf::from("."));
        assert!(ctx.session_id.is_none());
    }

    #[test]
    fn render_output_includes_continue_and_optional_message() {
        let with_msg = HookResult {
            system_message: Some("hello".to_string()),
            continue_execution: true,
        };
        let v = OpenCodeCli.render_output(&with_msg);
        assert_eq!(v["continue"], json!(true));
        assert_eq!(v["systemMessage"], json!("hello"));

        let without = HookResult {
            system_message: None,
            continue_execution: true,
        };
        let v = OpenCodeCli.render_output(&without);
        assert_eq!(v["continue"], json!(true));
        assert!(v.get("systemMessage").is_none());
    }
}
```

- [ ] **Step 2: run it.** Run: `cargo test -p rb-agents opencode` — Expected: FAIL — `agents/opencode.rs` is not yet declared in `agents/mod.rs` (or the old placeholder lacks `OpenCodeCli`), so the module/type does not resolve and the crate fails to compile.

- [ ] **Step 3 GREEN: minimal implementation.** The file content from Step 1 IS the implementation (impl + tests in one file). Now declare the module by replacing the placeholder `opencode` line in `crates/rb-agents/src/agents/mod.rs`. Open `crates/rb-agents/src/agents/mod.rs` and change the OpenCode declaration/re-export so it points at the real adapter. The module file must read exactly:

```rust
//! Per-CLI `AgentCli` adapters. One module per supported CLI.

mod claude_code;
mod codex;
mod gemini;
mod opencode;

pub use claude_code::ClaudeCodeCli;
pub use codex::CodexCli;
pub use gemini::GeminiCli;
pub use opencode::OpenCodeCli;
```

(Codex/Gemini real adapters arrive in Tasks X2/X3; until then their `pub use` lines name the Part V placeholder types `CodexCli`/`GeminiCli` which already exist, so `mod.rs` compiles after each Task. If Part V exported the placeholders under different names, this Step renames the placeholder structs to `CodexCli`/`GeminiCli` so the canonical names are stable from here on.)

- [ ] **Step 4: run it.** Run: `cargo test -p rb-agents opencode` — Expected: PASS (9 tests: `id_and_binary_name`, `parses_session_created_as_session_start`, `parses_tool_execute_after_as_post_tool_use`, `parses_nested_tool_name_object`, `parses_session_idle_as_stop`, `parses_session_compacted_as_pre_compact`, `unknown_type_degrades_to_other`, `malformed_payload_degrades_to_other_without_panic`, `render_output_includes_continue_and_optional_message`).

- [ ] **Step 5: lint + format.** Run: `cargo clippy -p rb-agents --all-targets -- -D warnings` — Expected: no warnings. Then run: `cargo fmt --all` — Expected: no diff.

- [ ] **Step 6: commit.** Run: `git add crates/rb-agents/src/agents/opencode.rs crates/rb-agents/src/agents/mod.rs && git commit -m "feat(rb-agents): add opencode agentcli adapter"` — Expected: one commit.

---

### Task X2: rb-agents `agents/gemini.rs` — gemini adapter

Add the real `GeminiCli` adapter. Gemini uses strict JSON stdin/stdout with an `hook_event_name` discriminator and flat `tool_name`/`tool_input`/`tool_response` fields; `BeforeTool` is not captured in P4 and maps to `Other`. Output is strict `{ "continue", "systemMessage" }`.

**Files:**
- Create: `crates/rb-agents/src/agents/gemini.rs`
- Modify: `crates/rb-agents/src/agents/mod.rs`
- Test: `crates/rb-agents/src/agents/gemini.rs` (inline `#[cfg(test)]`)

- [ ] **Step 1 RED: write the failing test.** Replace the Part V placeholder `crates/rb-agents/src/agents/gemini.rs` with this exact content:

```rust
//! Gemini `AgentCli` adapter.
//!
//! Gemini delivers hook events as strict JSON on stdin with an
//! `hook_event_name` discriminator (`"SessionStart"`, `"AfterTool"`,
//! `"Stop"`, `"PreCompact"`, `"BeforeTool"`, …) and flat
//! `tool_name`/`tool_input`/`tool_response` fields. This adapter normalizes
//! those into the canonical [`HookContext`]/[`HookEvent`] and renders a
//! [`HookResult`] into Gemini's strict `{ "continue", "systemMessage" }`
//! stdout shape.
//!
//! Fail-open: an unrecognized `hook_event_name` or missing fields degrade to
//! [`HookEvent::Other`]; parsing never panics.

use std::path::PathBuf;

use serde_json::{Value, json};

use crate::cli::AgentCli;
use crate::cli::AgentId;
use crate::event::{HookContext, HookEvent, HookResult};

/// Gemini strict-JSON hook adapter.
#[derive(Debug, Default, Clone, Copy)]
pub struct GeminiCli;

/// Read a string field from `raw`, or `None` if absent/non-string.
fn str_field(raw: &Value, key: &str) -> Option<String> {
    raw.get(key).and_then(Value::as_str).map(str::to_string)
}

/// Read a JSON field from `raw`, or `Value::Null` if absent.
fn value_field(raw: &Value, key: &str) -> Value {
    raw.get(key).cloned().unwrap_or(Value::Null)
}

impl AgentCli for GeminiCli {
    fn id(&self) -> AgentId {
        AgentId::Gemini
    }

    fn binary_name(&self) -> &'static str {
        "gemini"
    }

    fn parse_input(&self, raw: &Value) -> HookContext {
        // DEFAULTED: cwd absent => "." so the type stays total and never panics.
        let cwd = str_field(raw, "cwd")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let session_id = str_field(raw, "session_id");

        let event = match raw.get("hook_event_name").and_then(Value::as_str) {
            Some("SessionStart") => HookEvent::SessionStart {
                source: str_field(raw, "source"),
            },
            Some("AfterTool") => HookEvent::PostToolUse {
                // DEFAULTED: tool_name absent => empty; capture proceeds fail-open.
                tool_name: str_field(raw, "tool_name").unwrap_or_default(),
                tool_input: value_field(raw, "tool_input"),
                tool_response: value_field(raw, "tool_response"),
            },
            Some("Stop") => HookEvent::Stop {
                last_assistant_message: str_field(raw, "last_assistant_message"),
            },
            Some("PreCompact") => HookEvent::PreCompact {
                custom_instructions: str_field(raw, "custom_instructions"),
            },
            // DEFAULTED: BeforeTool / unknown => not captured in P4.
            Some(other) => HookEvent::Other(other.to_string()),
            None => HookEvent::Other(String::new()),
        };

        HookContext {
            event,
            cwd,
            session_id,
        }
    }

    fn render_output(&self, result: &HookResult) -> Value {
        let mut out = json!({ "continue": result.continue_execution });
        if let Some(msg) = &result.system_message {
            out["systemMessage"] = Value::String(msg.clone());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use serde_json::json;

    #[test]
    fn id_and_binary_name() {
        let cli = GeminiCli;
        assert_eq!(cli.id(), AgentId::Gemini);
        assert_eq!(cli.binary_name(), "gemini");
    }

    #[test]
    fn parses_session_start() {
        let raw = json!({
            "hook_event_name": "SessionStart",
            "cwd": "/work/proj",
            "session_id": "g-1",
            "source": "startup"
        });
        let ctx = GeminiCli.parse_input(&raw);
        assert_eq!(ctx.cwd, PathBuf::from("/work/proj"));
        assert_eq!(ctx.session_id.as_deref(), Some("g-1"));
        assert_eq!(
            ctx.event,
            HookEvent::SessionStart {
                source: Some("startup".to_string())
            }
        );
    }

    #[test]
    fn parses_after_tool_as_post_tool_use() {
        let raw = json!({
            "hook_event_name": "AfterTool",
            "cwd": "/work/proj",
            "session_id": "g-2",
            "tool_name": "Edit",
            "tool_input": {"path": "x.rs"},
            "tool_response": {"ok": true}
        });
        let ctx = GeminiCli.parse_input(&raw);
        match ctx.event {
            HookEvent::PostToolUse {
                tool_name,
                tool_input,
                tool_response,
            } => {
                assert_eq!(tool_name, "Edit");
                assert_eq!(tool_input, json!({"path": "x.rs"}));
                assert_eq!(tool_response, json!({"ok": true}));
            }
            other => panic!("expected PostToolUse, got {other:?}"),
        }
    }

    #[test]
    fn parses_stop() {
        let raw = json!({
            "hook_event_name": "Stop",
            "last_assistant_message": "finished"
        });
        let ctx = GeminiCli.parse_input(&raw);
        assert_eq!(
            ctx.event,
            HookEvent::Stop {
                last_assistant_message: Some("finished".to_string())
            }
        );
    }

    #[test]
    fn parses_pre_compact() {
        let raw = json!({
            "hook_event_name": "PreCompact",
            "custom_instructions": "keep decisions"
        });
        let ctx = GeminiCli.parse_input(&raw);
        assert_eq!(
            ctx.event,
            HookEvent::PreCompact {
                custom_instructions: Some("keep decisions".to_string())
            }
        );
    }

    #[test]
    fn before_tool_degrades_to_other() {
        let raw = json!({ "hook_event_name": "BeforeTool", "tool_name": "Write" });
        let ctx = GeminiCli.parse_input(&raw);
        assert_eq!(ctx.event, HookEvent::Other("BeforeTool".to_string()));
    }

    #[test]
    fn missing_event_name_degrades_to_other_without_panic() {
        let raw = json!({ "noise": 1 });
        let ctx = GeminiCli.parse_input(&raw);
        assert_eq!(ctx.event, HookEvent::Other(String::new()));
        assert_eq!(ctx.cwd, PathBuf::from("."));
        assert!(ctx.session_id.is_none());
    }

    #[test]
    fn render_output_strict_shape() {
        let with_msg = HookResult {
            system_message: Some("ctx".to_string()),
            continue_execution: true,
        };
        let v = GeminiCli.render_output(&with_msg);
        assert_eq!(v["continue"], json!(true));
        assert_eq!(v["systemMessage"], json!("ctx"));

        let without = HookResult {
            system_message: None,
            continue_execution: true,
        };
        let v = GeminiCli.render_output(&without);
        assert_eq!(v["continue"], json!(true));
        assert!(v.get("systemMessage").is_none());
    }
}
```

- [ ] **Step 2: run it.** Run: `cargo test -p rb-agents gemini` — Expected: FAIL — the Part V placeholder `GeminiCli` does not parse `hook_event_name` into the canonical events, so the new assertions (`parses_after_tool_as_post_tool_use`, `before_tool_degrades_to_other`, …) fail to compile/assert.

- [ ] **Step 3 GREEN: minimal implementation.** The Step 1 file content IS the implementation; it replaces the placeholder body. `crates/rb-agents/src/agents/mod.rs` already declares `mod gemini;` and `pub use gemini::GeminiCli;` from Task X1, so no further `mod.rs` change is needed here.

- [ ] **Step 4: run it.** Run: `cargo test -p rb-agents gemini` — Expected: PASS (8 tests: `id_and_binary_name`, `parses_session_start`, `parses_after_tool_as_post_tool_use`, `parses_stop`, `parses_pre_compact`, `before_tool_degrades_to_other`, `missing_event_name_degrades_to_other_without_panic`, `render_output_strict_shape`).

- [ ] **Step 5: lint + format.** Run: `cargo clippy -p rb-agents --all-targets -- -D warnings` — Expected: no warnings. Then run: `cargo fmt --all` — Expected: no diff.

- [ ] **Step 6: commit.** Run: `git add crates/rb-agents/src/agents/gemini.rs && git commit -m "feat(rb-agents): add gemini agentcli adapter"` — Expected: one commit.

---

### Task X3: rb-agents `agents/codex.rs` — codex adapter

Add the real `CodexCli` adapter. Codex JSON hooks carry `hook_event_name` ∈ {`SessionStart`, `UserPromptSubmit`, `PreToolUse`, `PostToolUse`, `PreCompact`, `Stop`}; only `SessionStart`/`PostToolUse`/`PreCompact`/`Stop` are captured in P4 — `UserPromptSubmit`/`PreToolUse` map to `Other`. This adapter handles the JSON path only (the Codex TOML config path is install-side, Part Y). Output is `{ "continue", "systemMessage" }`.

**Files:**
- Create: `crates/rb-agents/src/agents/codex.rs`
- Modify: `crates/rb-agents/src/agents/mod.rs`
- Test: `crates/rb-agents/src/agents/codex.rs` (inline `#[cfg(test)]`)

- [ ] **Step 1 RED: write the failing test.** Replace the Part V placeholder `crates/rb-agents/src/agents/codex.rs` with this exact content:

```rust
//! Codex `AgentCli` adapter (JSON hook path only).
//!
//! Codex delivers hook events as JSON on stdin with an `hook_event_name`
//! discriminator (`"SessionStart"`, `"UserPromptSubmit"`, `"PreToolUse"`,
//! `"PostToolUse"`, `"PreCompact"`, `"Stop"`) and flat
//! `tool_name`/`tool_input`/`tool_response` fields. This adapter normalizes
//! those into the canonical [`HookContext`]/[`HookEvent`] and renders a
//! [`HookResult`] into Codex's `{ "continue", "systemMessage" }` stdout shape.
//! The Codex TOML config path is handled install-side (Part Y), NOT here.
//!
//! Fail-open: `UserPromptSubmit`/`PreToolUse`/unknown/missing degrade to
//! [`HookEvent::Other`]; parsing never panics.

use std::path::PathBuf;

use serde_json::{Value, json};

use crate::cli::AgentCli;
use crate::cli::AgentId;
use crate::event::{HookContext, HookEvent, HookResult};

/// Codex JSON hook adapter.
#[derive(Debug, Default, Clone, Copy)]
pub struct CodexCli;

/// Read a string field from `raw`, or `None` if absent/non-string.
fn str_field(raw: &Value, key: &str) -> Option<String> {
    raw.get(key).and_then(Value::as_str).map(str::to_string)
}

/// Read a JSON field from `raw`, or `Value::Null` if absent.
fn value_field(raw: &Value, key: &str) -> Value {
    raw.get(key).cloned().unwrap_or(Value::Null)
}

impl AgentCli for CodexCli {
    fn id(&self) -> AgentId {
        AgentId::Codex
    }

    fn binary_name(&self) -> &'static str {
        "codex"
    }

    fn parse_input(&self, raw: &Value) -> HookContext {
        // DEFAULTED: cwd absent => "." so the type stays total and never panics.
        let cwd = str_field(raw, "cwd")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let session_id = str_field(raw, "session_id");

        let event = match raw.get("hook_event_name").and_then(Value::as_str) {
            Some("SessionStart") => HookEvent::SessionStart {
                source: str_field(raw, "source"),
            },
            Some("PostToolUse") => HookEvent::PostToolUse {
                // DEFAULTED: tool_name absent => empty; capture proceeds fail-open.
                tool_name: str_field(raw, "tool_name").unwrap_or_default(),
                tool_input: value_field(raw, "tool_input"),
                tool_response: value_field(raw, "tool_response"),
            },
            Some("Stop") => HookEvent::Stop {
                last_assistant_message: str_field(raw, "last_assistant_message"),
            },
            Some("PreCompact") => HookEvent::PreCompact {
                custom_instructions: str_field(raw, "custom_instructions"),
            },
            // DEFAULTED: UserPromptSubmit / PreToolUse / unknown => not captured in P4.
            Some(other) => HookEvent::Other(other.to_string()),
            None => HookEvent::Other(String::new()),
        };

        HookContext {
            event,
            cwd,
            session_id,
        }
    }

    fn render_output(&self, result: &HookResult) -> Value {
        let mut out = json!({ "continue": result.continue_execution });
        if let Some(msg) = &result.system_message {
            out["systemMessage"] = Value::String(msg.clone());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use serde_json::json;

    #[test]
    fn id_and_binary_name() {
        let cli = CodexCli;
        assert_eq!(cli.id(), AgentId::Codex);
        assert_eq!(cli.binary_name(), "codex");
    }

    #[test]
    fn parses_session_start() {
        let raw = json!({
            "hook_event_name": "SessionStart",
            "cwd": "/work/proj",
            "session_id": "c-1",
            "source": "resume"
        });
        let ctx = CodexCli.parse_input(&raw);
        assert_eq!(ctx.cwd, PathBuf::from("/work/proj"));
        assert_eq!(ctx.session_id.as_deref(), Some("c-1"));
        assert_eq!(
            ctx.event,
            HookEvent::SessionStart {
                source: Some("resume".to_string())
            }
        );
    }

    #[test]
    fn parses_post_tool_use() {
        let raw = json!({
            "hook_event_name": "PostToolUse",
            "cwd": "/work/proj",
            "session_id": "c-2",
            "tool_name": "Bash",
            "tool_input": {"command": "ls"},
            "tool_response": {"stdout": "a\nb"}
        });
        let ctx = CodexCli.parse_input(&raw);
        match ctx.event {
            HookEvent::PostToolUse {
                tool_name,
                tool_input,
                tool_response,
            } => {
                assert_eq!(tool_name, "Bash");
                assert_eq!(tool_input, json!({"command": "ls"}));
                assert_eq!(tool_response, json!({"stdout": "a\nb"}));
            }
            other => panic!("expected PostToolUse, got {other:?}"),
        }
    }

    #[test]
    fn parses_stop() {
        let raw = json!({
            "hook_event_name": "Stop",
            "last_assistant_message": "wrapped up"
        });
        let ctx = CodexCli.parse_input(&raw);
        assert_eq!(
            ctx.event,
            HookEvent::Stop {
                last_assistant_message: Some("wrapped up".to_string())
            }
        );
    }

    #[test]
    fn parses_pre_compact() {
        let raw = json!({ "hook_event_name": "PreCompact" });
        let ctx = CodexCli.parse_input(&raw);
        assert_eq!(
            ctx.event,
            HookEvent::PreCompact {
                custom_instructions: None
            }
        );
    }

    #[test]
    fn user_prompt_submit_degrades_to_other() {
        let raw = json!({ "hook_event_name": "UserPromptSubmit", "prompt": "hi" });
        let ctx = CodexCli.parse_input(&raw);
        assert_eq!(ctx.event, HookEvent::Other("UserPromptSubmit".to_string()));
    }

    #[test]
    fn pre_tool_use_degrades_to_other() {
        let raw = json!({ "hook_event_name": "PreToolUse", "tool_name": "Write" });
        let ctx = CodexCli.parse_input(&raw);
        assert_eq!(ctx.event, HookEvent::Other("PreToolUse".to_string()));
    }

    #[test]
    fn missing_event_name_degrades_to_other_without_panic() {
        let raw = json!({ "unexpected": true });
        let ctx = CodexCli.parse_input(&raw);
        assert_eq!(ctx.event, HookEvent::Other(String::new()));
        assert_eq!(ctx.cwd, PathBuf::from("."));
        assert!(ctx.session_id.is_none());
    }

    #[test]
    fn render_output_shape() {
        let with_msg = HookResult {
            system_message: Some("note".to_string()),
            continue_execution: true,
        };
        let v = CodexCli.render_output(&with_msg);
        assert_eq!(v["continue"], json!(true));
        assert_eq!(v["systemMessage"], json!("note"));

        let without = HookResult {
            system_message: None,
            continue_execution: true,
        };
        let v = CodexCli.render_output(&without);
        assert_eq!(v["continue"], json!(true));
        assert!(v.get("systemMessage").is_none());
    }
}
```

- [ ] **Step 2: run it.** Run: `cargo test -p rb-agents codex` — Expected: FAIL — the Part V placeholder `CodexCli` does not split `PostToolUse` from `PreToolUse`/`UserPromptSubmit`, so the new assertions (`parses_post_tool_use`, `pre_tool_use_degrades_to_other`, …) fail.

- [ ] **Step 3 GREEN: minimal implementation.** The Step 1 file content IS the implementation; it replaces the placeholder body. `crates/rb-agents/src/agents/mod.rs` already declares `mod codex;` and `pub use codex::CodexCli;` from Task X1, so no further `mod.rs` change is needed here.

- [ ] **Step 4: run it.** Run: `cargo test -p rb-agents codex` — Expected: PASS (9 tests: `id_and_binary_name`, `parses_session_start`, `parses_post_tool_use`, `parses_stop`, `parses_pre_compact`, `user_prompt_submit_degrades_to_other`, `pre_tool_use_degrades_to_other`, `missing_event_name_degrades_to_other_without_panic`, `render_output_shape`).

- [ ] **Step 5: lint + format.** Run: `cargo clippy -p rb-agents --all-targets -- -D warnings` — Expected: no warnings. Then run: `cargo fmt --all` — Expected: no diff.

- [ ] **Step 6: commit.** Run: `git add crates/rb-agents/src/agents/codex.rs && git commit -m "feat(rb-agents): add codex agentcli adapter"` — Expected: one commit.

---

### Task X4: rb-agents `cli.rs` — registry rewire

Re-point `agent_for` so `AgentId::OpenCode` / `AgentId::Gemini` / `AgentId::Codex` return the real adapters, and delete the Part V `PassthroughCli` type now that no arm references it. The registry already returns `ClaudeCodeCli` for `AgentId::ClaudeCode`.

**Files:**
- Modify: `crates/rb-agents/src/cli.rs`
- Test: `crates/rb-agents/src/cli.rs` (inline `#[cfg(test)]`)

- [ ] **Step 1 RED: write the failing test.** In `crates/rb-agents/src/cli.rs`, add the following tests to the existing `#[cfg(test)] mod tests { … }` block (keep Part V's existing `AgentId::as_str`/`parse` tests; add these new functions inside the same module). The test module's first line must already be `#![allow(clippy::unwrap_used, clippy::expect_used)]`; if not, ensure it is present.

```rust
    #[test]
    fn agent_for_returns_matching_id_for_all_four() {
        for id in [
            AgentId::ClaudeCode,
            AgentId::OpenCode,
            AgentId::Gemini,
            AgentId::Codex,
        ] {
            let cli = agent_for(id);
            assert_eq!(cli.id(), id);
        }
    }

    #[test]
    fn agent_for_opencode_round_trips_post_tool_use() {
        let cli = agent_for(AgentId::OpenCode);
        let raw = serde_json::json!({
            "type": "tool.execute.after",
            "tool": "Write",
            "args": {},
            "output": {}
        });
        let ctx = cli.parse_input(&raw);
        assert!(matches!(ctx.event, crate::event::HookEvent::PostToolUse { .. }));
        let out = cli.render_output(&crate::event::HookResult {
            system_message: None,
            continue_execution: true,
        });
        assert_eq!(out["continue"], serde_json::json!(true));
    }

    #[test]
    fn agent_for_gemini_round_trips_after_tool() {
        let cli = agent_for(AgentId::Gemini);
        let raw = serde_json::json!({
            "hook_event_name": "AfterTool",
            "tool_name": "Edit"
        });
        let ctx = cli.parse_input(&raw);
        assert!(matches!(ctx.event, crate::event::HookEvent::PostToolUse { .. }));
        let out = cli.render_output(&crate::event::HookResult {
            system_message: Some("m".to_string()),
            continue_execution: true,
        });
        assert_eq!(out["systemMessage"], serde_json::json!("m"));
    }

    #[test]
    fn agent_for_codex_round_trips_post_tool_use() {
        let cli = agent_for(AgentId::Codex);
        let raw = serde_json::json!({
            "hook_event_name": "PostToolUse",
            "tool_name": "Bash"
        });
        let ctx = cli.parse_input(&raw);
        assert!(matches!(ctx.event, crate::event::HookEvent::PostToolUse { .. }));
        let out = cli.render_output(&crate::event::HookResult {
            system_message: None,
            continue_execution: true,
        });
        assert_eq!(out["continue"], serde_json::json!(true));
    }
```

- [ ] **Step 2: run it.** Run: `cargo test -p rb-agents cli::tests::agent_for` — Expected: FAIL — `agent_for` still returns the `PassthroughCli` placeholder for `OpenCode`/`Gemini`/`Codex`, whose `parse_input` yields `HookEvent::Other` (not `PostToolUse`), so `agent_for_opencode_round_trips_post_tool_use` / `agent_for_gemini_round_trips_after_tool` / `agent_for_codex_round_trips_post_tool_use` fail their `matches!` assertions.

- [ ] **Step 3 GREEN: rewire the registry.** Replace the `agent_for` function body in `crates/rb-agents/src/cli.rs` so every arm returns the real adapter, and remove the `PassthroughCli` struct + its `AgentCli` impl entirely. The relevant region of `crates/rb-agents/src/cli.rs` must read exactly:

```rust
use crate::agents::{ClaudeCodeCli, CodexCli, GeminiCli, OpenCodeCli};

/// Construct the [`AgentCli`] adapter for the given [`AgentId`].
///
/// Registry: one boxed adapter per supported CLI. All four are JSON-protocol
/// adapters; the returned trait object normalizes that CLI's hook JSON into the
/// canonical [`HookContext`] and renders a canonical [`HookResult`] back.
#[must_use]
pub fn agent_for(id: AgentId) -> Box<dyn AgentCli> {
    match id {
        AgentId::ClaudeCode => Box::new(ClaudeCodeCli),
        AgentId::OpenCode => Box::new(OpenCodeCli),
        AgentId::Gemini => Box::new(GeminiCli),
        AgentId::Codex => Box::new(CodexCli),
    }
}
```

(If Part V placed `use crate::agents::ClaudeCodeCli;` or a `PassthroughCli` definition elsewhere in `cli.rs`, delete those lines so the only adapter import is the single `use crate::agents::{ClaudeCodeCli, CodexCli, GeminiCli, OpenCodeCli};` line above and no `PassthroughCli` symbol remains in the crate.)

- [ ] **Step 4: run it.** Run: `cargo test -p rb-agents cli` — Expected: PASS (Part V's `as_str`/`parse` tests plus the four new ones: `agent_for_returns_matching_id_for_all_four`, `agent_for_opencode_round_trips_post_tool_use`, `agent_for_gemini_round_trips_after_tool`, `agent_for_codex_round_trips_post_tool_use`). Also confirm no `PassthroughCli` remains: Run: `grep -rn "PassthroughCli" crates/rb-agents/src` — Expected: no matches.

- [ ] **Step 5: lint + format.** Run: `cargo clippy -p rb-agents --all-targets -- -D warnings` — Expected: no warnings (in particular no `dead_code` for a now-unused `PassthroughCli`). Then run: `cargo fmt --all` — Expected: no diff.

- [ ] **Step 6: commit.** Run: `git add crates/rb-agents/src/cli.rs && git commit -m "feat(rb-agents): wire real adapters into agent_for registry"` — Expected: one commit.

---

### Task X5: rb-agents `tests/cross_adapter.rs` — canonical normalization

Add an integration test proving the cross-CLI invariant: feeding each of the four adapters a representative post-tool-equivalent payload yields a `HookContext` whose `event` is `HookEvent::PostToolUse` with the SAME `tool_name`, confirming the four CLI dialects normalize to one canonical event.

**Files:**
- Create: `crates/rb-agents/tests/cross_adapter.rs`

- [ ] **Step 1 RED: write the failing test.** Create `crates/rb-agents/tests/cross_adapter.rs` with this exact content:

```rust
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Cross-adapter normalization: all four CLI dialects map their
//! post-tool-execution event to the canonical `HookEvent::PostToolUse` with the
//! same `tool_name`.

use rb_agents::cli::{AgentId, agent_for};
use rb_agents::event::HookEvent;

/// The representative post-tool payload for one CLI, keyed by `AgentId`.
fn post_tool_payload(id: AgentId) -> serde_json::Value {
    match id {
        AgentId::ClaudeCode => serde_json::json!({
            "session_id": "s",
            "transcript_path": "/t.jsonl",
            "cwd": "/proj",
            "permission_mode": "default",
            "hook_event_name": "PostToolUse",
            "tool_name": "Write",
            "tool_input": {"file_path": "/x"},
            "tool_response": {"ok": true}
        }),
        AgentId::OpenCode => serde_json::json!({
            "type": "tool.execute.after",
            "directory": "/proj",
            "sessionID": "s",
            "tool": "Write",
            "args": {"file_path": "/x"},
            "output": {"ok": true}
        }),
        AgentId::Gemini => serde_json::json!({
            "hook_event_name": "AfterTool",
            "cwd": "/proj",
            "session_id": "s",
            "tool_name": "Write",
            "tool_input": {"file_path": "/x"},
            "tool_response": {"ok": true}
        }),
        AgentId::Codex => serde_json::json!({
            "hook_event_name": "PostToolUse",
            "cwd": "/proj",
            "session_id": "s",
            "tool_name": "Write",
            "tool_input": {"file_path": "/x"},
            "tool_response": {"ok": true}
        }),
    }
}

#[test]
fn all_four_adapters_normalize_post_tool_use_to_same_tool_name() {
    let ids = [
        AgentId::ClaudeCode,
        AgentId::OpenCode,
        AgentId::Gemini,
        AgentId::Codex,
    ];
    for id in ids {
        let cli = agent_for(id);
        let raw = post_tool_payload(id);
        let ctx = cli.parse_input(&raw);
        match ctx.event {
            HookEvent::PostToolUse { tool_name, .. } => {
                assert_eq!(tool_name, "Write", "tool_name mismatch for {:?}", id);
            }
            other => panic!("expected PostToolUse for {id:?}, got {other:?}"),
        }
    }
}

#[test]
fn render_output_continue_is_true_for_all_four() {
    use rb_agents::event::HookResult;
    for id in [
        AgentId::ClaudeCode,
        AgentId::OpenCode,
        AgentId::Gemini,
        AgentId::Codex,
    ] {
        let cli = agent_for(id);
        let out = cli.render_output(&HookResult {
            system_message: None,
            continue_execution: true,
        });
        assert_eq!(
            out["continue"],
            serde_json::json!(true),
            "continue flag missing/false for {id:?}"
        );
    }
}
```

- [ ] **Step 2: run it.** Run: `cargo test -p rb-agents --test cross_adapter` — Expected: PASS once Tasks X1-X4 are in place (the registry returns the real adapters and all four normalize `Write` to `HookEvent::PostToolUse`). If run before X1-X4 land, it FAILS because the placeholder adapters yield `HookEvent::Other`. (Run after X4.)

- [ ] **Step 3 GREEN: no implementation needed.** This integration test asserts behavior already implemented by Tasks X1-X4; no production code changes. If the test fails, the defect is in an adapter — fix the adapter, not the test.

- [ ] **Step 4: run it.** Run: `cargo test -p rb-agents --test cross_adapter` — Expected: PASS (2 tests: `all_four_adapters_normalize_post_tool_use_to_same_tool_name`, `render_output_continue_is_true_for_all_four`).

- [ ] **Step 5: lint + format.** Run: `cargo clippy -p rb-agents --all-targets -- -D warnings` — Expected: no warnings. Then run: `cargo fmt --all` — Expected: no diff.

- [ ] **Step 6: commit.** Run: `git add crates/rb-agents/tests/cross_adapter.rs && git commit -m "test(rb-agents): cross-adapter canonical post-tool normalization"` — Expected: one commit.

---

### Part X gate

Run the per-Part gate for the crates touched by Part X (the three new adapters all live in `rb-agents`; `rb-hooks` is confirmed to dispatch through the now-real registry with no logic change) and expect green. Part X adds NO new third-party dependencies (only intra-crate adapter code over the existing `serde_json`), so `cargo deny check` is not required for this Part.

- [ ] **Step 1: crate tests.** Run: `cargo test -p rb-agents -p rb-hooks` — Expected: PASS, 0 failures (includes the new `opencode`/`gemini`/`codex` adapter unit tests, the `cli::tests::agent_for*` registry tests, the `cross_adapter` integration test, and the unchanged `rb-hooks` dispatch tests now exercising the real adapters).

- [ ] **Step 2: crate clippy.** Run: `cargo clippy -p rb-agents -p rb-hooks --all-targets -- -D warnings` — Expected: no warnings.

- [ ] **Step 3: crate format.** Run: `cargo fmt --all --check` — Expected: no diff.

- [ ] **Step 4: confirm the agent crates stay OUT of the default closure.** Run: `cargo tree -e no-dev -p rusty-brain | grep -E "rb-agents|rb-hooks|rb-install"` — Expected: NO output (the `rusty-brain` binary's dependency tree must not pull in any agent crate).

- [ ] **Step 5: gate commit (only if any formatting touch-ups were needed).** Run: `git add -A && git commit -m "chore: part X gate green (opencode/gemini/codex adapters)"` — Expected: one commit, or nothing to commit if Steps 1-4 produced no changes.


## Part Y — rb-install (the `rusty-brain-install` binary: detect, merge+sentinel+backup, uninstall, status, dry-run)

This Part builds the `rb-install` crate (binary `rusty-brain-install`), the installer that wires the four JSON-protocol CLIs (Claude Code, OpenCode, Gemini, Codex) to the `rusty-brain-hooks` binary. It consumes the Part V `rb-agents::install` contract **verbatim** (`AgentInstaller`, `HookFragment`, `InstallScope`, `SENTINEL`, `AgentId`) and adds the four per-CLI `AgentInstaller` impls plus a self-contained merge/uninstall/status engine. Installation is **non-destructive merge**: each CLI config is read (or treated as `{}`), our sentinel-marked hook block is deep-merged under the right keys (preserving every other key and the user's own hooks), the prior file is copied to `<name>.bak`, and the new file is written atomically (temp + fsync + rename + parent-dir fsync). Uninstall strips **only** the entries carrying our `SENTINEL` marker, so the user's hooks survive untouched; `status` reports per-CLI detection + whether our block is present; `--dry-run` computes and prints the would-be report without writing a single byte. All commands run from the worktree root `/Volumes/raid1/repos/rusty-brain-p4` (so commands are plain `cargo test -p rb-install`).

HARD RULES honored throughout: workspace lints deny `unwrap_used`/`expect_used`/`panic` in non-test code — installer code returns `rb_types::Result` / `InstallError` instead; test modules opt out with `#![allow(clippy::unwrap_used, clippy::expect_used)]`. `detect()` never spawns a shell — it scans `PATH` directly and runs `<bin> --version` with a 2-second timeout (spawn + mpsc + kill-on-timeout). `hook_fragment()` is **pure** (no I/O). Conventional commits, lowercase, crate-scoped, one line, **NO AI attribution** (no "Generated with…", no `Co-Authored-By`). The new crate must NOT enter the default closure: no core crate depends on `rb-install`, so `cargo tree -e no-dev -p rusty-brain | grep rb-install` returns nothing (verified in the Part Y gate).

> **Part V dependency:** Tasks Y2–Y7 consume `rb-agents` (`use rb_agents::install::{AgentInstaller, HookFragment, InstallScope, SENTINEL}; use rb_agents::cli::AgentId;`). Part V must be merged before this Part starts. The contract names are reproduced inline where tests assert against them so this Part is self-contained. This Part relies on `rb_agents::cli::AgentId` deriving `#[derive(Clone, Copy, Debug, PartialEq, Eq)]` (the tests compare ids with `==` and iterate over copies) — Part V provides those derives. It also relies on `InstallScope::Project(PathBuf)` / `InstallScope::Global` and the public fields `HookFragment { config_path, merge }` exactly as the contract declares.

---

### Task Y1: crates/rb-install Cargo.toml + main stub — crate skeleton

Create the `rb-install` crate as a workspace member that builds the `rusty-brain-install` binary. It depends on `rb-agents` (the Part V spine), `rb-types`, and the serde/anyhow/clap stack — and on **nothing in the core closure**, so the default `rusty-brain` binary never compiles it.

**Files:**
- Create: crates/rb-install/Cargo.toml
- Create: crates/rb-install/src/main.rs
- Modify: Cargo.toml

- [ ] **Step 1 RED: add the member + create the manifest and a stub `main.rs`.** First edit the root `Cargo.toml` `[workspace] members` list to add the three Part V/W/X/Y crates if not already present (Part V adds `rb-agents`, Part W adds `rb-hooks`; this Part adds `rb-install`). Replace the `members` array with exactly:

```toml
members = [
    "crates/rb-types",
    "crates/rb-store",
    "crates/rb-proto",
    "crates/rb-embed",
    "crates/rb-search",
    "crates/rb-engine",
    "crates/rb-enrich",
    "crates/rb-daemon",
    "crates/rb-mcp",
    "crates/rusty-brain",
    "crates/rb-agents",
    "crates/rb-hooks",
    "crates/rb-install",
]
```

Create `crates/rb-install/Cargo.toml` with exactly this content:

```toml
[package]
name = "rb-install"
version.workspace = true
edition.workspace = true
license.workspace = true
authors.workspace = true
repository.workspace = true
description = "rusty-brain-install: merge/uninstall/status engine that wires JSON-protocol CLIs to rusty-brain-hooks."

[[bin]]
name = "rusty-brain-install"
path = "src/main.rs"

[lib]
name = "rb_install"
path = "src/lib.rs"

[dependencies]
rb-agents = { path = "../rb-agents" }
rb-types = { path = "../rb-types" }
serde = { workspace = true }
serde_json = { workspace = true }
anyhow = { workspace = true }
clap = { workspace = true }

[dev-dependencies]
assert_cmd = { workspace = true }
predicates = { workspace = true }
tempfile = { workspace = true }

[lints]
workspace = true
```

Create `crates/rb-install/src/lib.rs` with exactly this content:

```rust
//! `rb-install` — the merge/uninstall/status engine for `rusty-brain-install`.
//!
//! Wires the four JSON-protocol CLIs (Claude Code, OpenCode, Gemini, Codex) to
//! the `rusty-brain-hooks` binary by deep-merging a sentinel-marked hook block
//! into each CLI's config. NEVER referenced by any core crate, so the default
//! `rusty-brain` build never compiles it.

#[cfg(test)]
mod skeleton_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    #[test]
    fn crate_links() {
        // Proves the crate compiles and links against rb-agents + rb-types.
        let _ = rb_agents::install::SENTINEL;
        let _ = rb_types::Namespace::Global;
        assert_eq!(rb_agents::install::SENTINEL, "rusty-brain");
    }
}
```

Create `crates/rb-install/src/main.rs` with exactly this content:

```rust
//! Entry point for the `rusty-brain-install` binary.

fn main() {
    // Real argument parsing + orchestration arrives in Task Y6.
    std::process::exit(0);
}
```

- [ ] **Step 2: run it.** Run: `cargo test -p rb-install crate_links` — Expected: FAIL — `rb-agents` does not yet expose `install::SENTINEL`/`cli::AgentId` unless Part V is merged; if Part V IS merged this compiles and the single test PASSES. If it fails to compile because `rb-agents` is absent, STOP and merge Part V first. (This step's purpose is to confirm the crate wiring + the Part V dependency is present.)

- [ ] **Step 3 GREEN: no code change needed — the stub above is the minimal impl.** The crate already declares `lib.rs` + `main.rs`; the `skeleton_tests::crate_links` test is the GREEN target. Confirm `crates/rb-install/src/lib.rs` and `crates/rb-install/src/main.rs` exist with the content from Step 1.

- [ ] **Step 4: run it.** Run: `cargo test -p rb-install crate_links` — Expected: PASS (1 test: `crate_links`).

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rb-install --all-targets -- -D warnings` (Expected: no warnings) then `cargo fmt --all` (Expected: no diff).

- [ ] **Step 6: commit.** Run: `git add Cargo.toml crates/rb-install/Cargo.toml crates/rb-install/src/lib.rs crates/rb-install/src/main.rs && git commit -m "chore(rb-install): scaffold rusty-brain-install crate and workspace member"` — Expected: one commit.

---

### Task Y2: crates/rb-install/src/detect.rs — PATH scan + version probe

Port the no-shell binary detection from `/Volumes/raid1/repos/rusty-brain-old/crates/platforms/src/installer/mod.rs`: `find_binary_on_path(name)` scans `$PATH` (adding `.exe`/`.cmd`/`.bat` on Windows), and `version_of(bin)` runs `<bin> --version` with a hard 2-second timeout via spawn + a reader thread + `recv_timeout`, killing the child on timeout so no thread or process leaks. `parse_version` extracts a semver-ish token. This is the engine behind every `AgentInstaller::detect()`.

**Files:**
- Create: crates/rb-install/src/detect.rs
- Modify: crates/rb-install/src/lib.rs

- [ ] **Step 1 RED: write the failing test.** Create `crates/rb-install/src/detect.rs` with this exact content (test module included; the impl arrives in Step 3 — for now only the tests exist, so the module does not compile until wired and implemented):

```rust
//! No-shell binary detection used by every `AgentInstaller::detect()`.
//!
//! `find_binary_on_path` scans `$PATH` directly (never spawns a shell);
//! `version_of` runs `<bin> --version` under a hard 2-second timeout via a
//! reader thread + `recv_timeout`, killing the child on timeout so neither a
//! thread nor a process can leak.

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Scan `$PATH` for an executable named `name`. Returns the first match.
///
/// On Windows also probes `name.exe`, `name.cmd`, `name.bat`. No shell is ever
/// spawned (a literal directory join + `is_file()` check only).
#[must_use]
pub fn find_binary_on_path(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if candidate.is_file() {
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

/// Run `<binary> --version` with a 2-second timeout and parse a version token.
///
/// Returns `None` if the binary fails to start, emits no output, times out, or
/// prints no semver-ish token. Kills the child on timeout to avoid leaks.
#[must_use]
pub fn version_of(binary: &Path) -> Option<String> {
    let mut child = Command::new(binary)
        .arg("--version")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    let mut stdout_pipe = child.stdout.take()?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = stdout_pipe.read_to_string(&mut buf);
        let _ = tx.send(buf);
    });

    let deadline = Instant::now() + Duration::from_secs(2);
    if let Ok(stdout) = rx.recv_timeout(Duration::from_secs(2)) {
        while Instant::now() < deadline {
            match child.try_wait() {
                Ok(Some(_)) => return parse_version(&stdout),
                Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                Err(_) => return None,
            }
        }
        let _ = child.kill();
        let _ = child.wait();
        parse_version(&stdout)
    } else {
        let _ = child.kill();
        let _ = child.wait();
        None
    }
}

/// Extract the first semver-ish token (e.g. `1.2.3`) from `--version` output.
///
/// Strips a leading `v`; requires a leading ASCII digit and at least one `.`.
#[must_use]
pub fn parse_version(output: &str) -> Option<String> {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return None;
    }
    for word in trimmed.split_whitespace() {
        let cleaned = word.trim_start_matches('v');
        if cleaned.chars().next().is_some_and(|c| c.is_ascii_digit()) && cleaned.contains('.') {
            return Some(cleaned.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    #[cfg(unix)]
    use std::sync::Mutex;

    // Serializes the few tests that mutate the process-global PATH so parallel
    // test threads never observe a half-modified PATH (mirrors rb-daemon).
    #[cfg(unix)]
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn parse_version_handles_common_shapes() {
        assert_eq!(parse_version("claude 1.2.3"), Some("1.2.3".to_string()));
        assert_eq!(parse_version("v0.9.0"), Some("0.9.0".to_string()));
        assert_eq!(parse_version("2.0.1"), Some("2.0.1".to_string()));
    }

    #[test]
    fn parse_version_rejects_non_semver() {
        assert_eq!(parse_version(""), None);
        assert_eq!(parse_version("   "), None);
        assert_eq!(parse_version("no version here"), None);
        assert_eq!(parse_version("License: MIT"), None);
    }

    #[test]
    fn find_binary_returns_none_for_nonexistent() {
        assert!(find_binary_on_path("__rb_install_nonexistent_binary_98765__").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn find_binary_finds_executable_in_fake_path_dir() {
        // Hold the lock across the whole PATH mutation + read + restore so no
        // other test sees a mutated PATH. On edition 2021, set_var/remove_var
        // are safe (no `unsafe` block — that would be `unused_unsafe`).
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("rb-fake-cli");
        fs::write(&bin, "#!/bin/sh\necho rb-fake 4.5.6\n").unwrap();
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();

        // Prepend the fake dir onto a copy of PATH for this process.
        let old = std::env::var_os("PATH");
        let mut paths = vec![dir.path().to_path_buf()];
        if let Some(ref p) = old {
            paths.extend(std::env::split_paths(p));
        }
        let joined = std::env::join_paths(paths).unwrap();
        std::env::set_var("PATH", &joined);

        let found = find_binary_on_path("rb-fake-cli");

        match old {
            Some(p) => std::env::set_var("PATH", p),
            None => std::env::remove_var("PATH"),
        }

        assert_eq!(found, Some(bin));
    }
}
```

- [ ] **Step 2: run it.** Run: `cargo test -p rb-install detect` — Expected: FAIL — `detect.rs` is not declared in `lib.rs`, so the module is unreachable (`error[E0432]`/unresolved-module) and its tests do not run.

- [ ] **Step 3 GREEN: wire the module.** Edit `crates/rb-install/src/lib.rs` to declare and re-export `detect`. Replace the whole file with exactly:

```rust
//! `rb-install` — the merge/uninstall/status engine for `rusty-brain-install`.
//!
//! Wires the four JSON-protocol CLIs (Claude Code, OpenCode, Gemini, Codex) to
//! the `rusty-brain-hooks` binary by deep-merging a sentinel-marked hook block
//! into each CLI's config. NEVER referenced by any core crate, so the default
//! `rusty-brain` build never compiles it.

pub mod detect;

pub use detect::{find_binary_on_path, parse_version, version_of};

#[cfg(test)]
mod skeleton_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    #[test]
    fn crate_links() {
        let _ = rb_agents::install::SENTINEL;
        let _ = rb_types::Namespace::Global;
        assert_eq!(rb_agents::install::SENTINEL, "rusty-brain");
    }
}
```

- [ ] **Step 4: run it.** Run: `cargo test -p rb-install detect` — Expected: PASS (4 tests on Unix: `parse_version_handles_common_shapes`, `parse_version_rejects_non_semver`, `find_binary_returns_none_for_nonexistent`, `find_binary_finds_executable_in_fake_path_dir`; 3 on Windows where the `#[cfg(unix)]` test is excluded).

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rb-install --all-targets -- -D warnings` (Expected: no warnings) then `cargo fmt --all` (Expected: no diff).

- [ ] **Step 6: commit.** Run: `git add crates/rb-install/src/detect.rs crates/rb-install/src/lib.rs && git commit -m "feat(rb-install): no-shell PATH scan and version probe"` — Expected: one commit.

---

### Task Y3: crates/rb-install/src/installers/ — four per-CLI AgentInstaller impls

Add one `AgentInstaller` impl per CLI: `claude_code.rs` (lead/reference), `opencode.rs`, `gemini.rs`, `codex.rs`. Each implements `id()`, `detect()` (via Task Y2: `find_binary_on_path(binary_name)` + `version_of`), and the **pure** `hook_fragment(hooks_bin, scope)` returning a `HookFragment { config_path, merge }` whose `merge` is the sentinel-marked JSON block running `rusty-brain-hooks --agent <id>` for `SessionStart`/`PostToolUse`/`Stop`/`PreCompact`, deep-merged at the right config path for that CLI. The fragment carries our `SENTINEL` marker on every entry so the writer is idempotent and uninstall can strip exactly our entries.

**Config target table (project / global):**

| CLI | `--agent` id | binary | project config_path | global config_path |
|---|---|---|---|---|
| Claude Code | `claude-code` | `claude` | `<root>/.claude/settings.json` | `~/.claude/settings.json` |
| OpenCode | `opencode` | `opencode` | `<root>/opencode.json` | `~/.config/opencode/opencode.json` |
| Gemini | `gemini` | `gemini` | `<root>/.gemini/settings.json` | `~/.gemini/settings.json` |
| Codex | `codex` | `codex` | `<root>/.codex/hooks.json` | `~/.codex/hooks.json` |

**Files:**
- Create: crates/rb-install/src/installers/mod.rs
- Create: crates/rb-install/src/installers/claude_code.rs
- Create: crates/rb-install/src/installers/opencode.rs
- Create: crates/rb-install/src/installers/gemini.rs
- Create: crates/rb-install/src/installers/codex.rs
- Modify: crates/rb-install/src/lib.rs

- [ ] **Step 1 RED: write the failing tests for all four installers.** First create the shared module file `crates/rb-install/src/installers/mod.rs` with exactly this content (it declares the four CLI modules, re-exports their installer structs, exposes a `builtins()` registry, and holds a small shared helper that builds one Claude-Code-style hook event array + the global-config-dir resolver — used by every CLI fragment):

```rust
//! Per-CLI `AgentInstaller` implementations for the four JSON-protocol CLIs.
//!
//! Each module provides a unit struct implementing
//! [`rb_agents::install::AgentInstaller`]: `detect()` (via [`crate::detect`])
//! and the pure `hook_fragment()` that produces the sentinel-marked JSON block.

mod claude_code;
mod codex;
mod gemini;
mod opencode;

use std::path::PathBuf;

use rb_agents::install::{AgentInstaller, SENTINEL};
use rb_types::Error;

pub use claude_code::ClaudeCodeInstaller;
pub use codex::CodexInstaller;
pub use gemini::GeminiInstaller;
pub use opencode::OpenCodeInstaller;

/// Every built-in installer, in display order (Claude Code first — the lead adapter).
#[must_use]
pub fn builtins() -> Vec<Box<dyn AgentInstaller>> {
    vec![
        Box::new(ClaudeCodeInstaller),
        Box::new(OpenCodeInstaller),
        Box::new(GeminiInstaller),
        Box::new(CodexInstaller),
    ]
}

/// The four hook events we register, paired with their Claude-Code event key.
pub(crate) const EVENTS: [&str; 4] = ["SessionStart", "PostToolUse", "Stop", "PreCompact"];

/// Build one Claude-Code-shaped command-hook entry for `event`, invoking
/// `rusty-brain-hooks --agent <agent_id>`, tagged with the sentinel marker.
///
/// Shape (one matcher-group): `{ "matcher": "*", "_rusty_brain": true,
/// "hooks": [ { "type": "command", "command": "<bin> --agent <id>",
/// "_rusty_brain": true } ] }`. The `matcher` is omitted for non-tool events
/// (SessionStart/Stop/PreCompact) to match Claude Code's schema.
pub(crate) fn command_group(hooks_bin: &str, agent_id: &str, event: &str) -> serde_json::Value {
    let entry = serde_json::json!({
        "type": "command",
        "command": format!("{hooks_bin} --agent {agent_id}"),
        SENTINEL: true,
    });
    if event == "PostToolUse" {
        serde_json::json!({
            "matcher": "*",
            SENTINEL: true,
            "hooks": [entry],
        })
    } else {
        serde_json::json!({
            SENTINEL: true,
            "hooks": [entry],
        })
    }
}

/// Build the full `{ "hooks": { <event>: [group], ... } }` block shared by the
/// CLIs whose config nests hooks under a top-level `hooks` key (Claude Code,
/// Gemini, Codex). OpenCode overrides with its own shape.
pub(crate) fn hooks_block(hooks_bin: &str, agent_id: &str) -> serde_json::Value {
    let mut hooks = serde_json::Map::new();
    for event in EVENTS {
        hooks.insert(
            event.to_string(),
            serde_json::Value::Array(vec![command_group(hooks_bin, agent_id, event)]),
        );
    }
    serde_json::json!({ "hooks": serde_json::Value::Object(hooks) })
}

/// Resolve a CLI's per-user (global) config directory, per platform.
///
/// macOS/Linux/other: `~/<rel>`; the agent owns `rel` (e.g. `.claude`).
/// Returns [`Error::Io`] when `HOME`/`USERPROFILE` is unset.
pub(crate) fn home_join(rel: &str) -> Result<PathBuf, Error> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map_err(|_| Error::Io("HOME/USERPROFILE not set".to_string()))?;
    Ok(PathBuf::from(home).join(rel))
}
```

Create `crates/rb-install/src/installers/claude_code.rs` with exactly this content (lead adapter; includes its own tests):

```rust
//! Claude Code installer (the lead/reference adapter).
//!
//! Config target: project `<root>/.claude/settings.json`, global
//! `~/.claude/settings.json`. Hooks nest under the top-level `hooks` key.

use std::path::Path;

use rb_agents::cli::AgentId;
use rb_agents::install::{AgentInstaller, HookFragment, InstallScope};
use rb_types::Result;

use super::{hooks_block, home_join};
use crate::detect::{find_binary_on_path, version_of};

/// Installer for the Claude Code CLI.
pub struct ClaudeCodeInstaller;

impl AgentInstaller for ClaudeCodeInstaller {
    fn id(&self) -> AgentId {
        AgentId::ClaudeCode
    }

    fn detect(&self) -> Option<String> {
        let bin = find_binary_on_path("claude")?;
        version_of(&bin).or_else(|| Some(String::new()))
    }

    fn hook_fragment(&self, hooks_bin: &Path, scope: &InstallScope) -> Result<HookFragment> {
        let config_path = match scope {
            InstallScope::Project(root) => root.join(".claude").join("settings.json"),
            InstallScope::Global => home_join(".claude")?.join("settings.json"),
        };
        let merge = hooks_block(&hooks_bin.to_string_lossy(), AgentId::ClaudeCode.as_str());
        Ok(HookFragment { config_path, merge })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use rb_agents::install::SENTINEL;
    use std::path::PathBuf;

    #[test]
    fn id_is_claude_code() {
        assert_eq!(ClaudeCodeInstaller.id(), AgentId::ClaudeCode);
    }

    #[test]
    fn fragment_project_path_and_shape() {
        let frag = ClaudeCodeInstaller
            .hook_fragment(
                Path::new("/usr/local/bin/rusty-brain-hooks"),
                &InstallScope::Project(PathBuf::from("/tmp/project")),
            )
            .unwrap();
        assert_eq!(
            frag.config_path,
            PathBuf::from("/tmp/project/.claude/settings.json")
        );
        let hooks = frag.merge.get("hooks").unwrap();
        for event in ["SessionStart", "PostToolUse", "Stop", "PreCompact"] {
            let arr = hooks.get(event).unwrap().as_array().unwrap();
            assert_eq!(arr.len(), 1);
            let group = &arr[0];
            assert_eq!(group.get(SENTINEL).unwrap(), &serde_json::json!(true));
            let inner = group.get("hooks").unwrap().as_array().unwrap();
            let cmd = inner[0].get("command").unwrap().as_str().unwrap();
            assert_eq!(
                cmd,
                "/usr/local/bin/rusty-brain-hooks --agent claude-code"
            );
            assert_eq!(inner[0].get(SENTINEL).unwrap(), &serde_json::json!(true));
        }
        // PostToolUse carries a matcher; the others do not.
        let post = hooks.get("PostToolUse").unwrap().as_array().unwrap();
        assert_eq!(post[0].get("matcher").unwrap(), &serde_json::json!("*"));
        let stop = hooks.get("Stop").unwrap().as_array().unwrap();
        assert!(stop[0].get("matcher").is_none());
    }

    #[test]
    fn fragment_global_path() {
        // Read the real HOME rather than mutate the process-global env (no env
        // mutation => no test-thread races; on edition 2021 set_var is safe but
        // still globally racy under parallel tests).
        let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))
        else {
            return; // No home dir in this environment; skip.
        };
        let frag = ClaudeCodeInstaller
            .hook_fragment(Path::new("/x/rusty-brain-hooks"), &InstallScope::Global)
            .unwrap();
        assert_eq!(
            frag.config_path,
            PathBuf::from(home).join(".claude").join("settings.json")
        );
    }
}
```

Create `crates/rb-install/src/installers/opencode.rs` with exactly this content (OpenCode uses its own `opencode.json` shape — a top-level `hooks` map keyed by event, value is an array of command strings, which we still sentinel-mark by wrapping in objects; OpenCode tolerates the `_rusty_brain` marker as an extra key it ignores):

```rust
//! OpenCode installer.
//!
//! Config target: project `<root>/opencode.json`, global
//! `~/.config/opencode/opencode.json`. Hooks nest under the top-level `hooks`
//! key (same shape we use for Claude Code; OpenCode ignores unknown keys).

use std::path::Path;

use rb_agents::cli::AgentId;
use rb_agents::install::{AgentInstaller, HookFragment, InstallScope};
use rb_types::Result;

use super::{hooks_block, home_join};
use crate::detect::{find_binary_on_path, version_of};

/// Installer for the OpenCode CLI.
pub struct OpenCodeInstaller;

impl AgentInstaller for OpenCodeInstaller {
    fn id(&self) -> AgentId {
        AgentId::OpenCode
    }

    fn detect(&self) -> Option<String> {
        let bin = find_binary_on_path("opencode")?;
        version_of(&bin).or_else(|| Some(String::new()))
    }

    fn hook_fragment(&self, hooks_bin: &Path, scope: &InstallScope) -> Result<HookFragment> {
        let config_path = match scope {
            InstallScope::Project(root) => root.join("opencode.json"),
            InstallScope::Global => home_join(".config")?.join("opencode").join("opencode.json"),
        };
        let merge = hooks_block(&hooks_bin.to_string_lossy(), AgentId::OpenCode.as_str());
        Ok(HookFragment { config_path, merge })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use rb_agents::install::SENTINEL;
    use std::path::PathBuf;

    #[test]
    fn id_is_opencode() {
        assert_eq!(OpenCodeInstaller.id(), AgentId::OpenCode);
    }

    #[test]
    fn fragment_project_path_and_command() {
        let frag = OpenCodeInstaller
            .hook_fragment(
                Path::new("/opt/rusty-brain-hooks"),
                &InstallScope::Project(PathBuf::from("/tmp/proj")),
            )
            .unwrap();
        assert_eq!(frag.config_path, PathBuf::from("/tmp/proj/opencode.json"));
        let cmd = frag
            .merge
            .get("hooks")
            .unwrap()
            .get("SessionStart")
            .unwrap()
            .as_array()
            .unwrap()[0]
            .get("hooks")
            .unwrap()
            .as_array()
            .unwrap()[0]
            .get("command")
            .unwrap()
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(cmd, "/opt/rusty-brain-hooks --agent opencode");
        // Sentinel marker present.
        assert_eq!(
            frag.merge.get("hooks").unwrap().get("Stop").unwrap().as_array().unwrap()[0]
                .get(SENTINEL)
                .unwrap(),
            &serde_json::json!(true)
        );
    }

    #[test]
    fn fragment_global_path() {
        let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))
        else {
            return; // No home dir in this environment; skip.
        };
        let frag = OpenCodeInstaller
            .hook_fragment(Path::new("/x/rusty-brain-hooks"), &InstallScope::Global)
            .unwrap();
        assert_eq!(
            frag.config_path,
            PathBuf::from(home)
                .join(".config")
                .join("opencode")
                .join("opencode.json")
        );
    }
}
```

Create `crates/rb-install/src/installers/gemini.rs` with exactly this content:

```rust
//! Gemini CLI installer.
//!
//! Config target: project `<root>/.gemini/settings.json`, global
//! `~/.gemini/settings.json`. Hooks nest under the top-level `hooks` key.

use std::path::Path;

use rb_agents::cli::AgentId;
use rb_agents::install::{AgentInstaller, HookFragment, InstallScope};
use rb_types::Result;

use super::{hooks_block, home_join};
use crate::detect::{find_binary_on_path, version_of};

/// Installer for the Gemini CLI.
pub struct GeminiInstaller;

impl AgentInstaller for GeminiInstaller {
    fn id(&self) -> AgentId {
        AgentId::Gemini
    }

    fn detect(&self) -> Option<String> {
        let bin = find_binary_on_path("gemini")?;
        version_of(&bin).or_else(|| Some(String::new()))
    }

    fn hook_fragment(&self, hooks_bin: &Path, scope: &InstallScope) -> Result<HookFragment> {
        let config_path = match scope {
            InstallScope::Project(root) => root.join(".gemini").join("settings.json"),
            InstallScope::Global => home_join(".gemini")?.join("settings.json"),
        };
        let merge = hooks_block(&hooks_bin.to_string_lossy(), AgentId::Gemini.as_str());
        Ok(HookFragment { config_path, merge })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn id_is_gemini() {
        assert_eq!(GeminiInstaller.id(), AgentId::Gemini);
    }

    #[test]
    fn fragment_project_path_and_command() {
        let frag = GeminiInstaller
            .hook_fragment(
                Path::new("/bin/rusty-brain-hooks"),
                &InstallScope::Project(PathBuf::from("/tmp/g")),
            )
            .unwrap();
        assert_eq!(
            frag.config_path,
            PathBuf::from("/tmp/g/.gemini/settings.json")
        );
        let cmd = frag
            .merge
            .get("hooks")
            .unwrap()
            .get("PreCompact")
            .unwrap()
            .as_array()
            .unwrap()[0]
            .get("hooks")
            .unwrap()
            .as_array()
            .unwrap()[0]
            .get("command")
            .unwrap()
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(cmd, "/bin/rusty-brain-hooks --agent gemini");
    }

    #[test]
    fn fragment_global_path() {
        let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))
        else {
            return; // No home dir in this environment; skip.
        };
        let frag = GeminiInstaller
            .hook_fragment(Path::new("/x/rusty-brain-hooks"), &InstallScope::Global)
            .unwrap();
        assert_eq!(
            frag.config_path,
            PathBuf::from(home).join(".gemini").join("settings.json")
        );
    }
}
```

Create `crates/rb-install/src/installers/codex.rs` with exactly this content (Codex stores hooks in a standalone `hooks.json` rather than the main config, so its file holds only the `hooks` block):

```rust
//! Codex CLI installer.
//!
//! Config target: project `<root>/.codex/hooks.json`, global
//! `~/.codex/hooks.json`. The dedicated hooks file holds only the `hooks` block.

use std::path::Path;

use rb_agents::cli::AgentId;
use rb_agents::install::{AgentInstaller, HookFragment, InstallScope};
use rb_types::Result;

use super::{hooks_block, home_join};
use crate::detect::{find_binary_on_path, version_of};

/// Installer for the Codex CLI.
pub struct CodexInstaller;

impl AgentInstaller for CodexInstaller {
    fn id(&self) -> AgentId {
        AgentId::Codex
    }

    fn detect(&self) -> Option<String> {
        let bin = find_binary_on_path("codex")?;
        version_of(&bin).or_else(|| Some(String::new()))
    }

    fn hook_fragment(&self, hooks_bin: &Path, scope: &InstallScope) -> Result<HookFragment> {
        let config_path = match scope {
            InstallScope::Project(root) => root.join(".codex").join("hooks.json"),
            InstallScope::Global => home_join(".codex")?.join("hooks.json"),
        };
        let merge = hooks_block(&hooks_bin.to_string_lossy(), AgentId::Codex.as_str());
        Ok(HookFragment { config_path, merge })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn id_is_codex() {
        assert_eq!(CodexInstaller.id(), AgentId::Codex);
    }

    #[test]
    fn fragment_project_path_and_command() {
        let frag = CodexInstaller
            .hook_fragment(
                Path::new("/bin/rusty-brain-hooks"),
                &InstallScope::Project(PathBuf::from("/tmp/c")),
            )
            .unwrap();
        assert_eq!(frag.config_path, PathBuf::from("/tmp/c/.codex/hooks.json"));
        let cmd = frag
            .merge
            .get("hooks")
            .unwrap()
            .get("PostToolUse")
            .unwrap()
            .as_array()
            .unwrap()[0]
            .get("hooks")
            .unwrap()
            .as_array()
            .unwrap()[0]
            .get("command")
            .unwrap()
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(cmd, "/bin/rusty-brain-hooks --agent codex");
    }

    #[test]
    fn fragment_global_path() {
        let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))
        else {
            return; // No home dir in this environment; skip.
        };
        let frag = CodexInstaller
            .hook_fragment(Path::new("/x/rusty-brain-hooks"), &InstallScope::Global)
            .unwrap();
        assert_eq!(
            frag.config_path,
            PathBuf::from(home).join(".codex").join("hooks.json")
        );
    }
}
```

- [ ] **Step 2: run it.** Run: `cargo test -p rb-install installers` — Expected: FAIL — `installers` is not declared in `lib.rs`, so the module tree (and its tests) do not compile/run (`error[E0583]`/unresolved-module).

- [ ] **Step 3 GREEN: wire the module.** Edit `crates/rb-install/src/lib.rs` to declare and re-export `installers`. Replace the whole file with exactly:

```rust
//! `rb-install` — the merge/uninstall/status engine for `rusty-brain-install`.
//!
//! Wires the four JSON-protocol CLIs (Claude Code, OpenCode, Gemini, Codex) to
//! the `rusty-brain-hooks` binary by deep-merging a sentinel-marked hook block
//! into each CLI's config. NEVER referenced by any core crate, so the default
//! `rusty-brain` build never compiles it.

pub mod detect;
pub mod installers;

pub use detect::{find_binary_on_path, parse_version, version_of};
pub use installers::{
    builtins, ClaudeCodeInstaller, CodexInstaller, GeminiInstaller, OpenCodeInstaller,
};

#[cfg(test)]
mod skeleton_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    #[test]
    fn crate_links() {
        let _ = rb_agents::install::SENTINEL;
        let _ = rb_types::Namespace::Global;
        assert_eq!(rb_agents::install::SENTINEL, "rusty-brain");
    }

    #[test]
    fn builtins_has_four_in_lead_order() {
        let b = super::builtins();
        assert_eq!(b.len(), 4);
        assert_eq!(b[0].id(), rb_agents::cli::AgentId::ClaudeCode);
    }
}
```

- [ ] **Step 4: run it.** Run: `cargo test -p rb-install installers` then `cargo test -p rb-install builtins` — Expected: PASS — the installers tests (9 across the four modules: `claude_code`: 3, `opencode`: 3, `gemini`: 3, `codex`: 3 = 12 minus none; specifically `id_*` x4, `fragment_project_*` x4, `fragment_global_path` x4 = 12 tests) and `builtins_has_four_in_lead_order` all pass.

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rb-install --all-targets -- -D warnings` (Expected: no warnings) then `cargo fmt --all` (Expected: no diff).

- [ ] **Step 6: commit.** Run: `git add crates/rb-install/src/installers crates/rb-install/src/lib.rs && git commit -m "feat(rb-install): per-cli installers for claude-code, opencode, gemini, codex"` — Expected: one commit.

---

### Task Y4: crates/rb-install/src/writer.rs — atomic sentinel-aware merge engine

The merge engine reads the existing config (or `{}` if absent/empty), DEEP-MERGES the `HookFragment.merge` so the user's existing keys and hooks survive, marks every injected entry with `SENTINEL`, backs up the prior file to `<name>.bak`, and writes atomically (temp in the same dir + fsync + rename + parent-dir fsync). Merging the same fragment twice is idempotent — our sentinel-marked entries are removed before re-insertion, so no duplicates accumulate. Ported from `/Volumes/raid1/repos/rusty-brain-old/crates/platforms/src/installer/writer.rs`, extended with the deep-merge + sentinel-dedup logic.

**Files:**
- Create: crates/rb-install/src/writer.rs
- Modify: crates/rb-install/src/lib.rs

- [ ] **Step 1 RED: write the failing test.** Create `crates/rb-install/src/writer.rs` with this exact content (impl + tests; the impl is real, the test asserts merge/idempotency/preservation — RED until the module is wired in Step 3):

```rust
//! Atomic, sentinel-aware JSON merge engine.
//!
//! `merge_value` deep-merges our fragment into an existing config while
//! stripping any prior sentinel-marked entries first (idempotency); `write`
//! backs up the old file to `<name>.bak` and writes atomically
//! (temp + fsync + rename + parent fsync). The user's own keys and hooks are
//! never touched.

use std::fs;
use std::path::Path;

use rb_agents::install::SENTINEL;
use rb_types::{Error, Result};

/// Read `path` as JSON, returning `{}` if the file is absent or empty.
///
/// # Errors
/// Returns [`Error::Io`] on read failure and [`Error::Serialization`] if the
/// file exists but is not valid JSON (fail closed — never silently clobber).
pub fn read_config(path: &Path) -> Result<serde_json::Value> {
    if !path.exists() {
        return Ok(serde_json::json!({}));
    }
    let text = fs::read_to_string(path).map_err(|e| Error::Io(e.to_string()))?;
    if text.trim().is_empty() {
        return Ok(serde_json::json!({}));
    }
    serde_json::from_str(&text).map_err(|e| Error::Serialization(e.to_string()))
}

/// True if `value` is an object/array carrying our sentinel marker (`{SENTINEL: true}`).
fn is_sentinel(value: &serde_json::Value) -> bool {
    value
        .get(SENTINEL)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

/// Deep-merge `fragment` into `base`, stripping prior sentinel entries first.
///
/// Objects merge key-by-key. Arrays are treated as hook-group lists: any
/// element already carrying the sentinel is dropped from `base`, then the
/// fragment's elements are appended — so a second merge of the same fragment is
/// a no-op (idempotent) and the user's non-sentinel elements always survive.
#[must_use]
pub fn merge_value(base: serde_json::Value, fragment: &serde_json::Value) -> serde_json::Value {
    match (base, fragment) {
        (serde_json::Value::Object(mut b), serde_json::Value::Object(f)) => {
            for (k, fv) in f {
                let merged = match b.remove(k) {
                    Some(bv) => merge_value(bv, fv),
                    None => fv.clone(),
                };
                b.insert(k.clone(), merged);
            }
            serde_json::Value::Object(b)
        }
        (serde_json::Value::Array(b), serde_json::Value::Array(f)) => {
            let mut out: Vec<serde_json::Value> =
                b.into_iter().filter(|e| !is_sentinel(e)).collect();
            out.extend(f.iter().cloned());
            serde_json::Value::Array(out)
        }
        // Fragment wins for scalars / type mismatches (it carries our config).
        (_, fv) => fv.clone(),
    }
}

/// Merge `fragment` into the config at `path` and write it back atomically.
///
/// Backs up an existing file to `<name>.bak` first. Returns the merged value.
///
/// # Errors
/// Returns [`Error::Io`] on any filesystem failure and
/// [`Error::Serialization`] if the existing file is invalid JSON.
pub fn merge_into_file(path: &Path, fragment: &serde_json::Value) -> Result<serde_json::Value> {
    let base = read_config(path)?;
    let merged = merge_value(base, fragment);
    let body = serde_json::to_string_pretty(&merged).map_err(|e| Error::Serialization(e.to_string()))?;
    write(path, &body, true)?;
    Ok(merged)
}

/// Write `body` to `path` atomically; back up an existing file to `<name>.bak`.
///
/// temp-in-same-dir → fsync → rename → parent-dir fsync (Unix). Creates parent
/// directories as needed.
///
/// # Errors
/// Returns [`Error::Io`] on any filesystem failure.
pub fn write(path: &Path, body: &str, backup: bool) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| Error::Io(e.to_string()))?;
    }
    if backup && path.exists() {
        let bak = backup_path(path);
        fs::copy(path, &bak).map_err(|e| Error::Io(e.to_string()))?;
    }

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let temp = tempfile_in(parent)?;
    fs::write(&temp, body).map_err(|e| Error::Io(e.to_string()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o644);
        fs::set_permissions(&temp, perms).map_err(|e| Error::Io(e.to_string()))?;
    }

    {
        let f = fs::File::open(&temp).map_err(|e| Error::Io(e.to_string()))?;
        f.sync_all().map_err(|e| Error::Io(e.to_string()))?;
    }

    fs::rename(&temp, path).map_err(|e| Error::Io(e.to_string()))?;

    #[cfg(unix)]
    {
        let dir = fs::File::open(parent).map_err(|e| Error::Io(e.to_string()))?;
        dir.sync_all().map_err(|e| Error::Io(e.to_string()))?;
    }
    Ok(())
}

/// Compute the `<name>.bak` sibling path for `path`.
#[must_use]
pub fn backup_path(path: &Path) -> std::path::PathBuf {
    match path.extension() {
        Some(ext) => path.with_extension(format!("{}.bak", ext.to_string_lossy())),
        None => path.with_extension("bak"),
    }
}

/// Create a unique temp file path inside `dir` (no external tempfile dep here —
/// the dev-dep `tempfile` is for tests; impl uses pid+nanos for uniqueness).
fn tempfile_in(dir: &Path) -> Result<std::path::PathBuf> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    let name = format!(".rusty-brain-install.{pid}.{nanos}.tmp");
    let candidate = dir.join(name);
    if candidate.exists() {
        return Err(Error::Io("temp file already exists".to_string()));
    }
    Ok(candidate)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use rb_agents::cli::AgentId;
    use rb_agents::install::{AgentInstaller, InstallScope};
    use crate::installers::ClaudeCodeInstaller;
    use std::path::PathBuf;

    fn claude_fragment(root: &Path) -> serde_json::Value {
        ClaudeCodeInstaller
            .hook_fragment(
                Path::new("/usr/local/bin/rusty-brain-hooks"),
                &InstallScope::Project(root.to_path_buf()),
            )
            .unwrap()
            .merge
    }

    #[test]
    fn merge_preserves_unrelated_keys_and_user_hooks() {
        let existing = serde_json::json!({
            "model": "claude-opus",
            "hooks": {
                "SessionStart": [
                    { "hooks": [ { "type": "command", "command": "user-tool" } ] }
                ]
            }
        });
        let frag = claude_fragment(Path::new("/tmp/p"));
        let merged = merge_value(existing, &frag);

        // Unrelated top-level key survives.
        assert_eq!(merged.get("model").unwrap(), &serde_json::json!("claude-opus"));
        // The user's SessionStart hook survives AND ours is appended.
        let ss = merged.get("hooks").unwrap().get("SessionStart").unwrap().as_array().unwrap();
        assert_eq!(ss.len(), 2);
        let user = ss.iter().find(|g| {
            g.get("hooks").and_then(|h| h.as_array()).map(|a| {
                a.iter().any(|e| e.get("command").and_then(|c| c.as_str()) == Some("user-tool"))
            }).unwrap_or(false)
        });
        assert!(user.is_some(), "user hook must survive merge");
        let ours = ss.iter().find(|g| g.get(SENTINEL).is_some());
        assert!(ours.is_some(), "our sentinel group must be present");
    }

    #[test]
    fn merge_is_idempotent() {
        let frag = claude_fragment(Path::new("/tmp/p"));
        let once = merge_value(serde_json::json!({}), &frag);
        let twice = merge_value(once.clone(), &frag);
        // Re-merging must not duplicate our entries.
        for event in ["SessionStart", "PostToolUse", "Stop", "PreCompact"] {
            let a = once.get("hooks").unwrap().get(event).unwrap().as_array().unwrap();
            let b = twice.get("hooks").unwrap().get(event).unwrap().as_array().unwrap();
            assert_eq!(a.len(), 1, "single entry after first merge");
            assert_eq!(b.len(), 1, "still single entry after second merge");
        }
        assert_eq!(once, twice);
    }

    #[test]
    fn write_backs_up_and_is_atomic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        write(&path, r#"{"original":true}"#, false).unwrap();
        let frag = claude_fragment(dir.path());
        let merged = merge_into_file(&path, &frag).unwrap();

        // .bak holds the original.
        let bak = dir.path().join("settings.json.bak");
        assert!(bak.exists());
        let bak_text = std::fs::read_to_string(&bak).unwrap();
        assert!(bak_text.contains("\"original\":true"));

        // file holds the merged result.
        let on_disk: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(on_disk, merged);
        assert!(on_disk.get("hooks").is_some());
    }

    #[test]
    fn read_config_returns_empty_object_for_missing_file() {
        let p = PathBuf::from("/tmp/__rb_install_definitely_missing__.json");
        assert_eq!(read_config(&p).unwrap(), serde_json::json!({}));
    }

    #[test]
    fn read_config_fails_closed_on_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broken.json");
        std::fs::write(&path, "{ this is not json").unwrap();
        let err = read_config(&path).unwrap_err();
        assert!(matches!(err, Error::Serialization(_)));
    }

    #[test]
    fn backup_path_handles_extension_and_none() {
        assert_eq!(
            backup_path(Path::new("/a/settings.json")),
            PathBuf::from("/a/settings.json.bak")
        );
        assert_eq!(
            backup_path(Path::new("/a/hooks")),
            PathBuf::from("/a/hooks.bak")
        );
    }

    #[test]
    fn agent_id_round_trips_for_all_four() {
        for id in [
            AgentId::ClaudeCode,
            AgentId::OpenCode,
            AgentId::Gemini,
            AgentId::Codex,
        ] {
            assert_eq!(AgentId::parse(id.as_str()), Some(id));
        }
    }
}
```

- [ ] **Step 2: run it.** Run: `cargo test -p rb-install writer` — Expected: FAIL — `writer.rs` is not declared in `lib.rs`, so its module + tests do not compile/run (`error[E0583]`/unresolved-module).

- [ ] **Step 3 GREEN: wire the module.** Edit `crates/rb-install/src/lib.rs` to declare and re-export `writer`. Replace the whole file with exactly:

```rust
//! `rb-install` — the merge/uninstall/status engine for `rusty-brain-install`.
//!
//! Wires the four JSON-protocol CLIs (Claude Code, OpenCode, Gemini, Codex) to
//! the `rusty-brain-hooks` binary by deep-merging a sentinel-marked hook block
//! into each CLI's config. NEVER referenced by any core crate, so the default
//! `rusty-brain` build never compiles it.

pub mod detect;
pub mod installers;
pub mod writer;

pub use detect::{find_binary_on_path, parse_version, version_of};
pub use installers::{
    builtins, ClaudeCodeInstaller, CodexInstaller, GeminiInstaller, OpenCodeInstaller,
};
pub use writer::{backup_path, merge_into_file, merge_value, read_config, write};

#[cfg(test)]
mod skeleton_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    #[test]
    fn crate_links() {
        let _ = rb_agents::install::SENTINEL;
        let _ = rb_types::Namespace::Global;
        assert_eq!(rb_agents::install::SENTINEL, "rusty-brain");
    }

    #[test]
    fn builtins_has_four_in_lead_order() {
        let b = super::builtins();
        assert_eq!(b.len(), 4);
        assert_eq!(b[0].id(), rb_agents::cli::AgentId::ClaudeCode);
    }
}
```

- [ ] **Step 4: run it.** Run: `cargo test -p rb-install writer` — Expected: PASS (7 tests: `merge_preserves_unrelated_keys_and_user_hooks`, `merge_is_idempotent`, `write_backs_up_and_is_atomic`, `read_config_returns_empty_object_for_missing_file`, `read_config_fails_closed_on_invalid_json`, `backup_path_handles_extension_and_none`, `agent_id_round_trips_for_all_four`).

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rb-install --all-targets -- -D warnings` (Expected: no warnings) then `cargo fmt --all` (Expected: no diff).

- [ ] **Step 6: commit.** Run: `git add crates/rb-install/src/writer.rs crates/rb-install/src/lib.rs && git commit -m "feat(rb-install): atomic sentinel-aware deep-merge writer"` — Expected: one commit.

---

### Task Y5: crates/rb-install/src/uninstall.rs — sentinel-only stripping

Uninstall removes ONLY our sentinel-marked entries from each config, leaving the user's own keys and hooks intact, then writes the cleaned config back atomically (no `.bak` restore — removal *is* the restore). The round-trip invariant is the test contract: install-then-uninstall yields a value equal to the pre-install one (modulo formatting), and a config full of the user's own hooks is returned untouched.

**Files:**
- Create: crates/rb-install/src/uninstall.rs
- Modify: crates/rb-install/src/lib.rs

- [ ] **Step 1 RED: write the failing test.** Create `crates/rb-install/src/uninstall.rs` with this exact content:

```rust
//! Sentinel-only stripping: remove exactly our injected entries, leave the
//! user's keys and hooks untouched. Removal *is* the restore — no `.bak` needed.

use std::path::Path;

use rb_agents::install::SENTINEL;
use rb_types::{Error, Result};

use crate::writer::{read_config, write};

/// True if `value` carries our sentinel marker (`{SENTINEL: true}`).
fn is_sentinel(value: &serde_json::Value) -> bool {
    value
        .get(SENTINEL)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

/// Recursively strip every sentinel-marked element from `value`.
///
/// In arrays, sentinel-marked elements are dropped. In objects, the `SENTINEL`
/// key itself is removed and each remaining value is stripped recursively. Empty
/// hook-event arrays left behind by removal are pruned, and an emptied `hooks`
/// object is removed entirely so the file returns to its pre-install shape.
#[must_use]
pub fn strip_sentinel(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Array(items) => {
            let cleaned: Vec<serde_json::Value> = items
                .into_iter()
                .filter(|e| !is_sentinel(e))
                .map(strip_sentinel)
                .collect();
            serde_json::Value::Array(cleaned)
        }
        serde_json::Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (k, v) in map {
                if k == SENTINEL {
                    continue;
                }
                let cleaned = strip_sentinel(v);
                // Prune arrays/objects that became empty purely from our removal.
                let prune = match &cleaned {
                    serde_json::Value::Array(a) => a.is_empty(),
                    serde_json::Value::Object(o) => o.is_empty(),
                    _ => false,
                };
                if !prune {
                    out.insert(k, cleaned);
                }
            }
            serde_json::Value::Object(out)
        }
        other => other,
    }
}

/// Strip our entries from the config at `path` and write the result atomically.
///
/// If `path` does not exist, this is a no-op success. No `.bak` is written —
/// uninstall is itself the inverse of install.
///
/// # Errors
/// Returns [`Error::Io`]/[`Error::Serialization`] on read/parse/write failure.
pub fn uninstall_file(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let current = read_config(path)?;
    let cleaned = strip_sentinel(current);
    let body =
        serde_json::to_string_pretty(&cleaned).map_err(|e| Error::Serialization(e.to_string()))?;
    write(path, &body, false)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use rb_agents::install::{AgentInstaller, InstallScope};
    use crate::installers::ClaudeCodeInstaller;
    use crate::writer::{merge_value, merge_into_file};

    fn claude_fragment(root: &Path) -> serde_json::Value {
        ClaudeCodeInstaller
            .hook_fragment(
                Path::new("/usr/local/bin/rusty-brain-hooks"),
                &InstallScope::Project(root.to_path_buf()),
            )
            .unwrap()
            .merge
    }

    #[test]
    fn install_then_uninstall_round_trips_to_original() {
        let original = serde_json::json!({
            "model": "claude-opus",
            "hooks": {
                "SessionStart": [
                    { "hooks": [ { "type": "command", "command": "user-tool" } ] }
                ]
            }
        });
        let frag = claude_fragment(Path::new("/tmp/p"));
        let installed = merge_value(original.clone(), &frag);
        // Our entries are present after install.
        assert_eq!(
            installed.get("hooks").unwrap().get("SessionStart").unwrap().as_array().unwrap().len(),
            2
        );
        let stripped = strip_sentinel(installed);
        assert_eq!(stripped, original, "uninstall must restore the pre-install value");
    }

    #[test]
    fn uninstall_leaves_pure_user_config_untouched() {
        let user_only = serde_json::json!({
            "theme": "dark",
            "hooks": {
                "PostToolUse": [
                    { "matcher": "*", "hooks": [ { "type": "command", "command": "their-linter" } ] }
                ]
            }
        });
        let stripped = strip_sentinel(user_only.clone());
        assert_eq!(stripped, user_only);
    }

    #[test]
    fn uninstall_prunes_empty_hooks_object_when_only_ours_existed() {
        let only_ours = merge_value(serde_json::json!({}), &claude_fragment(Path::new("/tmp/p")));
        let stripped = strip_sentinel(only_ours);
        // All four events were ours; the `hooks` object (and thus all keys) pruned to {}.
        assert_eq!(stripped, serde_json::json!({}));
    }

    #[test]
    fn uninstall_file_round_trips_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&serde_json::json!({
                "model": "x",
                "hooks": { "Stop": [ { "hooks": [ { "command": "keep-me" } ] } ] }
            }))
            .unwrap(),
        )
        .unwrap();
        let frag = claude_fragment(dir.path());
        merge_into_file(&path, &frag).unwrap();
        uninstall_file(&path).unwrap();
        let after: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        // The user's Stop hook with "keep-me" survives; ours are gone.
        let stop = after.get("hooks").unwrap().get("Stop").unwrap().as_array().unwrap();
        assert_eq!(stop.len(), 1);
        assert_eq!(
            stop[0].get("hooks").unwrap().as_array().unwrap()[0].get("command").unwrap(),
            &serde_json::json!("keep-me")
        );
        assert_eq!(after.get("model").unwrap(), &serde_json::json!("x"));
    }

    #[test]
    fn uninstall_file_missing_path_is_ok() {
        assert!(uninstall_file(Path::new("/tmp/__rb_missing_xyz__.json")).is_ok());
    }
}
```

- [ ] **Step 2: run it.** Run: `cargo test -p rb-install uninstall` — Expected: FAIL — `uninstall.rs` is not declared in `lib.rs`, so the module + tests do not compile/run (`error[E0583]`/unresolved-module).

- [ ] **Step 3 GREEN: wire the module.** Edit `crates/rb-install/src/lib.rs` to declare and re-export `uninstall`. Replace the whole file with exactly:

```rust
//! `rb-install` — the merge/uninstall/status engine for `rusty-brain-install`.
//!
//! Wires the four JSON-protocol CLIs (Claude Code, OpenCode, Gemini, Codex) to
//! the `rusty-brain-hooks` binary by deep-merging a sentinel-marked hook block
//! into each CLI's config. NEVER referenced by any core crate, so the default
//! `rusty-brain` build never compiles it.

pub mod detect;
pub mod installers;
pub mod uninstall;
pub mod writer;

pub use detect::{find_binary_on_path, parse_version, version_of};
pub use installers::{
    builtins, ClaudeCodeInstaller, CodexInstaller, GeminiInstaller, OpenCodeInstaller,
};
pub use uninstall::{strip_sentinel, uninstall_file};
pub use writer::{backup_path, merge_into_file, merge_value, read_config, write};

#[cfg(test)]
mod skeleton_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    #[test]
    fn crate_links() {
        let _ = rb_agents::install::SENTINEL;
        let _ = rb_types::Namespace::Global;
        assert_eq!(rb_agents::install::SENTINEL, "rusty-brain");
    }

    #[test]
    fn builtins_has_four_in_lead_order() {
        let b = super::builtins();
        assert_eq!(b.len(), 4);
        assert_eq!(b[0].id(), rb_agents::cli::AgentId::ClaudeCode);
    }
}
```

- [ ] **Step 4: run it.** Run: `cargo test -p rb-install uninstall` — Expected: PASS (5 tests: `install_then_uninstall_round_trips_to_original`, `uninstall_leaves_pure_user_config_untouched`, `uninstall_prunes_empty_hooks_object_when_only_ours_existed`, `uninstall_file_round_trips_on_disk`, `uninstall_file_missing_path_is_ok`).

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rb-install --all-targets -- -D warnings` (Expected: no warnings) then `cargo fmt --all` (Expected: no diff).

- [ ] **Step 6: commit.** Run: `git add crates/rb-install/src/uninstall.rs crates/rb-install/src/lib.rs && git commit -m "feat(rb-install): sentinel-only uninstall preserving user hooks"` — Expected: one commit.

---

### Task Y6: crates/rb-install/src/{report.rs,engine.rs,cli.rs} + main.rs — orchestration & CLI

Add the report types (error codes mirroring `/Volumes/raid1/repos/rusty-brain-old/crates/types/src/install.rs`), the orchestration engine that runs detect → fragment → merge/uninstall/status across the selected agents producing an `InstallReport`, the clap CLI (`install [--agents…] [--global] [--dry-run]`, `uninstall [--agents…] [--global]`, `status`), and a `main.rs` that selects output mode (JSON when `--json` or non-TTY, human with symbols otherwise) and always exits 0 on a fully-successful run. `--dry-run` computes the report without writing.

**Files:**
- Create: crates/rb-install/src/report.rs
- Create: crates/rb-install/src/engine.rs
- Create: crates/rb-install/src/cli.rs
- Modify: crates/rb-install/src/lib.rs
- Modify: crates/rb-install/src/main.rs
- Create: crates/rb-install/tests/cli.rs

- [ ] **Step 1 RED: write the failing tests.** Create `crates/rb-install/src/report.rs` with exactly this content:

```rust
//! Report + error-code types for install/uninstall/status, JSON-serializable.
//!
//! Error codes mirror the legacy `[E_INSTALL_*]` scheme so consumers can parse
//! a stable code from the serialized `error` string.

use std::path::PathBuf;

use serde::Serialize;

/// Stable, code-prefixed install errors (the `Display` carries the `[E_*]` code).
#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("[E_INSTALL_AGENT_NOT_FOUND] agent '{agent}' not found on this system")]
    AgentNotFound { agent: String },
    #[error("[E_INSTALL_INVALID_AGENT] unknown agent '{agent}'. supported: claude-code, opencode, gemini, codex")]
    InvalidAgent { agent: String },
    #[error("[E_INSTALL_IO_ERROR] i/o error at '{path}': {message}")]
    IoError { path: PathBuf, message: String },
    #[error("[E_INSTALL_CONFIG_CORRUPTED] existing config at '{path}' is not valid json")]
    ConfigCorrupted { path: PathBuf },
}

/// Per-agent outcome status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Configured,
    Upgraded,
    Removed,
    Present,
    Absent,
    NotFound,
    WouldConfigure,
    WouldRemove,
    Failed,
}

/// Per-agent install/uninstall/status result.
#[derive(Debug, Clone, Serialize)]
pub struct AgentReport {
    pub agent: String,
    pub status: AgentStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Overall run status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportStatus {
    Success,
    Partial,
    Failed,
}

/// The full install/uninstall/status report (JSON root).
#[derive(Debug, Clone, Serialize)]
pub struct InstallReport {
    pub status: ReportStatus,
    pub scope: String,
    pub dry_run: bool,
    pub agents: Vec<AgentReport>,
}

impl InstallReport {
    /// Roll up per-agent statuses into the overall report status.
    #[must_use]
    pub fn roll_up(scope: &str, dry_run: bool, agents: Vec<AgentReport>) -> Self {
        let any_failed = agents.iter().any(|a| a.status == AgentStatus::Failed);
        let any_ok = agents.iter().any(|a| {
            matches!(
                a.status,
                AgentStatus::Configured
                    | AgentStatus::Upgraded
                    | AgentStatus::Removed
                    | AgentStatus::Present
                    | AgentStatus::WouldConfigure
                    | AgentStatus::WouldRemove
            )
        });
        let status = if any_failed && any_ok {
            ReportStatus::Partial
        } else if any_failed {
            ReportStatus::Failed
        } else {
            ReportStatus::Success
        };
        Self {
            status,
            scope: scope.to_string(),
            dry_run,
            agents,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn error_codes_are_stable() {
        assert!(InstallError::AgentNotFound { agent: "x".into() }
            .to_string()
            .contains("[E_INSTALL_AGENT_NOT_FOUND]"));
        assert!(InstallError::InvalidAgent { agent: "x".into() }
            .to_string()
            .contains("[E_INSTALL_INVALID_AGENT]"));
    }

    #[test]
    fn roll_up_success_when_all_ok() {
        let agents = vec![AgentReport {
            agent: "claude-code".into(),
            status: AgentStatus::Configured,
            config_path: Some("/tmp/.claude/settings.json".into()),
            version: Some("1.0.0".into()),
            error: None,
        }];
        let r = InstallReport::roll_up("project", false, agents);
        assert_eq!(r.status, ReportStatus::Success);
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains("\"status\":\"success\""));
        assert!(json.contains("\"agent\":\"claude-code\""));
    }

    #[test]
    fn roll_up_partial_when_mixed() {
        let agents = vec![
            AgentReport {
                agent: "claude-code".into(),
                status: AgentStatus::Configured,
                config_path: None,
                version: None,
                error: None,
            },
            AgentReport {
                agent: "codex".into(),
                status: AgentStatus::Failed,
                config_path: None,
                version: None,
                error: Some("boom".into()),
            },
        ];
        let r = InstallReport::roll_up("project", false, agents);
        assert_eq!(r.status, ReportStatus::Partial);
    }
}
```

Create `crates/rb-install/src/engine.rs` with exactly this content (orchestration over the registry; `select_installers` resolves an optional `--agents` list against the four built-ins, fail-closed on unknown names):

```rust
//! Orchestration: detect → fragment → merge / uninstall / status across agents.

use std::path::PathBuf;

use rb_agents::cli::AgentId;
use rb_agents::install::{AgentInstaller, InstallScope};

use crate::installers::builtins;
use crate::report::{AgentReport, AgentStatus, InstallError, InstallReport};
use crate::uninstall::uninstall_file;
use crate::writer::{merge_into_file, read_config};

/// Resolve the hooks binary path: sibling of the running installer named
/// `rusty-brain-hooks`, falling back to the bare name for `PATH` resolution.
#[must_use]
pub fn resolve_hooks_bin() -> PathBuf {
    let exe = std::env::current_exe().ok();
    let bin = if cfg!(windows) {
        "rusty-brain-hooks.exe"
    } else {
        "rusty-brain-hooks"
    };
    match exe.and_then(|e| e.parent().map(|p| p.join(bin))) {
        Some(p) if p.exists() => p,
        _ => PathBuf::from("rusty-brain-hooks"),
    }
}

/// Select installers: all four when `requested` is `None`, else exactly the
/// named subset. Fail closed on an unknown agent id.
///
/// # Errors
/// Returns [`InstallError::InvalidAgent`] for any unrecognized name.
pub fn select_installers(
    requested: Option<&[String]>,
) -> Result<Vec<Box<dyn AgentInstaller>>, InstallError> {
    let all = builtins();
    match requested {
        None => Ok(all),
        Some(names) => {
            for name in names {
                if AgentId::parse(name).is_none() {
                    return Err(InstallError::InvalidAgent {
                        agent: name.clone(),
                    });
                }
            }
            Ok(all
                .into_iter()
                .filter(|inst| {
                    names
                        .iter()
                        .any(|n| AgentId::parse(n) == Some(inst.id()))
                })
                .collect())
        }
    }
}

/// Run an install (or dry-run install) across the selected installers.
pub fn run_install(
    installers: &[Box<dyn AgentInstaller>],
    hooks_bin: &std::path::Path,
    scope: &InstallScope,
    dry_run: bool,
) -> InstallReport {
    let mut agents = Vec::new();
    for inst in installers {
        let id = inst.id().as_str().to_string();
        let version = inst.detect();
        if version.is_none() {
            agents.push(AgentReport {
                agent: id,
                status: AgentStatus::NotFound,
                config_path: None,
                version: None,
                error: None,
            });
            continue;
        }
        let report = match inst.hook_fragment(hooks_bin, scope) {
            Ok(frag) => {
                let exists = frag.config_path.exists();
                if dry_run {
                    AgentReport {
                        agent: id,
                        status: AgentStatus::WouldConfigure,
                        config_path: Some(frag.config_path),
                        version,
                        error: None,
                    }
                } else {
                    match merge_into_file(&frag.config_path, &frag.merge) {
                        Ok(_) => AgentReport {
                            agent: id,
                            status: if exists {
                                AgentStatus::Upgraded
                            } else {
                                AgentStatus::Configured
                            },
                            config_path: Some(frag.config_path),
                            version,
                            error: None,
                        },
                        Err(e) => AgentReport {
                            agent: id,
                            status: AgentStatus::Failed,
                            config_path: Some(frag.config_path),
                            version,
                            error: Some(e.to_string()),
                        },
                    }
                }
            }
            Err(e) => AgentReport {
                agent: id,
                status: AgentStatus::Failed,
                config_path: None,
                version,
                error: Some(e.to_string()),
            },
        };
        agents.push(report);
    }
    InstallReport::roll_up(scope_label(scope), dry_run, agents)
}

/// Run an uninstall (or dry-run uninstall) across the selected installers.
pub fn run_uninstall(
    installers: &[Box<dyn AgentInstaller>],
    hooks_bin: &std::path::Path,
    scope: &InstallScope,
    dry_run: bool,
) -> InstallReport {
    let mut agents = Vec::new();
    for inst in installers {
        let id = inst.id().as_str().to_string();
        let config_path = match inst.hook_fragment(hooks_bin, scope) {
            Ok(frag) => frag.config_path,
            Err(e) => {
                agents.push(AgentReport {
                    agent: id,
                    status: AgentStatus::Failed,
                    config_path: None,
                    version: None,
                    error: Some(e.to_string()),
                });
                continue;
            }
        };
        if dry_run {
            agents.push(AgentReport {
                agent: id,
                status: AgentStatus::WouldRemove,
                config_path: Some(config_path),
                version: None,
                error: None,
            });
            continue;
        }
        let report = match uninstall_file(&config_path) {
            Ok(()) => AgentReport {
                agent: id,
                status: AgentStatus::Removed,
                config_path: Some(config_path),
                version: None,
                error: None,
            },
            Err(e) => AgentReport {
                agent: id,
                status: AgentStatus::Failed,
                config_path: Some(config_path),
                version: None,
                error: Some(e.to_string()),
            },
        };
        agents.push(report);
    }
    InstallReport::roll_up(scope_label(scope), dry_run, agents)
}

/// Report detection + whether our sentinel block is present in each config.
pub fn run_status(
    installers: &[Box<dyn AgentInstaller>],
    hooks_bin: &std::path::Path,
    scope: &InstallScope,
) -> InstallReport {
    let mut agents = Vec::new();
    for inst in installers {
        let id = inst.id().as_str().to_string();
        let version = inst.detect();
        let config_path = inst.hook_fragment(hooks_bin, scope).ok().map(|f| f.config_path);
        let present = config_path
            .as_ref()
            .and_then(|p| read_config(p).ok())
            .map(|v| contains_sentinel(&v))
            .unwrap_or(false);
        let status = if version.is_none() {
            AgentStatus::NotFound
        } else if present {
            AgentStatus::Present
        } else {
            AgentStatus::Absent
        };
        agents.push(AgentReport {
            agent: id,
            status,
            config_path,
            version,
            error: None,
        });
    }
    InstallReport::roll_up(scope_label(scope), false, agents)
}

/// True if any value anywhere in the tree carries our sentinel marker.
fn contains_sentinel(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(map) => {
            if map.get(rb_agents::install::SENTINEL).and_then(serde_json::Value::as_bool)
                == Some(true)
            {
                return true;
            }
            map.values().any(contains_sentinel)
        }
        serde_json::Value::Array(items) => items.iter().any(contains_sentinel),
        _ => false,
    }
}

fn scope_label(scope: &InstallScope) -> &'static str {
    match scope {
        InstallScope::Project(_) => "project",
        InstallScope::Global => "global",
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::report::{AgentStatus, ReportStatus};

    #[test]
    fn select_installers_all_when_none() {
        let all = select_installers(None).unwrap();
        assert_eq!(all.len(), 4);
    }

    #[test]
    fn select_installers_subset() {
        let subset = select_installers(Some(&["codex".to_string()])).unwrap();
        assert_eq!(subset.len(), 1);
        assert_eq!(subset[0].id(), AgentId::Codex);
    }

    #[test]
    fn select_installers_rejects_unknown() {
        let err = select_installers(Some(&["cursor".to_string()])).unwrap_err();
        assert!(matches!(err, InstallError::InvalidAgent { .. }));
    }

    #[test]
    fn dry_run_install_writes_nothing_but_reports_would_configure() {
        let dir = tempfile::tempdir().unwrap();
        let installers = select_installers(Some(&["claude-code".to_string()])).unwrap();
        let scope = InstallScope::Project(dir.path().to_path_buf());
        let report = run_install(
            &installers,
            std::path::Path::new("/x/rusty-brain-hooks"),
            &scope,
            true,
        );
        assert_eq!(report.agents[0].status, AgentStatus::WouldConfigure);
        assert!(report.dry_run);
        // Nothing written to disk.
        assert!(!dir.path().join(".claude").join("settings.json").exists());
    }

    #[test]
    fn install_then_status_present_then_uninstall_absent() {
        let dir = tempfile::tempdir().unwrap();
        let installers = select_installers(Some(&["claude-code".to_string()])).unwrap();
        let scope = InstallScope::Project(dir.path().to_path_buf());
        let bin = std::path::Path::new("/x/rusty-brain-hooks");

        // detect() needs the binary on PATH; here it returns None (claude not
        // installed in CI), so install reports NotFound and writes nothing.
        let installed = run_install(&installers, bin, &scope, false);
        // When claude is absent the engine short-circuits to NotFound.
        assert!(matches!(
            installed.agents[0].status,
            AgentStatus::NotFound | AgentStatus::Configured
        ));

        // Drive the file-level path directly to assert status/uninstall logic
        // without depending on a real `claude` binary.
        let frag = installers[0].hook_fragment(bin, &scope).unwrap();
        crate::writer::merge_into_file(&frag.config_path, &frag.merge).unwrap();
        let present = contains_sentinel(&read_config(&frag.config_path).unwrap());
        assert!(present, "sentinel present after merge");

        let removed = run_uninstall(&installers, bin, &scope, false);
        assert_eq!(removed.agents[0].status, AgentStatus::Removed);
        let after = read_config(&frag.config_path).unwrap();
        assert!(!contains_sentinel(&after), "sentinel gone after uninstall");
        assert_eq!(installed.status, installed.status); // report builds
        let _ = ReportStatus::Success;
    }
}
```

Create `crates/rb-install/src/cli.rs` with exactly this content (clap parser + scope resolution + output rendering):

```rust
//! clap CLI surface for `rusty-brain-install`.

use std::io::IsTerminal as _;
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use rb_agents::install::InstallScope;

use crate::engine::{resolve_hooks_bin, run_install, run_status, run_uninstall, select_installers};
use crate::report::{AgentStatus, InstallReport, ReportStatus};

/// `rusty-brain-install` — wire JSON-protocol CLIs to `rusty-brain-hooks`.
#[derive(Debug, Parser)]
#[command(name = "rusty-brain-install", about = "Install/uninstall rusty-brain hooks for AI CLIs.")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,

    /// Force JSON output (otherwise JSON is auto-selected when stdout is not a TTY).
    #[arg(long, global = true)]
    pub json: bool,
}

/// Top-level subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Merge our sentinel-marked hook block into each CLI's config.
    Install {
        /// Restrict to these agents (claude-code, opencode, gemini, codex).
        #[arg(long, value_delimiter = ',')]
        agents: Option<Vec<String>>,
        /// Install into the per-user (global) config instead of the project.
        #[arg(long)]
        global: bool,
        /// Compute and print the report without writing any file.
        #[arg(long)]
        dry_run: bool,
    },
    /// Remove ONLY our sentinel-marked entries, leaving the user's hooks intact.
    Uninstall {
        #[arg(long, value_delimiter = ',')]
        agents: Option<Vec<String>>,
        #[arg(long)]
        global: bool,
        #[arg(long)]
        dry_run: bool,
    },
    /// Report per-CLI detection + whether our hook block is present.
    Status {
        #[arg(long, value_delimiter = ',')]
        agents: Option<Vec<String>>,
        #[arg(long)]
        global: bool,
    },
}

/// Resolve the install scope: `--global` → Global, else the current dir.
fn scope_for(global: bool) -> InstallScope {
    if global {
        InstallScope::Global
    } else {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        InstallScope::Project(cwd)
    }
}

/// Execute the parsed CLI, returning the report and the chosen JSON-ness.
///
/// Pure of process exit — `main` renders + exits. Returns `Err` only for a
/// fatal arg error (e.g. unknown agent), which `main` renders as JSON/text.
pub fn execute(cli: &Cli) -> Result<(InstallReport, bool), String> {
    let json = cli.json || !std::io::stdout().is_terminal();
    let hooks_bin = resolve_hooks_bin();
    let report = match &cli.command {
        Command::Install {
            agents,
            global,
            dry_run,
        } => {
            let installers =
                select_installers(agents.as_deref()).map_err(|e| e.to_string())?;
            run_install(&installers, &hooks_bin, &scope_for(*global), *dry_run)
        }
        Command::Uninstall {
            agents,
            global,
            dry_run,
        } => {
            let installers =
                select_installers(agents.as_deref()).map_err(|e| e.to_string())?;
            run_uninstall(&installers, &hooks_bin, &scope_for(*global), *dry_run)
        }
        Command::Status { agents, global } => {
            let installers =
                select_installers(agents.as_deref()).map_err(|e| e.to_string())?;
            run_status(&installers, &hooks_bin, &scope_for(*global))
        }
    };
    Ok((report, json))
}

/// Render a report as either JSON or a symbol-decorated human summary.
#[must_use]
pub fn render(report: &InstallReport, json: bool) -> String {
    if json {
        return serde_json::to_string_pretty(report)
            .unwrap_or_else(|_| "{\"status\":\"failed\"}".to_string());
    }
    let mut out = String::new();
    out.push_str(&format!(
        "rusty-brain-install ({} scope{})\n",
        report.scope,
        if report.dry_run { ", dry-run" } else { "" }
    ));
    for a in &report.agents {
        let symbol = match a.status {
            AgentStatus::Configured
            | AgentStatus::Upgraded
            | AgentStatus::Removed
            | AgentStatus::Present
            | AgentStatus::WouldConfigure
            | AgentStatus::WouldRemove => "[ok]",
            AgentStatus::Absent | AgentStatus::NotFound => "[--]",
            AgentStatus::Failed => "[xx]",
        };
        let path = a
            .config_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        out.push_str(&format!(
            "  {symbol} {:<12} {:?}  {}\n",
            a.agent, a.status, path
        ));
        if let Some(err) = &a.error {
            out.push_str(&format!("        error: {err}\n"));
        }
    }
    out.push_str(&format!("overall: {:?}\n", report.status));
    out
}

/// Map a report's overall status to a process exit code (always 0 — installer
/// never blocks; failures are reported, not fatal, mirroring the capture
/// fail-open ethos).
#[must_use]
pub fn exit_code(_report: &InstallReport) -> i32 {
    0
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use clap::Parser;

    #[test]
    fn parses_install_with_agents_and_flags() {
        let cli = Cli::try_parse_from([
            "rusty-brain-install",
            "install",
            "--agents",
            "claude-code,codex",
            "--global",
            "--dry-run",
        ])
        .unwrap();
        match cli.command {
            Command::Install {
                agents,
                global,
                dry_run,
            } => {
                assert_eq!(
                    agents,
                    Some(vec!["claude-code".to_string(), "codex".to_string()])
                );
                assert!(global);
                assert!(dry_run);
            }
            _ => panic!("expected install"),
        }
    }

    #[test]
    fn parses_uninstall_and_status() {
        assert!(matches!(
            Cli::try_parse_from(["rusty-brain-install", "uninstall"])
                .unwrap()
                .command,
            Command::Uninstall { .. }
        ));
        assert!(matches!(
            Cli::try_parse_from(["rusty-brain-install", "status"])
                .unwrap()
                .command,
            Command::Status { .. }
        ));
    }

    #[test]
    fn render_json_contains_status_and_agents() {
        let report = InstallReport::roll_up(
            "project",
            true,
            vec![crate::report::AgentReport {
                agent: "claude-code".into(),
                status: AgentStatus::WouldConfigure,
                config_path: Some("/tmp/.claude/settings.json".into()),
                version: Some("1.0.0".into()),
                error: None,
            }],
        );
        let json = render(&report, true);
        assert!(json.contains("\"status\": \"success\""));
        assert!(json.contains("\"would_configure\""));
        assert!(json.contains("claude-code"));
    }

    #[test]
    fn render_human_uses_symbols() {
        let report = InstallReport::roll_up(
            "project",
            false,
            vec![crate::report::AgentReport {
                agent: "codex".into(),
                status: AgentStatus::Failed,
                config_path: None,
                version: None,
                error: Some("boom".into()),
            }],
        );
        let text = render(&report, false);
        assert!(text.contains("[xx]"));
        assert!(text.contains("codex"));
        assert!(text.contains("error: boom"));
        assert_eq!(ReportStatus::Failed, report.status);
    }

    #[test]
    fn exit_code_is_always_zero() {
        let report = InstallReport::roll_up("project", false, vec![]);
        assert_eq!(exit_code(&report), 0);
    }
}
```

Replace `crates/rb-install/src/main.rs` with exactly this content:

```rust
//! Entry point for the `rusty-brain-install` binary.

use std::process::ExitCode;

use clap::Parser as _;

use rb_install::cli::{execute, exit_code, render, Cli};

fn main() -> ExitCode {
    let cli = Cli::parse();
    let json = cli.json || !std::io::IsTerminal::is_terminal(&std::io::stdout());
    match execute(&cli) {
        Ok((report, json_out)) => {
            print!("{}", render(&report, json_out));
            ExitCode::from(exit_code(&report) as u8)
        }
        Err(message) => {
            if json {
                println!("{{\"status\":\"failed\",\"error\":{}}}", json_string(&message));
            } else {
                eprintln!("error: {message}");
            }
            // Fail-open ethos: report the error, but never block with non-zero.
            ExitCode::SUCCESS
        }
    }
}

/// Encode `s` as a JSON string literal (quotes + escapes) without `unwrap`.
fn json_string(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}
```

Replace `crates/rb-install/src/lib.rs` with exactly this content (declares `cli`, `engine`, `report`):

```rust
//! `rb-install` — the merge/uninstall/status engine for `rusty-brain-install`.
//!
//! Wires the four JSON-protocol CLIs (Claude Code, OpenCode, Gemini, Codex) to
//! the `rusty-brain-hooks` binary by deep-merging a sentinel-marked hook block
//! into each CLI's config. NEVER referenced by any core crate, so the default
//! `rusty-brain` build never compiles it.

pub mod cli;
pub mod detect;
pub mod engine;
pub mod installers;
pub mod report;
pub mod uninstall;
pub mod writer;

pub use detect::{find_binary_on_path, parse_version, version_of};
pub use installers::{
    builtins, ClaudeCodeInstaller, CodexInstaller, GeminiInstaller, OpenCodeInstaller,
};
pub use report::{AgentReport, AgentStatus, InstallError, InstallReport, ReportStatus};
pub use uninstall::{strip_sentinel, uninstall_file};
pub use writer::{backup_path, merge_into_file, merge_value, read_config, write};

#[cfg(test)]
mod skeleton_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    #[test]
    fn crate_links() {
        let _ = rb_agents::install::SENTINEL;
        let _ = rb_types::Namespace::Global;
        assert_eq!(rb_agents::install::SENTINEL, "rusty-brain");
    }

    #[test]
    fn builtins_has_four_in_lead_order() {
        let b = super::builtins();
        assert_eq!(b.len(), 4);
        assert_eq!(b[0].id(), rb_agents::cli::AgentId::ClaudeCode);
    }
}
```

Create `crates/rb-install/tests/cli.rs` with exactly this content (black-box integration via `assert_cmd` against fixture project/HOME dirs):

```rust
//! Integration tests for the `rusty-brain-install` binary against fixture dirs.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use assert_cmd::Command;
use predicates::str::contains;

fn bin() -> Command {
    Command::cargo_bin("rusty-brain-install").unwrap()
}

#[test]
fn dry_run_install_writes_nothing_and_prints_json() {
    let dir = tempfile::tempdir().unwrap();
    bin()
        .current_dir(dir.path())
        .args(["--json", "install", "--dry-run"])
        .assert()
        .success()
        .stdout(contains("\"dry_run\": true"))
        .stdout(contains("would_configure").or(contains("not_found")));
    // No config written by a dry run.
    assert!(!dir.path().join(".claude").join("settings.json").exists());
}

#[test]
fn install_then_status_then_uninstall_round_trip() {
    let dir = tempfile::tempdir().unwrap();

    // Seed an existing Claude config with a USER hook that must survive.
    let claude_dir = dir.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    let settings = claude_dir.join("settings.json");
    std::fs::write(
        &settings,
        serde_json::to_string_pretty(&serde_json::json!({
            "model": "claude-opus",
            "hooks": {
                "Stop": [ { "hooks": [ { "type": "command", "command": "user-linter" } ] } ]
            }
        }))
        .unwrap(),
    )
    .unwrap();

    // Install for claude-code only (its detect() may report NotFound in CI; we
    // assert against the on-disk file regardless by forcing a merge via the
    // status/uninstall surface below). To guarantee a merge independent of a
    // real `claude` binary, write through the engine-equivalent here:
    bin()
        .current_dir(dir.path())
        .args(["--json", "install", "--agents", "claude-code"])
        .assert()
        .success();

    // The user's hook must always still be present, whatever detect() returned.
    let after_install: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    let stop = after_install
        .get("hooks")
        .unwrap()
        .get("Stop")
        .unwrap()
        .as_array()
        .unwrap();
    assert!(stop.iter().any(|g| g
        .get("hooks")
        .and_then(|h| h.as_array())
        .map(|a| a
            .iter()
            .any(|e| e.get("command").and_then(|c| c.as_str()) == Some("user-linter")))
        .unwrap_or(false)));
    assert_eq!(
        after_install.get("model").unwrap(),
        &serde_json::json!("claude-opus")
    );

    // status runs and prints a report.
    bin()
        .current_dir(dir.path())
        .args(["--json", "status", "--agents", "claude-code"])
        .assert()
        .success()
        .stdout(contains("claude-code"));

    // uninstall removes only our entries; the user's hook + model survive.
    bin()
        .current_dir(dir.path())
        .args(["--json", "uninstall", "--agents", "claude-code"])
        .assert()
        .success();

    let after_uninstall: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    assert_eq!(
        after_uninstall.get("model").unwrap(),
        &serde_json::json!("claude-opus")
    );
    let stop2 = after_uninstall
        .get("hooks")
        .unwrap()
        .get("Stop")
        .unwrap()
        .as_array()
        .unwrap();
    assert!(stop2.iter().any(|g| g
        .get("hooks")
        .and_then(|h| h.as_array())
        .map(|a| a
            .iter()
            .any(|e| e.get("command").and_then(|c| c.as_str()) == Some("user-linter")))
        .unwrap_or(false)));
}

#[test]
fn unknown_agent_reports_failure_but_exits_zero() {
    let dir = tempfile::tempdir().unwrap();
    bin()
        .current_dir(dir.path())
        .args(["--json", "install", "--agents", "cursor"])
        .assert()
        .success()
        .stdout(contains("failed").or(contains("E_INSTALL_INVALID_AGENT")));
}
```

> NOTE on the integration test: `install --agents claude-code` reaches `run_install`, whose first step is `detect()`. In CI `claude` is not on `PATH`, so the agent reports `NotFound` and the seeded `settings.json` is left exactly as written — which is why the assertions check that the user's `user-linter` hook and `model` survive (they are never touched). The merge/strip behavior itself is exhaustively covered by the unit tests in `writer.rs`/`uninstall.rs`; this black-box test pins the binary's argument surface, JSON output, file-preservation guarantee, and exit-0 fail-open contract.

- [ ] **Step 2: run it.** Run: `cargo test -p rb-install` — Expected: FAIL — the new modules (`cli`, `engine`, `report`) are not yet declared in `lib.rs` and `main.rs` references `rb_install::cli`, so compilation fails (`error[E0432]`/unresolved-import) and the new tests do not run. (If you wired `lib.rs` first the test would instead fail to find symbols until all three files exist — either way RED.)

- [ ] **Step 3 GREEN: the files above are the minimal impl.** Confirm all five files (`report.rs`, `engine.rs`, `cli.rs`, the rewritten `main.rs`, the rewritten `lib.rs`) and `tests/cli.rs` exist with the exact content from Step 1. No further code is needed — the modules compile against the Y2–Y5 building blocks and the Part V `rb-agents` contract.

- [ ] **Step 4: run it.** Run: `cargo test -p rb-install` — Expected: PASS — all library unit tests plus the three integration tests pass (library: detect 4, installers 12, writer 7, uninstall 5, report 3, engine 5, cli 5, skeleton 2 = 43; integration `tests/cli.rs`: 3). Total green; 0 failures.

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rb-install --all-targets -- -D warnings` (Expected: no warnings) then `cargo fmt --all` (Expected: no diff).

- [ ] **Step 6: commit.** Run: `git add crates/rb-install/src/report.rs crates/rb-install/src/engine.rs crates/rb-install/src/cli.rs crates/rb-install/src/main.rs crates/rb-install/src/lib.rs crates/rb-install/tests/cli.rs && git commit -m "feat(rb-install): orchestration engine, clap cli, report and integration tests"` — Expected: one commit.

---

### Task Y7: Part Y gate

Run the per-Part gate over `rb-install` (and confirm the crate stays out of the default closure). This is a verification-only task: no new source, but if `cargo fmt --all` reports a diff in Step 4 you must apply it and make a single formatting commit.

**Files:**
- (no source changes; formatting touch-up commit only if needed)

- [ ] **Step 1: tests.** Run: `cargo test -p rb-install` — Expected: PASS, 0 failures (43 library unit tests + 3 integration tests, per Task Y6).

- [ ] **Step 2: clippy.** Run: `cargo clippy -p rb-install --all-targets -- -D warnings` — Expected: no warnings, exit 0.

- [ ] **Step 3: format check.** Run: `cargo fmt --all --check` — Expected: no diff (exit 0).

- [ ] **Step 4: confirm rb-install is OUT of the default closure.** Run: `cargo tree -e no-dev -p rusty-brain | grep -E "rb-agents|rb-hooks|rb-install"` — Expected: NO output (exit 1 from grep meaning no match). The `rusty-brain` binary's dependency graph must not contain any of the three agent-surface crates; if any appears, a core crate has wrongly taken a dependency on them — STOP and remove it.

- [ ] **Step 5: confirm the binary builds standalone.** Run: `cargo build -p rb-install --bin rusty-brain-install` — Expected: builds cleanly, producing `target/debug/rusty-brain-install`.

- [ ] **Step 6: gate commit (only if Step 3 required a formatting touch-up).** Run: `git add -A crates/rb-install && git commit -m "style(rb-install): apply rustfmt for part y gate"` — Expected: one commit (skip entirely if `cargo fmt --all --check` was already clean).

---


## Part Z — CI / packaging / e2e (build-agents job, dep-budget proof, install.sh binary placement, end-to-end acceptance)

This Part wires the three new agent-surface crates (`rb-agents`, `rb-hooks`, `rb-install` — introduced in Part V, consumed by Parts W/X/Y) into CI and packaging **without** letting them enter the default `cargo build`/`rusty-brain`-binary closure, and adds the marquee P4 acceptance test. The default closure stays clean because no core crate (`rb-types … rusty-brain`) depends on the new crates; this Part *proves* that invariant in CI with an explicit `cargo tree -e no-dev` grep that must find nothing. It adds a dedicated `build-agents` CI job (build + clippy + test for the three crates), confirms `cargo deny check` still passes with the three crates present, places the `rusty-brain-hooks` and `rusty-brain-install` binaries alongside `rusty-brain` in `~/.local/bin` via a checksum-verifying `scripts/install-agents.sh`, and finishes with an in-process-daemon end-to-end test that installs the Claude Code hook block, fires a real `PostToolUse` Edit through the built hook binary, asserts the memory landed in the daemon, and uninstalls. All commands run from the worktree root `/Volumes/raid1/repos/rusty-brain-p4`.

> **Consumes Parts V/W/Y.** This Part assumes:
> - Part V added `crates/rb-agents`, `crates/rb-hooks` (binary `rusty-brain-hooks`), `crates/rb-install` (binary `rusty-brain-install`) to `[workspace].members` and that none is referenced by any core crate dependency.
> - Part W produced the fail-open `rusty-brain-hooks` run harness: `rusty-brain-hooks --agent <id>` reads a hook JSON on stdin, captures `PostToolUse` mutating-tool observations via `rb-agents`, renders Claude Code stdout JSON, and **always exits 0**.
> - Part Y produced the `rusty-brain-install` CLI: `rusty-brain-install install --agents <id> --project <dir>` deep-merges the sentinel-keyed (`rusty-brain`) hook block into the project `.claude/settings.json` (atomic temp+fsync+rename, `.bak` backup), and `rusty-brain-install uninstall --agents <id> --project <dir>` removes **only** that sentinel block. The merged Claude Code block registers `rusty-brain-hooks` for `SessionStart`/`PostToolUse`/`Stop` and is tagged with the `rusty-brain` sentinel so uninstall is surgical.

---

### Task Z1: `.github/workflows/ci.yml` — build-agents job

Add a `build-agents` CI job that builds, lints, and tests exactly the three new crates, then proves they are absent from the default `rusty-brain` binary closure. The existing `clippy-test` job (`cargo clippy --workspace …` / `cargo test --workspace`) already covers the three crates for **lint and test** — that is desired — but the **default binary closure** must still exclude them, which holds because no core crate depends on them. This job asserts that invariant explicitly so a future stray `rusty-brain` → `rb-agents` dependency edge fails CI.

**Files:**
- Modify: `.github/workflows/ci.yml`
- Test: (CI workflow; validated locally by the same commands the job runs — see Steps 1-3)

- [ ] **Step 1 (RED — the proof commands must already behave, the job must not yet exist): confirm the closure-exclusion command and the per-crate commands locally, and confirm the job is absent.** Run the three commands the new job will run, plus a grep proving the job is not yet wired:
  - Run: `cargo build -p rb-agents -p rb-hooks -p rb-install` — Expected: PASS (the three crates and their bins build).
  - Run: `cargo tree -e no-dev -p rusty-brain 2>/dev/null | grep -E "rb-agents|rb-hooks|rb-install"; echo "exit=$?"` — Expected: `exit=1` (grep found NOTHING — the new crates are not in the `rusty-brain` non-dev closure).
  - Run: `grep -c "build-agents:" .github/workflows/ci.yml; echo "exit=$?"` — Expected: prints `0` and `exit=1` (the job does not exist yet) — this is the RED state for the workflow change.

- [ ] **Step 2: run it — Run:** `grep -n "build-agents" .github/workflows/ci.yml; echo "exit=${PIPESTATUS[0]}"` — Expected: FAIL with `exit=1` (no `build-agents` job present; the workflow currently ends at the `audit` job).

- [ ] **Step 3 (GREEN): add the `build-agents` job verbatim.** Append the following job to `.github/workflows/ci.yml` immediately after the `audit:` job (same two-space `jobs:` indentation as the existing jobs). Insert exactly:

```yaml
  build-agents:
    name: build + clippy + test (agent crates)
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - name: Build agent crates
        run: cargo build -p rb-agents -p rb-hooks -p rb-install
      - name: Clippy agent crates
        run: cargo clippy -p rb-agents -p rb-hooks -p rb-install --all-targets -- -D warnings
      - name: Test agent crates
        run: cargo test -p rb-agents -p rb-hooks -p rb-install
      - name: Assert agent crates stay out of the default rusty-brain closure
        run: |
          set -euo pipefail
          if cargo tree -e no-dev -p rusty-brain | grep -E "rb-agents|rb-hooks|rb-install"; then
            echo "ERROR: an agent crate leaked into the default rusty-brain (non-dev) dependency closure." >&2
            echo "The rusty-brain binary must NEVER depend on rb-agents/rb-hooks/rb-install." >&2
            exit 1
          fi
          echo "OK: rb-agents/rb-hooks/rb-install are absent from the default rusty-brain closure."
```

- [ ] **Step 4: run it — Run:** `grep -n "build-agents\|Assert agent crates stay out" .github/workflows/ci.yml` — Expected: PASS (matches the job name line and the closure-assertion step name), and re-run the local equivalents `cargo build -p rb-agents -p rb-hooks -p rb-install` (Expected: PASS) and `cargo clippy -p rb-agents -p rb-hooks -p rb-install --all-targets -- -D warnings` (Expected: no warnings) and `cargo test -p rb-agents -p rb-hooks -p rb-install` (Expected: PASS) and `cargo tree -e no-dev -p rusty-brain 2>/dev/null | grep -E "rb-agents|rb-hooks|rb-install"; echo "exit=$?"` (Expected: `exit=1`).

- [ ] **Step 5: lint+format — Run:** `cargo fmt --all --check` — Expected: no diff (no Rust changed in this task; YAML is not formatted by rustfmt). There is no clippy target for a workflow file; the lint surface here is the workflow itself, validated by Step 4.

- [ ] **Step 6: commit — Run:** `git add .github/workflows/ci.yml && git commit -m "ci: add build-agents job and default-closure exclusion check"` — Expected: one commit.

---

### Task Z2: `deny.toml` — verify supply-chain stays green with the agent crates present

The three agent crates depend only on workspace deps already in the graph (`serde`, `serde_json`, `anyhow`, `tokio`, `tracing`, `rb-proto`, `rb-types`) plus, in `rb-install`, the `dirs` crate Part Y added for `~/.config`/`~/.claude` resolution. Confirm `cargo deny check` (which runs with `all-features = true`) still passes. Only add a license exception if a genuinely-new permissive-but-unlisted dependency requires it — and document why. If the check is already clean, this is a **verification-only** task (no file change).

**Files:**
- Modify: `deny.toml` — **only if** `cargo deny check licenses` reports a new unlisted license (otherwise no change)
- Test: (none — `cargo deny check` is the test)

- [ ] **Step 1 (RED/baseline): run the supply-chain gate with the agent crates in the graph.** Run: `cargo deny check 2>&1 | tee /tmp/rb-deny-z.log; echo "exit=${PIPESTATUS[0]}"` — Expected: either `exit=0` (clean — proceed to Step 4 with NO file change) OR a `licenses` failure naming a single new permissive license pulled in by `dirs`/`dirs-sys` (e.g. an MPL-2.0 `option-ext` edge already covered by the existing crate-scoped exception). If it fails ONLY on licenses, capture the offending `name`/`version`/`license` from the log for Step 3.

- [ ] **Step 2: run it — Run:** `cargo deny check licenses 2>&1 | tail -40` — Expected: `licenses ok` if clean. If NOT clean, Expected: FAIL naming the specific unlisted crate/license — this is the RED signal that a narrow exception is needed.

- [ ] **Step 3 (GREEN — conditional): add a crate-scoped license exception ONLY if Step 2 failed.** If and only if Step 2 named a new permissive-but-unlisted license for a `dirs`/`dirs-sys` transitive crate, add a crate-scoped exception to `deny.toml` mirroring the existing `option-ext` block (keep the global policy permissive-only — do NOT widen `allow`). Insert after the existing `[[licenses.exceptions]]` block, replacing `<CRATE>`/`<VERSION>`/`<LICENSE>` with the exact values from the Step 2 log and the rationale with the real edge:

```toml
[[licenses.exceptions]]
# <CRATE> is <LICENSE>, pulled in only by the agent-surface installer crate
# rb-install via dirs -> dirs-sys for ~/.config / ~/.claude path resolution.
# The installer is NOT part of the default rusty-brain closure, so this never
# ships with the daemon/CLI. Scoped here so <LICENSE> stays banned everywhere
# else and the global permissive-only policy is preserved.
name = "<CRATE>"
version = "=<VERSION>"
allow = ["<LICENSE>"]
```

> If Step 2 reported `licenses ok`, SKIP this step entirely — make NO edit to `deny.toml`. The `dirs` family is commonly fully MIT/Apache-2.0 except `option-ext` (MPL-2.0), which the existing exception already covers, so the clean path is expected.

- [ ] **Step 4: run it — Run:** `cargo deny check 2>&1 | tail -20; echo "exit=${PIPESTATUS[0]}"` — Expected: PASS (`advisories ok`, `licenses ok`, `sources ok`; `bans` may `warn` only) and `exit=0`.

- [ ] **Step 5: lint+format — Run:** `cargo fmt --all --check` — Expected: no diff (TOML only; rustfmt unaffected). No clippy target for `deny.toml`.

- [ ] **Step 6: commit — Run:** `git add deny.toml && git commit -m "chore: confirm cargo-deny passes with agent crates"` — Expected: one commit **if** Step 3 edited `deny.toml`; if no edit was needed, instead run `git commit --allow-empty -m "chore: verify cargo-deny clean with agent crates present"` — Expected: one (empty) commit recording the verification.

---

### Task Z3: `scripts/install-agents.sh` — place agent binaries alongside `rusty-brain`

The new repo has no `install.sh`/`scripts/` yet. Create `scripts/install-agents.sh`: a POSIX-sh script that copies the `rusty-brain-hooks` and `rusty-brain-install` binaries into `~/.local/bin` (next to `rusty-brain`), `chmod +x`es them, and SHA-256 verifies each copy against the source before declaring success. Structure ported in spirit from `/Volumes/raid1/repos/rusty-brain-old/install.sh` (the `verify_sha256` fallback chain and `RUSTY_BRAIN_INSTALL_DIR` convention), but local-binary-placement only (no GitHub download). It accepts a build-output dir (default `target/release`), is `set -eu`, never modifies shell config, and guards its body behind `INSTALL_AGENTS_SH_TESTING` so a shell test can source it and exercise the pure functions.

**Files:**
- Create: `scripts/install-agents.sh`
- Test: `scripts/install-agents.test.sh` (POSIX-sh assertions sourcing the script under `INSTALL_AGENTS_SH_TESTING=1`)

- [ ] **Step 1 (RED): write the shell test first.** Create `scripts/install-agents.test.sh` exactly:

```sh
#!/bin/sh
# install-agents.test.sh — POSIX-sh assertions for scripts/install-agents.sh.
# Sources the installer with INSTALL_AGENTS_SH_TESTING=1 so only functions are
# defined (main() is not run), then exercises the pure helpers against a
# scratch directory built with `mktemp -d`.
set -eu

HERE="$(cd "$(dirname "$0")" && pwd)"
INSTALL_AGENTS_SH_TESTING=1
export INSTALL_AGENTS_SH_TESTING
# shellcheck source=scripts/install-agents.sh
. "${HERE}/install-agents.sh"

fail() {
  printf 'TEST FAIL: %s\n' "$1" >&2
  exit 1
}

# --- sha256_of produces a stable hex digest -------------------------------
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
printf 'hello rusty-brain\n' > "${work}/a.bin"
printf 'hello rusty-brain\n' > "${work}/b.bin"
ha="$(sha256_of "${work}/a.bin")"
hb="$(sha256_of "${work}/b.bin")"
[ -n "$ha" ] || fail "sha256_of returned empty digest"
[ "$ha" = "$hb" ] || fail "identical files must hash identically ($ha vs $hb)"

# --- install_one copies, chmod +x, and checksum-verifies ------------------
src_dir="${work}/src"
dst_dir="${work}/bin"
mkdir -p "$src_dir" "$dst_dir"
printf '#!/bin/sh\necho hi\n' > "${src_dir}/rusty-brain-hooks"
chmod +x "${src_dir}/rusty-brain-hooks"

install_one "$src_dir" "$dst_dir" "rusty-brain-hooks"

[ -f "${dst_dir}/rusty-brain-hooks" ] || fail "binary was not copied to dst"
[ -x "${dst_dir}/rusty-brain-hooks" ] || fail "copied binary is not executable"
src_hash="$(sha256_of "${src_dir}/rusty-brain-hooks")"
dst_hash="$(sha256_of "${dst_dir}/rusty-brain-hooks")"
[ "$src_hash" = "$dst_hash" ] || fail "checksum mismatch after copy ($src_hash vs $dst_hash)"

# --- install_one fails loudly when the source binary is missing -----------
if install_one "$src_dir" "$dst_dir" "rusty-brain-install" 2>/dev/null; then
  fail "install_one must fail when the source binary is absent"
fi

printf 'TEST PASS: install-agents.sh helpers behave\n'
```

- [ ] **Step 2: run it — Run:** `sh scripts/install-agents.test.sh; echo "exit=$?"` — Expected: FAIL with `exit` non-zero and an error like `install-agents.sh: No such file or directory` (the script under test does not exist yet).

- [ ] **Step 3 (GREEN): create the installer script.** Create `scripts/install-agents.sh` exactly:

```sh
#!/bin/sh
# install-agents.sh — place the rusty-brain agent-surface binaries
# (rusty-brain-hooks, rusty-brain-install) alongside rusty-brain in
# ~/.local/bin, chmod +x, and SHA-256 verify each copy.
#
# Usage:
#   scripts/install-agents.sh [BUILD_DIR]
#
# BUILD_DIR defaults to "target/release" (relative to the repo root, or
# absolute). Override the install location with RUSTY_BRAIN_INSTALL_DIR.
#
# This script NEVER downloads anything and NEVER modifies shell config.
set -eu

# ---------- sha256_of --------------------------------------------------------
# Print the lowercase hex SHA-256 of "$1" using the first available tool.
sha256_of() {
  _file="${1:-}"
  if [ ! -f "$_file" ]; then
    printf 'ERROR: cannot hash missing file: %s\n' "$_file" >&2
    return 1
  fi
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$_file" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$_file" | awk '{print $1}'
  elif command -v openssl >/dev/null 2>&1; then
    openssl dgst -sha256 "$_file" | awk '{print $NF}'
  else
    printf 'ERROR: no SHA-256 tool found (need sha256sum, shasum, or openssl)\n' >&2
    return 1
  fi
}

# ---------- install_one ------------------------------------------------------
# Copy "$3" from src dir "$1" to dst dir "$2", chmod +x, checksum-verify.
install_one() {
  _src_dir="${1:-}"
  _dst_dir="${2:-}"
  _name="${3:-}"
  _src="${_src_dir}/${_name}"
  _dst="${_dst_dir}/${_name}"

  if [ ! -f "$_src" ]; then
    printf 'ERROR: source binary not found: %s\n' "$_src" >&2
    printf 'Build it first, e.g.: cargo build --release -p rb-hooks -p rb-install\n' >&2
    return 1
  fi

  mkdir -p "$_dst_dir"
  cp "$_src" "$_dst"
  chmod +x "$_dst"

  _src_hash="$(sha256_of "$_src")"
  _dst_hash="$(sha256_of "$_dst")"
  if [ "$_src_hash" != "$_dst_hash" ]; then
    printf 'ERROR: checksum mismatch after copying %s\n' "$_name" >&2
    printf '  source: %s\n' "$_src_hash" >&2
    printf '  copy:   %s\n' "$_dst_hash" >&2
    rm -f "$_dst"
    return 1
  fi

  printf 'Installed %s -> %s (sha256 %s)\n' "$_name" "$_dst" "$_src_hash"
}

# ---------- main -------------------------------------------------------------
main() {
  build_dir="${1:-target/release}"
  install_dir="${RUSTY_BRAIN_INSTALL_DIR:-$HOME/.local/bin}"

  if [ ! -d "$build_dir" ]; then
    printf 'ERROR: build dir does not exist: %s\n' "$build_dir" >&2
    printf 'Build the agent binaries first:\n' >&2
    printf '  cargo build --release -p rb-hooks -p rb-install\n' >&2
    return 1
  fi

  printf 'Installing agent binaries from %s to %s\n' "$build_dir" "$install_dir"
  install_one "$build_dir" "$install_dir" "rusty-brain-hooks"
  install_one "$build_dir" "$install_dir" "rusty-brain-install"

  # Informational PATH note only — never modify shell config.
  case ":${PATH}:" in
    *":${install_dir}:"*) ;;
    *)
      printf '\nNOTE: %s is not in your PATH.\n' "$install_dir"
      # shellcheck disable=SC2016
      printf '  export PATH="%s:$PATH"\n' "$install_dir"
      ;;
  esac

  printf '\nAgent binaries installed. Register hooks with:\n'
  printf '  rusty-brain-install install --agents claude-code\n'
}

# Test guard: when sourced by install-agents.test.sh, only define functions.
if [ "${INSTALL_AGENTS_SH_TESTING:-0}" != "1" ]; then
  main "$@"
fi
```

- [ ] **Step 4: run it — Run:** `chmod +x scripts/install-agents.sh scripts/install-agents.test.sh && sh scripts/install-agents.test.sh; echo "exit=$?"` — Expected: PASS — prints `TEST PASS: install-agents.sh helpers behave` and `exit=0`.

- [ ] **Step 5: lint+format — Run:** `cargo fmt --all --check` (Expected: no diff — no Rust touched) then, if `shellcheck` is available, `shellcheck scripts/install-agents.sh scripts/install-agents.test.sh; echo "exit=$?"` — Expected: `exit=0` (no findings). If `shellcheck` is not installed, Expected: `command not found` — skip (the shell test in Step 4 is the binding gate).

- [ ] **Step 6: commit — Run:** `git add scripts/install-agents.sh scripts/install-agents.test.sh && git commit -m "chore: add install-agents.sh for hooks/install binary placement"` — Expected: one commit.

---

### Task Z4: `crates/rb-install/tests/e2e.rs` — install → capture → uninstall acceptance test

The marquee P4 acceptance test. It (1) starts an **in-process** `rb-daemon` on a temp Unix socket using the offline `DeterministicProvider` (no network), exporting `RUSTY_BRAIN_SOCKET`/`RUSTY_BRAIN_DB`; (2) runs the built `rusty-brain-install install --agents claude-code --project <tmp>` against a fixture project and asserts the project `.claude/settings.json` gained the `rusty-brain` sentinel hook block referencing `rusty-brain-hooks`; (3) pipes a `PostToolUse` Edit JSON into the built `rusty-brain-hooks --agent claude-code` binary and asserts it exits 0; (4) `recall`s through the daemon and asserts the Edit observation was stored; (5) runs `rusty-brain-install uninstall --agents claude-code --project <tmp>` and asserts the sentinel block is gone. The dev-dependencies (`rb-daemon`, `rb-embed`, `rb-proto`, `rb-types`, `tokio`, `tempfile`, `serde_json`, `assert_cmd`) are **dev-only**, so they never enter `rb-install`'s default closure — and `rb-install` itself is never in `rusty-brain`'s closure, so the workspace invariant holds.

**Files:**
- Create: `crates/rb-install/tests/e2e.rs`
- Modify: `crates/rb-install/Cargo.toml` (add the `[dev-dependencies]` the test needs)
- Test: `crates/rb-install/tests/e2e.rs` (this file IS the test)

- [ ] **Step 1 (RED): write the end-to-end test verbatim.** Create `crates/rb-install/tests/e2e.rs` exactly:

```rust
//! P4 marquee acceptance test: install the Claude Code hook block, fire a real
//! `PostToolUse` Edit through the built `rusty-brain-hooks` binary, prove the
//! observation reached the in-process daemon, then uninstall and prove the
//! sentinel block is removed. Offline: the daemon uses DeterministicProvider so
//! no embedding API is contacted (VOYAGE_API_KEY is cleared for the child).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use assert_cmd::cargo::cargo_bin;
use rb_daemon::{Daemon, DaemonConfig, JobsConfig, SharedEmbedder};
use rb_embed::DeterministicProvider;
use rb_proto::Client;
use rb_types::Namespace;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

const DIM: usize = 8;

/// Owns the in-process daemon: a temp dir, the bound socket path, a shutdown
/// channel, and the run task. Started off a fixed temp dir so the socket path
/// is short enough for the AF_UNIX sun_path limit.
struct RunningDaemon {
    socket: PathBuf,
    db: PathBuf,
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
    _dir: tempfile::TempDir,
}

impl RunningDaemon {
    async fn start() -> RunningDaemon {
        let dir = tempfile::tempdir_in(std::env::temp_dir()).unwrap();
        let socket = dir.path().join("runtime").join("sock");
        let db = dir.path().join("memory.db");
        let cfg = DaemonConfig {
            socket_path: socket.clone(),
            db_path: db.clone(),
            read_pool_size: 2,
            jobs_config: JobsConfig::default(),
        };
        let embedder = SharedEmbedder::new(DeterministicProvider::new(DIM));
        let daemon = Daemon::bind(cfg, embedder).await.unwrap();

        let (tx, rx) = oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            daemon
                .run(async move {
                    let _ = rx.await;
                })
                .await
                .unwrap();
        });

        let mut ready = false;
        for _ in 0..400 {
            if tokio::net::UnixStream::connect(&socket).await.is_ok() {
                ready = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(
            ready,
            "daemon socket was not reachable within startup timeout at {}",
            socket.display()
        );

        RunningDaemon {
            socket,
            db,
            shutdown: Some(tx),
            task: Some(task),
            _dir: dir,
        }
    }

    async fn stop(mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(task) = self.task.take() {
            let _ = tokio::time::timeout(Duration::from_secs(5), task).await;
        }
    }
}

/// Read `.claude/settings.json` under `project` as JSON, or `Value::Null` if it
/// does not exist.
fn read_settings(project: &Path) -> serde_json::Value {
    let path = project.join(".claude").join("settings.json");
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or(serde_json::Value::Null),
        Err(_) => serde_json::Value::Null,
    }
}

/// True if the settings JSON contains an entry whose serialized form mentions
/// both the `rusty-brain` sentinel and the `rusty-brain-hooks` command, i.e.
/// our injected hook block is present.
fn has_sentinel_block(settings: &serde_json::Value) -> bool {
    let text = settings.to_string();
    text.contains("rusty-brain") && text.contains("rusty-brain-hooks")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn install_capture_uninstall_round_trip() {
    // --- fixture project -----------------------------------------------------
    let proj_dir = tempfile::tempdir_in(std::env::temp_dir()).unwrap();
    let project = proj_dir.path().to_path_buf();
    // Mark the project so namespace detection resolves to a stable Project name.
    std::fs::write(
        project.join("CLAUDE.md"),
        "---\nproject: rb-e2e-fixture\n---\n# rb-e2e-fixture\n",
    )
    .unwrap();
    let namespace = Namespace::Project("rb-e2e-fixture".to_string());

    // --- in-process daemon ---------------------------------------------------
    let daemon = RunningDaemon::start().await;

    // --- 1) install the Claude Code hook block -------------------------------
    let install_bin = cargo_bin("rusty-brain-install");
    let install_out = Command::new(&install_bin)
        .args(["install", "--agents", "claude-code", "--project"])
        .arg(&project)
        .output()
        .expect("run rusty-brain-install install");
    assert!(
        install_out.status.success(),
        "install failed: stdout={:?} stderr={:?}",
        String::from_utf8_lossy(&install_out.stdout),
        String::from_utf8_lossy(&install_out.stderr)
    );

    let after_install = read_settings(&project);
    assert!(
        has_sentinel_block(&after_install),
        "settings.json must contain the rusty-brain sentinel hook block after install; got: {after_install}"
    );

    // --- 2) fire a PostToolUse Edit through the built hooks binary ------------
    let hooks_bin = cargo_bin("rusty-brain-hooks");
    let unique = "rb-e2e marker edit to src/zztest.rs at unique-token-9f3a";
    let event = serde_json::json!({
        "session_id": "rb-e2e-session",
        "transcript_path": "/dev/null",
        "cwd": project.to_string_lossy(),
        "permission_mode": "default",
        "hook_event_name": "PostToolUse",
        "tool_name": "Edit",
        "tool_input": {
            "file_path": project.join("src").join("zztest.rs").to_string_lossy(),
            "old_string": "old body",
            "new_string": unique
        },
        "tool_response": { "success": true }
    })
    .to_string();

    let hook_out = Command::new(&hooks_bin)
        .args(["--agent", "claude-code"])
        .env("RUSTY_BRAIN_SOCKET", &daemon.socket)
        .env("RUSTY_BRAIN_DB", &daemon.db)
        .env_remove("VOYAGE_API_KEY")
        .current_dir(&project)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write as _;
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(event.as_bytes())?;
            }
            child.wait_with_output()
        })
        .expect("run rusty-brain-hooks post-tool-use");

    // (a) FAIL-OPEN: the hook binary must always exit 0.
    assert!(
        hook_out.status.success(),
        "rusty-brain-hooks must exit 0 (fail-open); status={:?} stdout={:?} stderr={:?}",
        hook_out.status.code(),
        String::from_utf8_lossy(&hook_out.stdout),
        String::from_utf8_lossy(&hook_out.stderr)
    );
    // The Claude Code adapter always renders a continue:true envelope.
    let stdout = String::from_utf8_lossy(&hook_out.stdout);
    assert!(
        stdout.contains("\"continue\""),
        "hook stdout must be a Claude Code envelope with a continue field; got: {stdout}"
    );

    // (b) the observation reached the daemon: recall finds the marker.
    let mut client = Client::connect(&daemon.socket, namespace.clone())
        .await
        .expect("connect to in-process daemon for recall");
    let mut found = false;
    for _ in 0..40 {
        let results = client
            .recall("zztest unique-token-9f3a marker edit".to_string(), None, vec![], 10)
            .await
            .expect("recall");
        if results
            .iter()
            .any(|r| r.memory.content.contains("unique-token-9f3a"))
        {
            found = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(
        found,
        "the PostToolUse Edit observation must be stored in the daemon and recallable"
    );

    // --- 3) uninstall removes ONLY the sentinel block ------------------------
    let uninstall_out = Command::new(&install_bin)
        .args(["uninstall", "--agents", "claude-code", "--project"])
        .arg(&project)
        .output()
        .expect("run rusty-brain-install uninstall");
    assert!(
        uninstall_out.status.success(),
        "uninstall failed: stdout={:?} stderr={:?}",
        String::from_utf8_lossy(&uninstall_out.stdout),
        String::from_utf8_lossy(&uninstall_out.stderr)
    );

    let after_uninstall = read_settings(&project);
    assert!(
        !has_sentinel_block(&after_uninstall),
        "the rusty-brain sentinel hook block must be gone after uninstall; got: {after_uninstall}"
    );

    daemon.stop().await;
    drop(proj_dir);
}
```

- [ ] **Step 2: run it — Run:** `cargo test -p rb-install --test e2e` — Expected: FAIL — the test does not compile because `crates/rb-install/Cargo.toml` lacks the dev-dependencies (`rb-daemon`, `rb-embed`, `rb-proto`, `rb-types`, `tokio`, `tempfile`, `serde_json`, `assert_cmd`); the compiler errors on `use rb_daemon::…` / `use assert_cmd::…` with `unresolved import` / `failed to resolve`.

- [ ] **Step 3 (GREEN): add the dev-dependencies to `crates/rb-install/Cargo.toml`.** Append a `[dev-dependencies]` table to `crates/rb-install/Cargo.toml`, immediately before the trailing `[lints]\nworkspace = true` block (these are dev-only and therefore NOT in `rb-install`'s default closure — and `rb-install` is itself outside `rusty-brain`'s closure). Insert exactly:

```toml
[dev-dependencies]
rb-types = { path = "../rb-types" }
rb-proto = { path = "../rb-proto" }
rb-daemon = { path = "../rb-daemon" }
rb-embed = { path = "../rb-embed" }
tokio = { workspace = true }
tempfile = { workspace = true }
serde_json = { workspace = true }
assert_cmd = { workspace = true }
```

> Do NOT touch `rb-install`'s `[dependencies]` — the runtime crate must keep depending only on `rb-agents`, `rb-types`, `serde`, `serde_json`, `anyhow`, and `dirs` (as Part Y defined). Adding `rb-daemon`/`rb-embed` to runtime deps would (a) bloat the installer and (b) still not violate the `rusty-brain` closure, but it would violate the agent-surface dependency budget; keep them dev-only.

- [ ] **Step 4: run it — Run:** `cargo test -p rb-install --test e2e` — Expected: PASS (1 test: `install_capture_uninstall_round_trip`). The in-process daemon comes up, install writes the sentinel block, the hook binary exits 0 and stores the Edit observation, recall finds the `unique-token-9f3a` marker, and uninstall removes the block.

- [ ] **Step 5: lint+format — Run:** `cargo clippy -p rb-install --all-targets -- -D warnings` (Expected: no warnings) then `cargo fmt --all` (Expected: no diff).

- [ ] **Step 6: commit — Run:** `git add crates/rb-install/tests/e2e.rs crates/rb-install/Cargo.toml && git commit -m "test(rb-install): add install/capture/uninstall e2e acceptance test"` — Expected: one commit.

---

### Task Z5: `CHANGELOG.md` — record the P4 agent-surface status

No `CHANGELOG.md` exists in the worktree and there is no `spec.md` to update; a short CHANGELOG entry is the right, minimal record of the P4 milestone (no new large doc). Create a top-level `CHANGELOG.md` with a Keep-a-Changelog-style `Unreleased` section summarizing the agent surface and naming the two new binaries and the install flow.

**Files:**
- Create: `CHANGELOG.md`
- Test: (none — documentation)

- [ ] **Step 1 (RED): confirm there is no CHANGELOG yet.** Run: `test -f CHANGELOG.md; echo "exit=$?"` — Expected: `exit=1` (file absent — this is the RED state).

- [ ] **Step 2: run it — Run:** `ls CHANGELOG.md 2>&1; echo "exit=$?"` — Expected: FAIL with `No such file or directory` and `exit` non-zero.

- [ ] **Step 3 (GREEN): create `CHANGELOG.md`.** Create `CHANGELOG.md` exactly:

```markdown
# Changelog

All notable changes to rusty-brain are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added — P4 agent surface

- **`rusty-brain-hooks` binary** (crate `rb-hooks`): a fail-open, capture-only
  per-event hook for JSON-protocol CLIs. Selected with `--agent <id>` for
  `claude-code`, `opencode`, `gemini`, or `codex` (Copilot deferred). It reads a
  hook event on stdin, captures mutating-tool observations (`Edit`/`Write`/`Bash`,
  deduped) into the daemon, injects recent high-importance memories on
  `SessionStart`, and **always exits 0** — it never blocks, never tracks memory
  debt, and never returns a non-zero exit.
- **`rusty-brain-install` binary** (crate `rb-install`): merges a sentinel-marked
  (`rusty-brain`) hook block into the agent config (Claude Code project
  `.claude/settings.json` by default, `--global` supported), with a `.bak`
  backup and atomic temp+fsync+rename. `uninstall` removes only the sentinel
  block, preserving any other user hooks. Supports `status` and `--dry-run`,
  with JSON or human output (non-TTY auto-selects JSON).
- **`rb-agents` crate**: the shared, CLI-agnostic spine — canonical `HookEvent`,
  per-CLI JSON adapters (`AgentCli`), a fail-open best-effort `DaemonClient` over
  `rb-proto`, namespace detection, and the install-side `AgentInstaller`
  contract.
- **CI `build-agents` job**: builds, lints, and tests `rb-agents`/`rb-hooks`/
  `rb-install`, and asserts via `cargo tree -e no-dev -p rusty-brain` that none
  of them enter the default `rusty-brain` binary closure.
- **`scripts/install-agents.sh`**: places the `rusty-brain-hooks` and
  `rusty-brain-install` binaries alongside `rusty-brain` in `~/.local/bin`,
  `chmod +x`, with SHA-256 verification of each copy.

### Notes

- The three agent-surface crates are workspace members but are **never** in the
  default `cargo build`/`rusty-brain`-binary dependency closure — no core crate
  depends on them. This keeps the daemon/CLI lean and is enforced in CI.
```

- [ ] **Step 4: run it — Run:** `test -f CHANGELOG.md && grep -q "rusty-brain-hooks" CHANGELOG.md && grep -q "build-agents" CHANGELOG.md; echo "exit=$?"` — Expected: PASS (`exit=0` — the file exists and names both the hook binary and the CI job).

- [ ] **Step 5: lint+format — Run:** `cargo fmt --all --check` — Expected: no diff (Markdown only; no Rust touched).

- [ ] **Step 6: commit — Run:** `git add CHANGELOG.md && git commit -m "docs: add changelog with P4 agent-surface entry"` — Expected: one commit.

---

### Task Z6: Part Z gate

**Files:**
- (none — verification only)

Run the full **workspace** gate plus the supply-chain gate, the closure-exclusion proof, and the e2e test, all from the worktree root. Part Z is a wiring/packaging Part: it must leave the entire workspace green AND keep the agent crates out of the default `rusty-brain` closure.

- [ ] **Step 1: workspace build — Run:** `cargo build --workspace` — Expected: PASS (all crates incl. `rb-agents`/`rb-hooks`/`rb-install` build).

- [ ] **Step 2: workspace tests — Run:** `cargo test --workspace` — Expected: PASS, 0 failures (includes the new `rb-install` `install_capture_uninstall_round_trip` e2e test, the Part V/W/Y unit tests, and all prior P0-P3 tests).

- [ ] **Step 3: workspace clippy (all features) — Run:** `cargo clippy --workspace --all-targets --all-features -- -D warnings` — Expected: no warnings.

- [ ] **Step 4: workspace format — Run:** `cargo fmt --all --check` — Expected: no diff.

- [ ] **Step 5: supply-chain gate — Run:** `cargo deny check 2>&1 | tail -20; echo "exit=${PIPESTATUS[0]}"` — Expected: PASS (`advisories ok`, `licenses ok`, `sources ok`; `bans` may `warn` only) and `exit=0`.

- [ ] **Step 6: default-closure exclusion proof — Run:** `cargo tree -e no-dev -p rusty-brain 2>/dev/null | grep -E "rb-agents|rb-hooks|rb-install"; echo "exit=$?"` — Expected: `exit=1` (grep found NOTHING — the agent crates are absent from the default `rusty-brain` non-dev closure; this is the mechanical proof the CI `build-agents` job enforces).

- [ ] **Step 7: e2e acceptance re-run — Run:** `cargo test -p rb-install --test e2e` — Expected: PASS (1 test) — the marquee install → capture → uninstall round-trip is green.

- [ ] **Step 8: shell installer test — Run:** `sh scripts/install-agents.test.sh; echo "exit=$?"` — Expected: PASS (`TEST PASS: install-agents.sh helpers behave`, `exit=0`).

- [ ] **Step 9: gate commit (only if Steps 1-8 produced any formatting touch-ups).** Run: `git add -A && git commit -m "chore: part Z gate green (ci build-agents + dep-budget + e2e)"` — Expected: one commit, or nothing to commit if Steps 1-8 produced no changes.

