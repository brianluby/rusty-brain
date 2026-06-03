# rusty-brain — P7: APM Package Distribution — Design Spec

- **Status:** Draft (brainstormed and approved; pending written-spec review)
- **Date:** 2026-06-02
- **Author:** Brian Luby
- **Depends on:** P0–P4 (esp. P4 `rb-mcp`, `rb-hooks`, `rb-install`). Independent of P5/P6.
- **References:** `docs/specs/2026-05-31-rusty-brain-architecture-design.md` (§12 interfaces/ContractVersion, §14 security), P4 (`rb-install`/`rb-hooks` multi-CLI installer). APM docs: <https://microsoft.github.io/apm/>, <https://github.com/microsoft/apm>, <https://microsoft.github.io/apm/guides/mcp-servers/>.

---

## 1. Context & Motivation

APM (Microsoft **Agent Package Manager**) is a manifest-driven dependency manager for AI-agent context. A single `apm.yml` declares the instructions, skills, prompts, agents, hooks, plugins, and **MCP servers** a project needs; `apm install` reproduces that exact setup across every detected harness (GitHub Copilot, Claude Code, Cursor, Codex, OpenCode, Gemini, Windsurf), and `apm.lock.yaml` hash-pins the resolved tree the way `package-lock.json` does for npm. `apm install --mcp <name>` wires an MCP server into every client in one step.

This overlaps almost exactly with what P4's `rb-install` does by hand: it edits each harness's config to register rusty-brain. APM does the same job in a **standard, hash-pinned, reproducible, multi-harness** way that users already adopt for the rest of their agent toolchain. rusty-brain already exposes the right surface to plug in: a stdio MCP server (`rusty-brain mcp`), a daemon that auto-starts on first connect (architecture spec §8), and capture hooks (`rb-hooks`).

P7 publishes rusty-brain as a first-class APM package so a user can add project-wide, cross-harness shared memory with one `apm install`, and makes `rb-install` **APM-aware** — delegating to APM when present, falling back to the bespoke installer otherwise. This is distribution/ecosystem work, deliberately separated from P5/P6.

## 2. Goals

1. A maintained **APM package** for rusty-brain: an `apm.yml` declaring the MCP server plus bundled memory **skills / prompts / instructions / hooks**, so `apm install` wires shared memory into all supported harnesses.
2. **`rb-install` APM-awareness:** detect `apm`, and prefer delegating to `apm install --mcp` (with the bundled artifacts) over hand-editing configs; fall back to the existing P4 installer when `apm` is absent.
3. A `rusty-brain apm` CLI to **emit and validate** the manifest/descriptor (so the package stays in lockstep with the binary's actual MCP surface and `ContractVersion`).
4. Hash-pinned, secret-free, fail-open install — no regression to rusty-brain's security model.

## 3. Non-Goals

- **Not reimplementing APM** or hosting a registry. rusty-brain produces a package APM consumes.
- **Not distributing the rusty-brain binary via APM.** APM wires *agent context* (MCP config, skills, prompts), not arbitrary binaries. The binary is installed separately (cargo/Homebrew/release artifact); the package documents and checks this prerequisite.
- No change to the daemon, store, or wire protocol beyond a CLI subcommand and an `rb-install` backend.
- No secrets in `apm.yml` (the Voyage key stays in env/keychain; the manifest uses env interpolation only).

## 4. Locked Decisions

| Decision | Choice | Rationale |
|---|---|---|
| Integration direction | **Publish rusty-brain as an APM package** (distribution-side) | Highest leverage: one `apm install` wires memory into 7 harnesses; reuses the existing MCP server + P4 hooks. |
| MCP entry form | **stdio**: `command: rusty-brain`, `args: ["mcp"]`, `registry: false` | rusty-brain is a local binary; the daemon auto-starts on first MCP connect. No remote endpoint, no secret in the manifest. |
| Binary delivery | **Out of band** (cargo/brew/release); package checks the prerequisite | APM scope is agent context, not binary install; stated honestly. |
| `rb-install` relationship | **APM-aware delegation with fallback** | Prefer the standard tool when available; keep the bespoke installer working where it isn't. Fail-open either way. |
| Manifest authority | **Generated/validated by `rusty-brain apm`** | The package's MCP descriptor and `ContractVersion` stay in sync with the binary, not hand-maintained and drifting. |
| Package contents | **MCP server + memory skills + prompts + instructions + optional capture hooks** | Wiring the server is necessary but not sufficient; agents need the skills/prompts that teach *when* to remember/recall. |

## 5. Package Shape

A versioned APM package lives in-repo (e.g. `apm/` at the repo root, or a dedicated `rusty-brain-apm` repo referenced as `brianluby/rusty-brain`). It contains:

```text
apm/
  apm.yml                      # the manifest (MCP server + bundled artifacts)
  instructions/
    using-memory.md            # when/why to use shared memory (always-on context)
  skills/
    memory/                    # skill: remember/recall/context workflow
  prompts/
    recall-context.prompt.md   # one-shot "load project memory" prompt
  hooks/                       # optional: capture hooks (ported from rb-hooks), fail-open
```

**Manifest (illustrative — exact fields pinned against APM docs at implementation time):**

```yaml
# apm.yml
dependencies:
  mcp:
    - name: rusty-brain
      registry: false
      transport: stdio
      command: rusty-brain
      args: ["mcp"]
  apm:
    # skills/prompts/instructions referenced as virtual subdirectories / files
    rusty-brain-memory:
      git: brianluby/rusty-brain
      path: apm/skills/memory
      ref: v0.1.0           # hash-pinned via apm.lock.yaml
```

APM reference forms this relies on (verified from the docs): `owner/repo`, pinned `owner/repo#v1.0.0`, virtual subdirectory `owner/repo/skills/...`, virtual file `owner/repo/prompts/x.prompt.md`, and the object form `{ git, path, ref, alias }`; MCP entries live under `dependencies.mcp` and support `stdio`/registry/`remote` shapes. `apm.lock.yaml` pins the resolved tree with content hashes.

**Consumption:** in any project, `apm install` (after referencing the package) wires the `rusty-brain` MCP server and installs the skills/prompts/instructions into every detected harness; the daemon auto-starts on first MCP connect and resolves the project namespace from git root / `CLAUDE.md` as today. No rusty-brain-specific namespace machinery is needed.

## 6. `rusty-brain apm` CLI

A new subcommand group on the existing binary:

- `rusty-brain apm emit` — print the canonical `apm.yml` MCP descriptor for *this* binary (command/args, `ContractVersion`, recommended skills/prompts), so the published manifest is generated, not hand-written.
- `rusty-brain apm validate [path]` — validate an `apm.yml` against the rusty-brain package's expectations (correct MCP entry, no embedded secrets, version/ref present) and against the APM schema shape; non-zero exit on problems. Used in CI.
- `rusty-brain apm doctor` — check prerequisites: `rusty-brain` on PATH, `apm` present, harnesses detected; report what `apm install` will wire. Read-only.

These are thin, read-only/validation commands — no daemon writes, no config mutation (mutation is APM's job).

## 7. `rb-install` APM-aware backend

`rb-install` gains an `apm` backend selected by capability detection:

1. **Detect `apm`** on PATH (and a project `apm.yml`).
2. **If present:** delegate — ensure the rusty-brain MCP entry + bundled artifacts are declared, then invoke `apm install` (or `apm install --mcp rusty-brain ...`). APM performs the hash-pinned, multi-harness wiring.
3. **If absent:** fall back to the existing P4 direct-config installer (Claude Code / Gemini / Codex / …).

**Fail-open** is preserved end to end (architecture spec §6, P4 rule): a delegation or detection failure logs and falls back or no-ops; it never breaks the user's harness setup. Subprocess spawning for `apm` follows the security rule — `env_clear()` then set only the needed vars (global security policy), never inherit the parent environment wholesale.

## 8. Security

- **No secrets in the manifest.** The Voyage API key stays in env/OS keychain (architecture spec §14); `apm.yml` uses env interpolation only (APM supports `$TOKEN`-style values) and CI's `rusty-brain apm validate` rejects any literal secret.
- **Hash-pinning** via `apm.lock.yaml` gives supply-chain integrity on the bundled skills/prompts/hooks — an improvement over hand-copied config.
- **Local transport unchanged.** The MCP server is local stdio; the daemon's 0600 UDS and server-side namespace isolation are untouched. No network surface is added.
- **Capture hooks stay fail-open** (P4 rule): a hook installed via the package must never block or break an agent session.
- **TOCTOU/PATH:** binary-prerequisite and `apm` detection re-check immediately before use; PATH detection requires an executable bit (carried forward from the P4 `rb-install` hardening).

## 9. Testing Strategy

- `rusty-brain apm emit`/`validate` unit + integration tests: emitted descriptor round-trips through `validate`; `validate` rejects embedded secrets, missing `ref`, and malformed MCP entries.
- `rb-install` APM backend: in-process tests with a **fake `apm`** on PATH (a stub script) asserting correct delegation arguments; fallback test when `apm` is absent asserting the P4 installer runs; fail-open test (stub `apm` exits non-zero → fallback/no-op, session unbroken).
- Manifest fixture test: the committed `apm/apm.yml` parses, declares the stdio `rusty-brain` MCP entry, and contains no literal secret.
- A real `apm install` smoke test is `#[ignore]` (requires the `apm` binary + network) — run manually, not in CI.
- Per-phase gate: `cargo test --workspace`; `cargo clippy --workspace --all-targets --all-features -- -D warnings`; `cargo fmt --all --check`; `cargo deny check`. No new default runtime deps expected (CLI/validation reuse `clap`/`serde`/`toml`; subprocess via `std`).

## 10. Risks & Mitigations

| Risk | Mitigation |
|---|---|
| APM schema/CLI drift (fast-moving project) | Pin exact fields against the docs at implementation; `rusty-brain apm validate` centralizes the schema assumptions; smoke test catches breakage. |
| Users expect `apm install` to install the binary too | `apm doctor` + package instructions state the binary prerequisite explicitly; clear error if `rusty-brain` is not on PATH. |
| Delegation path breaks an existing harness setup | Fail-open with fallback to the P4 installer; never mutate config directly in the APM path. |
| Secret leakage into a committed `apm.yml` | `validate` rejects literal secrets in CI; env interpolation only. |
| Divergence between published manifest and binary surface | Manifest is *generated* by `rusty-brain apm emit`; CI regenerates and diffs. |

## 11. Traceability

| Driver | P7 feature |
|---|---|
| APM wires MCP servers across 7 harnesses in one step | stdio `rusty-brain` MCP package entry |
| APM installs skills/prompts/instructions/hooks | bundled memory skills/prompts/instructions/(hooks) |
| P4 `rb-install` hand-edits each harness config | APM-aware delegation with P4 fallback |
| `ContractVersion` drift risk (architecture spec §12) | `rusty-brain apm emit`/`validate` keep the manifest in sync |
| Security model (architecture spec §14; global secret/subprocess rules) | no secrets in manifest; hash-pinning; `env_clear()`; fail-open hooks |
