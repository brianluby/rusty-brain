# PRD: HTTP/REST Surface and Agent-Agnostic Prompt-Time Recall

## Status

Delivered 2026-07-11 (PR #62, merge `e4ca88e`). The optional listener is
default-off and loopback-only, reuses the daemon's wire requests/responses,
and exposes shortcut routes plus generic `POST /ops`. The transport-generic
`rb_proto::Client<S>` and the shared prompt-time recall contract are shipped;
the capability matrix records unsupported adapters rather than implying
parity. HTTP/UDS agreement and HTTP security behavior are covered by
`crates/rb-daemon/tests/http_e2e.rs`.

The delivered security posture is deliberately stricter than this draft:
non-loopback binds are refused outright (there is no override), HTTP callers
are always non-admin because TCP has no kernel peer credential, and streaming
`subscribe` is rejected. Every route requires an explicit namespace header;
Host/Origin, JSON media type, body size, deadlines, and connection count are
bounded. HTTP v1 remains a same-machine convenience surface, not an auth
boundary or multi-host API. Native prompt-time injection remains Claude-only;
other adapters are explicitly unsupported or may call `/recall` from their own
tooling.

## Owner Area

Primary: transport, server listeners, and the prompt-time injection seam.

Touchpoints:

- `crates/rb-daemon/src/http.rs` (HTTP listener and request hygiene)
- `crates/rb-daemon/src/server.rs` (shared dispatch; UDS listener)
- `crates/rb-proto/src/client.rs`, `crates/rb-proto/src/codec.rs`
  (stream-generic framing - the W5a.3 transport-genericization, pulled
  forward)
- `crates/rb-mcp/src/proxy.rs`
- `crates/rb-agents/src/event.rs` (`UserPromptSubmit` abstraction)
- `crates/rb-agents/src/capability.rs` (`retrieval` capability)
- `crates/rb-hooks/src/capture.rs` (`capture::user_prompt_submit`)
- `docs/prds/2026-06-23-cross-agentic-agent-parity.md` (CA6 prompt-time
  retrieval)
- `docs/THREAT_MODEL.md` (network surface - must be updated)

## Problem

Two coupled gaps:

1. **Transport lock-in.** The daemon speaks Unix-domain-socket framing only.
   Anything that is not the CLI, an MCP stdio client, or a hook cannot reach
   memory - no dashboards, no scripts in other languages, no non-MCP agents,
   no remote/SSH use. The "substrate" promise is undercut.
2. **Claude-specific recall.** The W3.2 deterministic prompt-time injection
   depends on Claude's `UserPromptSubmit`. Other agents either lack it or
   model it differently, so recall-before-work stays Claude-only instead of
   being a product invariant.

## Goals

- An optional local HTTP/REST listener (`rusty-brain serve --http`) exposing
  the same non-admin request shapes as the CLI/MCP, for any local client.
- An agent-agnostic "recall-before-work" abstraction so prompt-time retrieval
  is a capability, not a Claude implementation detail.
- Default-off, opt-in, loopback-only, with clear security framing; the
  single-machine/per-user posture is preserved.

## Non-Goals

- Do not replace MCP stdio (it stays the primary agent surface).
- Do not build team-mode auth (that is Phase 5a/W5a.1); HTTP v1 is loopback
  only and is not a multi-host surface.
- Do not expose admin operations over HTTP; TCP callers have no kernel-verified
  peer credential and are always treated as non-admin.
- Do not change ranking or the response shape.

## Functional Requirements

### HTTP-1. HTTP listener (opt-in, loopback)

- `serve --http [bind]` (default `127.0.0.1:0` or a configured port) starts
  an HTTP listener alongside (or instead of) the UDS listener.
- REST endpoints mirror non-admin CLI/MCP operations: `POST /remember`, `POST
  /recall`, `GET /memories/:id`, `GET /context`, `POST /feedback`, etc.,
  over JSON. The existing `Response` types serialize directly.
- Default-off; enabling it is explicit. A config knob (`[http] enable`,
  `bind`) under the same precedence rules as other knobs; secrets stay
  env-only.

### HTTP-2. Security posture (v1: loopback-only, non-admin)

- Refuse every non-loopback bind; v1 has no override or warning-only mode.
- Treat every HTTP caller as non-admin because TCP supplies no kernel-verified
  peer credential. Reject `RunJob`, `Reembed`, `Scrub`, and `NamespaceRename`.
- The threat model is updated: HTTP is a new network surface; v1 is
  same-machine and explicitly not an auth or same-user boundary.

### HTTP-3. Transport genericization (pulled forward from W5a.3)

- Generalize the codec/client to `Client<S>` over any async stream
  (`UnixStream` / `TcpStream`), reusing the existing length-delimited JSON
  framing. This is the W5a.3 seam pulled forward; it must not regress the
  UDS path (agreement tests pin both).

### HTTP-4. Agent-agnostic prompt-time recall

- Promote `recall-before-work` to a capability in
  `rb-agents::capability` (`retrieval`: supported/partial/unsupported).
- Define an agent-agnostic injection contract (top-k under the token budget,
  the W2.5 untrusted-data frame, the W3.3 source-aware rules) independent of
  any one agent's event name.
- Per-adapter, map the agent's closest event to that contract (Claude's
  `UserPromptSubmit`; Codex/OpenCode equivalents where they exist; documented
  unsupported where they do not). Capability matrix (CA6) records the truth.

## Acceptance Criteria

- `serve --http` responds to `POST /recall` with the same ranked results as
  the CLI, over loopback.
- A non-loopback bind is refused with no override.
- Admin operations are always rejected over HTTP.
- An agent without `UserPromptSubmit` can still get prompt-time recall via
  its mapped equivalent, or the capability matrix records `unsupported`
  (never silent parity).
- UDS path is byte-for-byte unaffected (agreement test).

## Verification

```bash
cargo test -p rb-daemon
cargo test -p rb-proto
cargo test -p rb-mcp
cargo test -p rb-agents
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
```

Plus an HTTP e2e hitting `/recall` and `/feedback`, and a non-admin HTTP test
asserting admin rejection.

## Risks

- New attack surface. Mitigate: default-off, loopback-only binding,
  unconditionally denied admin operations, and a threat-model update; no
  multi-host auth in v1.
- Transport refactor regresses UDS. Mitigate: pull W5a.3's `Client<S>`
  genericization with agreement tests on both transports.
- Agent recall abstraction too broad. Mitigate: narrow contract (top-k,
  budget, framing) and fixture-gated per-agent mapping (no invented events).

## Implementation Checklist

- [x] Generalize codec/client to `Client<S>` (W5a.3 seam).
- [x] Add the opt-in HTTP listener, shortcut routes, and generic `/ops`.
- [x] Enforce loopback-only binding and always deny admin operations over HTTP
      (stricter than the draft's non-loopback opt-in/peer-cred proposal).
- [x] Promote recall-before-work to an agent capability and shared contract.
- [x] Map verified adapter events and record unsupported mappings explicitly in
      the capability matrix.
- [x] Update the threat model; add HTTP/UDS agreement and adversarial HTTP
      security tests.

## Roadmap Fit

Realizes the "substrate, not orchestrator" promise and expands TAM (PRD
review, Tier 2). Pulls W5a.3 transport genericization forward safely and
delivers the cross-agent parity PRD's CA6 as a product invariant rather than
adapter plumbing.
