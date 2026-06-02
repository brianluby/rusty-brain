# Follow-up: Capture Codex `apply_patch` edits once Codex emits PostToolUse for it

- **Date:** 2026-06-02
- **Area:** `rb-hooks` capture flow / `rb-agents` Codex adapter (P4 broader agent surface)
- **Status:** Deferred (blocked upstream)
- **Severity:** Low (latent — cannot occur today)

## Summary

Codex's file-edit tool `apply_patch` is **not** recognized by
`rb_hooks::capture::normalize_tool`, so a Codex `apply_patch` PostToolUse event
would degrade to a no-op capture. This is intentionally deferred — see the inline
note in `crates/rb-hooks/src/capture.rs` (the `normalize_tool` match).

## Verified facts (2026-06-02)

- Codex's **shell** tool reports `tool_name: "Bash"` — Claude-style. Codex shell
  capture therefore **already works** today via the existing `"bash" => "Bash"`
  arm, and the `codex.rs` adapter test using `"Bash"` is accurate.
  Sources: [Codex Hooks (OpenAI)](https://developers.openai.com/codex/hooks),
  [Codex CLI Hooks complete guide](https://codex.danielvaughan.com/2026/04/15/codex-cli-hooks-complete-guide-events-policy-patterns/)
  ("the tool name is always `Bash` in Codex CLI").
- Codex's **file edit** tool reports `tool_name: "apply_patch"` (matchers may use
  `apply_patch`/`Edit`/`Write`, but the hook input always reports
  `tool_name: "apply_patch"`). This name is not in `normalize_tool`.
- **Blocker:** Codex does **not** currently fire PreToolUse/PostToolUse for
  `apply_patch` — hooks fire only for the shell (Bash) tool. So the gap cannot be
  exercised today.
  Source: [openai/codex#16732 — ApplyPatchHandler doesn't emit PreToolUse/PostToolUse](https://github.com/openai/codex/issues/16732)
- **Input-shape wrinkle:** `apply_patch` carries `tool_input.command` (the raw
  patch), not a `file_path`. A bare `"apply_patch" => "Edit"` mapping would
  summarize as `"Edited unknown"`, so a proper fix must extract the edited path
  from the patch.

## What to do when unblocked

When openai/codex#16732 ships (Codex emits PostToolUse for `apply_patch` and the
payload exposes the edited path):

1. Add an `"apply_patch" => "Edit"` arm to `normalize_tool` in
   `crates/rb-hooks/src/capture.rs` (next to the Gemini arms).
2. Extend `summarize_post_tool_use` to parse the `apply_patch` payload and emit a
   real `"Edited <path>"` summary (verify the exact `tool_input` shape against a
   real Codex build first).
3. Add capture-layer tests mirroring `gemini_tools_are_mutations` and an
   end-to-end captured test using the real Codex `apply_patch` name.
4. Optionally update `crates/rb-agents/tests/cross_adapter.rs` (which currently
   uses the placeholder `"Write"` for Codex) to assert the real `apply_patch`
   name, so the test reflects actual Codex output.

## Provenance

Surfaced by a multi-agent review of the P4 review-fixes; the original alarm
(symmetric blocking bug with Gemini) was overstated — Codex shell capture works.
The accurate residual is this latent, upstream-blocked `apply_patch` gap.
