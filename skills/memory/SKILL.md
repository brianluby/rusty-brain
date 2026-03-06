---
name: memory
description: Claude Mind - Search and manage Claude's persistent memory stored in a single portable .mv2 file
---

# Claude Memory

Capture and store memories for persistent context across conversations.

## How It Works

Memory capture happens automatically through Claude Code hooks:
- **SessionStart**: Loads existing memory context
- **PostToolUse**: Captures relevant observations from tool interactions
- **Stop**: Persists captured memories to the `.mv2` file

## Storage

Memories are stored in `.agent-brain/mind.mv2` using memvid video-encoded format. This file is portable and persists across sessions.

## Manual Memory Operations

Use the `mind` skill for manual memory operations:
- `/mind:search <query>` - Search existing memories
- `/mind:ask <question>` - Ask questions about stored context
- `/mind:recent` - View recent activity
- `/mind:stats` - View storage statistics
