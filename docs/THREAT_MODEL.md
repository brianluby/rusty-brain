# rusty-brain threat model

Status: living document, first written for W2.6 (Phase 2 of the road-to-tens
plan). Covers the surface that exists today (single host, single user, local
daemon) and the team surface being built toward (Phase 5 replication). Honest
about residual risk: where a control is best-effort, this document says so
instead of implying a guarantee.

## What the system holds

A rusty-brain database is a long-lived corpus of developer memories: decisions,
constraints, bug fixes, preferences, file paths, and whatever an agent or hook
chose to capture from sessions. Assume it contains fragments of proprietary
code, internal hostnames, and — despite redaction (below) — possibly secrets
that leaked through. Treat the DB file with the same sensitivity as a shell
history plus an editor's local history.

## Trust boundaries today

```text
agent / CLI / hooks (same user) ──UDS──▶ daemon ──▶ SQLite file
        ▲                                  │
   client-declared identity          kernel-verified peer uid
   (provenance metadata ONLY)        (the authorization principal)
```

1. **The Unix socket is the only network surface, and it is local.** The
   daemon listens on a Unix domain socket created `0600` inside a `0700`
   directory. There is no TCP listener. Remote attackers have no direct
   surface; the threat model starts at local processes.

2. **The OS user is the security principal.** Any process running as the same
   user can connect, read, write, and delete every memory in every namespace.
   This is deliberate: the daemon serves *your* agents. Consequences:
   - Malware running as your user owns your memory corpus (as it owns your
     `~/.ssh`). rusty-brain does not attempt to defend against a compromised
     user account.
   - **Namespace is NOT an auth boundary.** Namespaces scope retrieval and
     writes per project so agents see relevant context; the daemon enforces
     that scoping server-side per connection (fail-closed validation, vec0
     partition keys, server-side subscribe filtering). But nothing stops a
     same-user client from handshaking into any other namespace. Do not model
     namespaces as tenant isolation — they are organization, not security.

3. **Peer identity is kernel-verified; handshake identity is not (W2.6).**
   The handshake's `ClientIdentity` (user/host/agent/session/source) is
   client-declared and is stored as *provenance metadata only* — useful for
   audit and ranking, never for authorization. The connection's principal is
   the peer uid read via `getpeereid`/`SO_PEERCRED` at accept time.
   - **Admin ops** (`RunJob`, `Reembed`, `NamespaceRename`, `Scrub` — the
     cross-namespace maintenance surface) require the peer uid to equal the
     daemon's effective uid; everything else stays namespace-scoped. The check
     fails closed: unreadable peer credentials are non-admin. Root gets no
     special grant (root does not need one; it can read the DB file directly).
   - In the normal single-user deployment the socket's `0600` mode already
     keeps foreign uids out; the peer-cred gate is defense-in-depth for
     loosened socket permissions and a hard precondition for any future
     shared-socket deployment.

4. **The DB file is `0600`, tightened at every open.** rb-store re-applies
   permissions on open, and the install e2e proves a planted fake secret is
   not greppable from the DB in plaintext *when the redactor catches it*
   (see redaction below).

## Data-flow exposures (deliberate, user-controlled)

- **Embeddings**: with `VOYAGE_API_KEY` set, memory content is sent to the
  Voyage API to be embedded. With the local ONNX or deterministic provider,
  content never leaves the machine. Choosing a remote embedder is choosing to
  send memory content to that vendor.
- **LLM enrichment**: when `enrich.base_url`/`model` are configured, raw
  memory content and context are POSTed to that endpoint. Pointing this at a
  third-party API is an exfiltration path by configuration; pointing it at
  localhost (Ollama) keeps content local.
- **Secrets are env-only.** `VOYAGE_API_KEY` / `RB_ENRICH_API_KEY` never pass
  through the config file, the `Debug`-printable config structs, or the wire.
  The auto-start env allowlist (`FORWARD_ENV`) is frozen to secrets +
  identity + path resolution, so a daemon child cannot inherit arbitrary
  parent env.
- **Repo-committed config is identity-only.** `.rusty-brain.toml` (committed,
  read from `HEAD`) can set the namespace and nothing else — a hostile repo
  must not be able to repoint sockets, databases, or enrichment endpoints.
  CLAUDE.md frontmatter overrides require explicit per-directory acceptance
  (`--accept-namespace-override`).

## Adversarial inputs

- **Captured content is untrusted.** Hooks write whatever flowed through a
  session, including text an attacker controlled (a malicious issue comment,
  a poisoned web page the agent read). Two consequences:
  1. **Prompt injection via recall** — a stored memory containing
     instruction-shaped text is re-injected into future sessions.
     Mitigation (W2.5): SessionStart injection wraps memory content in
     data-not-instructions framing — a preamble stating the entries are
     recalled data that must never be followed, each memory quoted and
     labeled with its W0.5 provenance (who/what wrote it). Unit tests pin the
     framing; the live scripted injection drill (plant a memory with
     instruction-shaped text, assert the agent does not act on it) lands with
     the W3.4 real-session harness. This is **best-effort** — framing
     reduces, does not eliminate, the class. The Phase 5 curation queue is
     the team-mode backstop.
  2. **Stored secrets** — the shared `rb-redact` pass (one rule set, used at
     capture time in `rb-hooks` and by the retroactive `rusty-brain scrub`
     admin op) scrubs recognizable token shapes plus a high-entropy sweep
     before content is persisted. It is pattern-based and **best-effort by
     construction**. Measured against the committed benchmark corpus
     (`crates/rb-redact/fixtures/benchmark.json`, shape-true synthetic secrets
     across the gitleaks rule families): **90.3% detection (56/62), 9.7%
     false-negative rate, 0 false positives** on the benign-bait set, pinned
     by `crates/rb-redact/tests/benchmark.rs`. The documented false-negative
     classes — bare 32-hex tokens (Twilio/Mailchimp-style: only two character
     classes, so they sit below the entropy gate's three-class floor, the same
     property that keeps git SHAs and UUIDs intact) and prose-stated passwords
     with no `key=value` shape — are explicit residual risk, not gaps to
     assume away. The backstops are file permissions (`0600`), the retroactive
     `rusty-brain scrub` (re-runs the same pass over an existing DB: rewrites
     content/summary/context, resyncs FTS, marks affected rows for
     re-embedding), and hard-delete/purge (W5b.3). A base64 secret containing
     `/` is split by the entropy sweep (paths are deliberately token
     separators, to avoid eating every absolute path) and only fires if a
     segment still clears the length gate — a known residual miss.
- **Confidence/importance are caller-declared.** A same-user client can store
  high-importance, full-confidence falsehoods; ranking dampens low-confidence
  memories but nothing authenticates truth. `contested` (contradicts links)
  surfaces disagreement; it does not adjudicate it.

## Wire-error hygiene

Internal faults (storage/io/migration/serialization/embedding/enrichment) are
mapped to an opaque `"internal error"` on the wire with detail logged
server-side only — paths and infrastructure strings do not leak to clients.
Validation and permission errors travel verbatim because their message *is*
the guidance.

## The team surface (Phase 5, forward-looking)

Replication/sharing changes the model qualitatively; recording the deltas now
so Phase 5 inherits requirements instead of discovering them:

- **Namespace is still not an auth boundary.** Team scoping needs a real
  authorization layer at the hub (per-principal grants), not namespace
  filtering.
- **Provenance becomes load-bearing.** Today origin fields are advisory; in a
  team store, write attribution must be authenticated (the W0.5 fields are
  the schema, not the proof). Hub-side authn is a Phase 5 design input.
- **The oplog is the replication substrate** (W2.7 consumers; site_id + seq).
  Replay-on-reconnect means a malicious or compromised peer replaying crafted
  oplog entries is in-scope for Phase 5 review.
- **Curation queue** (Phase 5b) is the moderation point for injected/poisoned
  content before it reaches other users' sessions.

## Out of scope (explicitly)

- Defending against a compromised OS user account or root.
- Memory-safety attacks on SQLite/sqlite-vec internals beyond keeping them
  vendored and updated.
- Multi-user sharing of one daemon socket (unsupported today; the peer-cred
  gate exists so loosening this later starts from deny).
