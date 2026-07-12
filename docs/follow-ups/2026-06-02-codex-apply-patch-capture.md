# Follow-up: Capture Codex `apply_patch` edits once Codex emits PostToolUse for it

- **Date:** 2026-06-02
- **Area:** `rb-hooks` capture flow / `rb-agents` Codex adapter (P4 broader agent surface)
- **Status:** Codex `apply_patch` capture **landed and live-verified** — the
  upstream blocker shipped and the payload shape is confirmed by a real
  capture (see the 2026-07-12 resolution below).
- **Severity:** Resolved for capture; the residual limitation is the
  fixture-gated `Stop` terminus fold — Codex `apply_patch` edits reach the
  scratch but never fold into a summary until the terminus mapping is
  verified (`docs/plans/2026-06-26-cross-cli-terminus-mapping.md`).

## Resolution (2026-07-12)

The upstream blocker is gone: openai/codex#16732 closed as completed — PR
openai/codex#18391 ("fix(core): emit hooks for apply_patch edits") shipped in
**Codex 0.123.0**. `ApplyPatchHandler` now emits `PreToolUse`/`PostToolUse`
with `tool_name: "apply_patch"` and the **raw V4A patch under
`tool_input.command`** (the speculated field name was correct).

**Payload shape verified EMPIRICALLY**: a live headless `codex exec` session
against codex-cli **0.144.1** (isolated scratch `CODEX_HOME`, self-authored
stdin-dumping hooks) recorded real `PostToolUse` payloads; the sanitized
captures are committed as
`crates/rb-hooks/tests/fixtures/codex/post_tool_use_apply_patch.json` and
`post_tool_use_apply_patch_multifile.json`. The shape also matches the
`rust-v0.144.1` upstream sources (`post-tool-use.command.input.schema.json`;
`handlers/apply_patch.rs` builds `tool_input: {"command": <patch>}`).
`tool_response` is a plain string ("Exit code: 0 … Success. Updated the
following files: A <path>"), so the object-shaped `is_error` failure
detection simply never fires for it (conservative, by design).

What landed (all four recipe steps below, plus review-round hardening; the
governing PRD is `docs/prds/2026-06-23-codex-apply-patch-capture.md`):

1. The `"apply_patch" => "Edit"` arm already existed (landed for OpenCode);
   its Codex path is now live-reachable and covered by un-gated tests.
2. **Multi-file finding:** one `apply_patch` call can touch SEVERAL files —
   the live capture's second event adds two files in a single patch.
   `edited_paths`/`v4a_patch_paths` now record **one observation per
   directive** (`*** Add|Update|Delete File:` plus the `*** Move to:` rename
   destination — a rename records both source and destination, per PRD AP3),
   in patch order, deduplicated, exactly as separate Edit events would; a
   malformed/non-V4A payload still fails open to one `"unknown"` file touch.
3. **Hunk-aware, path-vetted parsing (review round):** directives are
   recognized at column 0 only and never inside `@@` hunk bodies, so patch
   CONTENT that looks like a directive (a context/added/removed line reading
   `*** Add File: /etc/cron.d/evil`) can no longer register phantom touched
   files in summaries/anchors. Paths are vetted per PRD AP3: leading `./`
   stripped (matching `rb_types::normalize_anchor_value`), empty, absolute,
   and `..`-traversal paths rejected (V4A paths are relative-only by spec).
   A stray `*** Move to:` not immediately following an `*** Update File:`
   header is ignored. One `apply_patch` event now batches all its scratch
   appends into a single write round (`Scratch::append_many`).
4. Capture tests cover the real Codex shape
   (`codex_apply_patch_command_field_captures_path`,
   `codex_apply_patch_multi_file_patch_captures_every_touched_path`,
   `apply_patch_malformed_payload_fails_open_to_unknown`, the hunk-poisoning
   and Move/normalization suites, the AP5 no-content-leak test) and the
   binary-level e2e replays both recorded fixtures
   (`codex_apply_patch_post_tool_use_captures_every_edited_file`).
   `crates/rb-agents/tests/cross_adapter.rs` asserts the real `apply_patch`
   name for Codex instead of the `"Write"` placeholder.

Codex `capture` stays **Partial** in the capability matrix: the scratch now
records `apply_patch` edits, but it never folds into a summary until the
Codex `Stop` terminus mapping is fixture-verified (tracked in
`docs/plans/2026-06-26-cross-cli-terminus-mapping.md`).

## Summary

> **Update 2026-07-02:** the `"apply_patch" => "Edit"` arm and V4A path extraction
> (`edited_path`, which checks both `patchText` and `command`) now **exist** in
> `normalize_tool`, landed for OpenCode (which fires the event today). Codex is
> still **upstream-blocked from firing** the event at all (openai/codex#16732), so
> the arm is simply unreached for Codex. The remaining Codex-specific risk is the
> unverified **payload shape**: if Codex carries the patch under a field other
> than `patchText`/`command`, or in a non-V4A format, `edited_path` falls back to
> `"unknown"` (still captured as an unidentified file touch, never dropped).
> Verify against a real Codex fixture before declaring Codex capture restored.
> _(Superseded by the 2026-07-12 resolution above.)_

Codex's file-edit tool `apply_patch` shares its name with OpenCode's, and
`rb_hooks::capture::normalize_tool` now maps `"apply_patch" => "Edit"` (landed for
OpenCode, which fires the event today). Codex itself does not yet emit
PreToolUse/PostToolUse for `apply_patch` — hooks fire only for the shell (Bash)
tool — so for Codex the arm is present but unreached, and this remains deferred
behind openai/codex#16732.

## Verified facts (2026-06-02)

_Snapshot of the pre-arm state; the `apply_patch => Edit` arm + `edited_path`
landed 2026-07-02 (see the update above). The blocker below — Codex not firing
the event — is unchanged._

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
