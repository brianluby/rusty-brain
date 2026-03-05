---
name: mind
description: Search and manage Claude's persistent memory stored in a single portable .mv2 file
---

# Claude Mind

Search and manage Claude's persistent memory.

## Commands

- `/mind:search <query>` - Search memories for specific content or patterns
- `/mind:ask <question>` - Ask questions about memories and get context-aware answers
- `/mind:recent` - Show recent memories and activity timeline
- `/mind:stats` - Show memory statistics and storage information

## Usage

All memory operations use the `rusty-brain` CLI binary. Memories are stored in `.agent-brain/mind.mv2` and persist across conversations.

### Search memories
```bash
rusty-brain find "<query>"
```

### Ask a question
```bash
rusty-brain ask "<question>"
```

### View recent activity
```bash
rusty-brain timeline
```

### View statistics
```bash
rusty-brain stats
```

_Memories are captured automatically from your tool use via hooks._
