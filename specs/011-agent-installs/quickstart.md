# Quickstart: Agent Installs

**Feature**: 011-agent-installs | **Date**: 2026-03-05

## Prerequisites

- Rust stable >= 1.85.0
- rusty-brain binary built (`cargo build`)
- At least one supported agent installed: OpenCode, GitHub Copilot CLI, OpenAI Codex CLI, or Google Gemini CLI

## Build

```bash
cargo build
```

## Usage

### Install for a single agent (project-scoped)

```bash
rusty-brain install --agents opencode --project
```

### Install for all detected agents (project-scoped)

```bash
rusty-brain install --project
```

### Install globally (user-level config directories)

```bash
rusty-brain install --global
```

### Install with JSON output (for agentic self-install)

```bash
rusty-brain install --agents opencode --project --json
```

### Reconfigure existing installation

```bash
rusty-brain install --agents opencode --project --reconfigure
```

## Expected Output (JSON mode)

```json
{
  "status": "success",
  "results": [
    {
      "agent_name": "opencode",
      "status": "configured",
      "config_path": "/path/to/project/.opencode/plugins/rusty-brain.json",
      "version_detected": "1.2.3"
    }
  ],
  "memory_store": "/path/to/project/.rusty-brain/mind.mv2",
  "scope": "project"
}
```

## Testing

```bash
# Run all tests
cargo test

# Run install-specific tests
cargo test --package platforms installer
cargo test --package cli install

# Run with verbose logging
RUSTY_BRAIN_LOG=debug cargo test --package platforms installer
```

## Key Files

| File | Purpose |
|------|---------|
| `crates/platforms/src/installer/mod.rs` | `AgentInstaller` trait definition |
| `crates/platforms/src/installer/orchestrator.rs` | Install workflow coordinator |
| `crates/platforms/src/installer/writer.rs` | Atomic file writer with backup |
| `crates/platforms/src/installer/registry.rs` | Installer registry |
| `crates/platforms/src/installer/agents/*.rs` | Per-agent installer implementations |
| `crates/types/src/install.rs` | Install types and error codes |
| `crates/cli/src/args.rs` | CLI subcommand definition |
| `crates/cli/src/install_cmd.rs` | Install command handler |

## Scope Flag Requirement

The `--project` or `--global` flag is **required**. The command will error if neither is specified:

```bash
# ERROR: scope required
rusty-brain install --agents opencode

# OK: project scope
rusty-brain install --agents opencode --project

# OK: global scope
rusty-brain install --agents opencode --global
```
