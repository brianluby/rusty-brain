You are implementing one small change in the rusty-brain Rust workspace, in the current git worktree.

Rules:
- Make only the change described below. Do not commit, push, or create branches. Do not edit CHANGELOG or docs.
- Add focused tests for the behavior you implement.
- Before finishing, run `cargo clippy -p <each crate you touched> --all-targets -- -D warnings` (add `--features local` where the task needs it) and `cargo nextest run -p <each crate you touched>`, and fix what fails.
- Keep the change minimal and idiomatic with the surrounding code. No new crates.
- Finish with a short summary of what you changed and how you tested it.

Task:
