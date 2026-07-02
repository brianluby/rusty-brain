# PRD: Native Windows Support

## Status

Draft, deliberately deferred. From the 2026-07-02 senior-PM product review.
Native Windows is currently an explicit Road-to-Tens non-goal (SS15: WSL2 is
the documented path). This PRD records the *conditions* under which the
non-goal should be revisited, so the decision is tracked rather than
forgotten.

## Owner Area

Primary: transport, paths, and CI matrix.

Touchpoints:

- `crates/rb-daemon/src/server.rs` (UDS listener)
- `crates/rb-proto/src/client.rs` (UDS client)
- `crates/rb-config/src/file.rs`, `crates/rusty-brain/src/paths.rs`
- `crates/rb-hooks/src/io.rs`
- `.github/workflows/*` (CI matrix)
- `docs/ARCHITECTURE.md`, `docs/THREAT_MODEL.md` (platform policy)
- `docs/prds/2026-07-02-http-surface-and-agent-agnostic-recall.md`
  (HTTP removes the UDS blocker)

## Problem

The daemon and hooks depend on Unix-domain sockets and Unix file-mode
semantics, so Windows users are limited to WSL2. WSL2 works but adds
friction (install, file-system perf across the boundary) and excludes users
who cannot or will not use WSL2. As TAM grows, Windows is the largest
unsupported pool.

## Goals

- Define a falsifiable trigger for promoting Windows to a supported tier.
- Land the prerequisites cheaply as side effects of other work, so the
  eventual lift is small.
- Keep the decision explicit and revisitable, not silently deferred.

## Non-Goals

- Do not implement Windows support now; this is a gating/decision PRD.
- Do not drop WSL2 support (it remains a documented path).
- Do not change the platform policy until the trigger conditions are met.

## Functional Requirements

### WIN-1. Trigger conditions (all must hold)

Native Windows becomes a justified investment only when ALL are true:

1. The HTTP/REST surface (PRD 7) lands, removing the hard UDS dependency for
   the primary client path. (UDS is not available on Windows in the same
   form; named pipes are the local alternative but HTTP is transport-agnostic
   and already needed for other reasons.)
2. A measured Windows demand signal exists (issues, pilots, or the Phase 5
   pilot requesting it).
3. The path/mode abstractions are platform-parametric (WIN-2).

### WIN-2. Prerequisite refactor (land incrementally, no Windows target yet)

- Abstract the listener behind a `Listener` trait with UDS and (later)
  named-pipe/TCP impls - a side effect of PRD 7's `Client<S>`
  genericization.
- Replace Unix file-mode assumptions (0600/0700) with a permissions
  abstraction that enforces the *intent* (owner-private) per-platform; on
  Windows this maps to ACLs, documented honestly.
- Make all path resolution `PathBuf`-clean and test on a Windows runner in CI
  (green build is the floor, not "supported").

### WIN-3. CI matrix floor

Add a Windows runner to CI that builds the workspace and runs
platform-agnostic tests (green build + tests). This surfaces portability
debt early without claiming support; failures are allowed-to-fail with
alerting until the trigger (WIN-1) is met.

### WIN-4. Documentation honesty

Until the trigger holds, `ARCHITECTURE.md`/README state Windows is
unsupported with WSL2 as the path, and this PRD is the single source for the
revisit conditions. When the trigger holds, promote and update the platform
policy (SS15 closure).

## Acceptance Criteria

- The revisit conditions are recorded and linked from the non-goals section.
- The listener/path abstractions are platform-parametric (UDS path
  unchanged, agreement tests green).
- A Windows CI runner builds the workspace (allowed-to-fail until WIN-1).
- Documentation states the current truth and points here.

## Verification

```bash
cargo build --workspace            # current platforms
cargo test --workspace
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
# CI: windows-latest build + platform-agnostic tests (allowed-to-fail)
```

## Risks

- Premature investment before demand. Mitigate: this PRD implements no
  Windows target, only prerequisites that are valuable regardless.
- Abstracting modes/paths introduces bugs on current platforms. Mitigate:
  agreement tests pin UDS behavior; abstractions are additive.
- "Builds on Windows" is mistaken for "supported." Mitigate: explicit
  documentation; the CI job is allowed-to-fail and labeled as a floor.

## Implementation Checklist

- [ ] Land the `Listener`/`Client<S>` abstraction via PRD 7.
- [ ] Abstract file-mode intent (owner-private) per platform.
- [ ] Add a Windows CI runner (build + platform-agnostic tests,
  allowed-to-fail).
- [ ] Record WIN-1 trigger conditions in the non-goals closure table.
- [ ] Update platform policy docs to point here.

## Roadmap Fit

Keeps the SS15 Windows non-goal *tracked* rather than forgotten, and lands
the cheap prerequisites as side effects of PRD 7 (HTTP/transport). Revisit
only when HTTP removes the UDS blocker and a real demand signal exists -
this changes the TAM math materially at that point.
