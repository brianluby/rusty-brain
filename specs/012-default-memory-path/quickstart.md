# Quickstart: Default Memory Path Change

## What Changed

The default memory directory changed from `.agent-brain/` to `.rusty-brain/`.

## For New Users

No action needed. rusty-brain automatically creates `.rusty-brain/mind.mv2` on first use.

## For Existing Users

If you have an existing `.agent-brain/` directory:

```bash
# Move the entire directory (preserves all data files)
# If .rusty-brain/ doesn't exist yet:
mv .agent-brain .rusty-brain

# Or if .rusty-brain/ already exists (e.g. from supporting files):
mkdir -p .rusty-brain
mv .agent-brain/mind.mv2 .rusty-brain/mind.mv2
# Then remove old directory: rm -rf .agent-brain

# Update .gitignore (macOS)
sed -i '' 's/\.agent-brain/\.rusty-brain/' .gitignore
# On Linux: sed -i 's/\.agent-brain/\.rusty-brain/' .gitignore
```

If you have the oldest `.claude/mind.mv2` legacy path:

```bash
mkdir -p .rusty-brain
mv .claude/mind.mv2 .rusty-brain/mind.mv2
```

## What Happens Without Migration

- Your existing `.agent-brain/mind.mv2` continues to work (graceful fallback)
- You'll see an info message suggesting migration
- New supporting files (dedup cache, version marker) go to `.rusty-brain/`

## Environment Variable Override

Custom paths via `MEMVID_PLATFORM_MEMORY_PATH` are unaffected by this change.
