# rusty-brain

rusty-brain is a persistent memory system for AI coding agents. It captures observations from agent sessions and stores them locally so your agents remember context across conversations. Supported agents include Claude Code, OpenCode, GitHub Copilot CLI, OpenAI Codex CLI, and Google Gemini CLI — all sharing the same memory store.

## How It Works

Each supported agent is configured to send observations to rusty-brain at the end of every session. Memories are stored in `.rusty-brain/mind.mv2` at your project root. Any configured agent can read and write to this shared store, so context learned in one agent is available to all others.

## Install the Binary

### macOS / Linux

```bash
curl -sSf https://raw.githubusercontent.com/brianluby/rusty-brain/main/install.sh | sh
```

### Windows (PowerShell)

```powershell
irm https://raw.githubusercontent.com/brianluby/rusty-brain/main/install.ps1 | iex
```

## Agent Setup

After installing the binary, configure rusty-brain for each agent you use.

### Claude Code

```
/plugin marketplace add brianluby/rusty-brain
/plugin install rusty-brain@rusty-brain
```

### OpenCode

```bash
rusty-brain install --project --agents opencode
```

### GitHub Copilot CLI

```bash
rusty-brain install --project --agents copilot
```

### OpenAI Codex CLI

```bash
rusty-brain install --project --agents codex
```

### Google Gemini CLI

```bash
rusty-brain install --project --agents gemini
```

### All detected agents at once

```bash
rusty-brain install --project
```

This auto-detects which supported agents are installed on your system and configures all of them in one step.

Use `--global` instead of `--project` to install configuration in user-level directories (e.g., `~/.config/`) so rusty-brain is available across all your projects without per-project setup.

## Using rusty-brain

Search your memories:

```bash
rusty-brain find "authentication"
rusty-brain find "query" --limit 5 --type decision
```

Ask a question about stored context:

```bash
rusty-brain ask "why did we choose PostgreSQL over SQLite?"
```

View recent activity:

```bash
rusty-brain timeline
rusty-brain timeline --limit 20 --oldest-first
```

View memory statistics:

```bash
rusty-brain stats
```

All commands accept `--json` for structured output, which is useful for scripting or piping into other tools.

## Memory Location

Memories are stored in `.rusty-brain/mind.mv2` at your project root. Add this directory to your `.gitignore` to keep personal memory data out of version control:

```bash
echo '.rusty-brain/' >> .gitignore
```

If you have memories stored in an older location (`.agent-brain/mind.mv2` or `.claude/mind.mv2`), rusty-brain detects these on startup and shows migration instructions.

## Contributing

See [docs/developer.md](docs/developer.md) for build instructions, crate layout, quality gates, and the spec-driven development workflow.

## License

Apache-2.0
