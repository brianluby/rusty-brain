//! Write-path gate vocabulary (Vikunja #69): the channel a write arrived on,
//! the per-namespace policy that validates writes pre-insert, and the
//! structured rejection the daemon surfaces when a write fails the gate.
//!
//! Trust posture (see docs/THREAT_MODEL.md, "The write-path gate"):
//! - [`WriteChannel`] is stamped BY THE DAEMON from the connection's
//!   handshake identity (the loopback listener for HTTP). It is NEVER read
//!   from a per-REQUEST payload — request fields cannot move the tag.
//!   Honesty note (PR #89 review): over the UDS the kernel credential
//!   verifies the peer's UID, not its executable — the surface string is
//!   the honest first-party client's self-report, so the tag separates
//!   honest first-party paths and degrades unknown/old clients to `None`.
//!   It is NOT a defense against a hostile same-user process, which the
//!   threat model already treats as inside the boundary (same-user
//!   privilege is the boundary the admin gate uses too).
//! - Rows whose stamped channel is untrusted ([`WriteChannel::Http`], the
//!   surface with no UDS peer credential at all) are QUARANTINED at
//!   retrieval: excluded from recall/injection, still visible on list/get
//!   surfaces with an explicit marker. Quarantine — not rank demotion — is
//!   the documented choice: a poisoned row must not ride into a prompt.
//! - `origin_channel == None` means one of (a) a pre-migration row (the
//!   004 no-backfill precedent applies here too), (b) a daemon-internal
//!   write (jobs), or (c) a handshake surface the daemon does not
//!   recognize as a first-party writer. None of those carries Http's
//!   network-origin risk, so they keep today's retrieval behavior.

use crate::memory_type::MemoryType;
use crate::namespace::Namespace;
use serde::{Deserialize, Serialize};

/// Default content-size ceiling applied by the gate when no `[write_gate]`
/// config exists: 256 KiB. Content larger than this is rejected pre-insert
/// (today's build accepts it up to the 1 MiB frame bound — this default is
/// the gate's always-on floor, independent of configuration).
pub const DEFAULT_MAX_CONTENT_BYTES: usize = 256 * 1024;

/// Default context-size ceiling applied by the gate when no `[write_gate]`
/// config exists: 64 KiB.
pub const DEFAULT_MAX_CONTEXT_BYTES: usize = 64 * 1024;

/// The surface a write arrived on (Vikunja #69). Db strings are in lockstep
/// with the `origin_channel` SQL CHECK (migration 013), like `MemoryType`.
///
/// `job` is deliberately NOT a channel: daemon-internal job writes are not
/// client requests and carry no client-context stamp (their rows keep
/// `origin_source = "job"` advisory provenance and `origin_channel = NULL`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WriteChannel {
    Hook,
    Cli,
    Mcp,
    Http,
}

impl WriteChannel {
    /// Every variant, in declaration order.
    pub fn all() -> [WriteChannel; 4] {
        [
            WriteChannel::Hook,
            WriteChannel::Cli,
            WriteChannel::Mcp,
            WriteChannel::Http,
        ]
    }

    /// Stable db string. MUST stay in lockstep with the SQL CHECK constraint
    /// (migration 013) — same discipline as `MemoryType::as_str`.
    pub fn as_str(&self) -> &'static str {
        match self {
            WriteChannel::Hook => "hook",
            WriteChannel::Cli => "cli",
            WriteChannel::Mcp => "mcp",
            WriteChannel::Http => "http",
        }
    }

    /// Fail-closed parse of a db/config string (the `MemoryType::parse`
    /// discipline): an unknown value is an error, never a guess.
    pub fn parse(s: &str) -> crate::error::Result<Self> {
        match s {
            "hook" => Ok(WriteChannel::Hook),
            "cli" => Ok(WriteChannel::Cli),
            "mcp" => Ok(WriteChannel::Mcp),
            "http" => Ok(WriteChannel::Http),
            other => Err(crate::error::Error::InvalidArgument(format!(
                "unknown write channel '{other}': expected one of hook, cli, mcp, http"
            ))),
        }
    }

    /// How an UNVERIFIED stamp (`None`) is named in messages: honest about
    /// what it is (the daemon could not verify a first-party executable).
    pub const UNVERIFIED_LABEL: &'static str = "unverified";
}

/// Per-namespace write policy enforced pre-insert by the daemon (Vikunja #69):
/// which types may be stored, how large content/context may be, whether
/// anchors are required, and which stamped channels may write at all.
///
/// Defaults keep a working system (all types, all channels, generous size
/// caps) while still bounding every write: the size ceilings apply even with
/// NO configuration, which is a behavior change the gate's tests pin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteGatePolicy {
    /// Memory types this namespace accepts. Default: all.
    pub allowed_types: Vec<MemoryType>,
    /// Maximum `content` length in bytes. Default: 256 KiB.
    pub max_content_bytes: usize,
    /// Maximum `context` length in bytes. Default: 64 KiB.
    pub max_context_bytes: usize,
    /// Minimum anchor count a write must carry (0 = no requirement).
    /// Default: 0.
    pub min_anchors: usize,
    /// Stamped channels allowed to write. Default: all four.
    pub permitted_channels: Vec<WriteChannel>,
    /// Whether an unverified-stamp write (daemon could not verify the peer
    /// executable) is accepted. Default: true — the single-user threat model
    /// treats same-user processes as the principal; opt out per namespace to
    /// require a positively verified first-party binary.
    pub allow_unverified_channel: bool,
}

impl Default for WriteGatePolicy {
    fn default() -> Self {
        Self {
            allowed_types: MemoryType::all().to_vec(),
            max_content_bytes: DEFAULT_MAX_CONTENT_BYTES,
            max_context_bytes: DEFAULT_MAX_CONTEXT_BYTES,
            min_anchors: 0,
            permitted_channels: WriteChannel::all().to_vec(),
            allow_unverified_channel: true,
        }
    }
}

impl WriteGatePolicy {
    /// Whether `channel` (a daemon-stamped channel, or `None` for an
    /// unverified stamp) may write under this policy.
    #[must_use]
    pub fn channel_permitted(&self, channel: Option<WriteChannel>) -> bool {
        match channel {
            Some(c) => self.permitted_channels.contains(&c),
            None => self.allow_unverified_channel,
        }
    }

    /// Whether `memory_type` is storable under this policy.
    #[must_use]
    pub fn type_allowed(&self, memory_type: &MemoryType) -> bool {
        self.allowed_types.contains(memory_type)
    }
}

/// The resolved write-gate configuration the daemon enforces: a default
/// policy plus optional per-namespace overrides. Unknown namespaces resolve
/// to the default policy — fail-closed in the sense that the default is
/// explicit and bounded (size ceilings always apply); stricter behavior is
/// opted into per namespace.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WriteGateConfig {
    /// Policy for namespaces without an explicit entry.
    pub default: WriteGatePolicy,
    /// `(namespace, policy)` overrides, first match wins.
    pub namespaces: Vec<(Namespace, WriteGatePolicy)>,
}

impl WriteGateConfig {
    /// The policy that governs `namespace`: the first explicit entry, else
    /// the default. Never fails and never silently widens — an unlisted
    /// namespace gets the bounded default, not an allow-all escape hatch.
    #[must_use]
    pub fn policy_for(&self, namespace: &Namespace) -> &WriteGatePolicy {
        self.namespaces
            .iter()
            .find(|(ns, _)| ns == namespace)
            .map(|(_, policy)| policy)
            .unwrap_or(&self.default)
    }
}

/// One structured, testable pre-insert rejection (Vikunja #69). The daemon
/// maps this to a validation-class wire error (message travels verbatim) so
/// clients see the exact reason; `code()` is the stable machine-checkable
/// identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteRejection {
    /// The stamped channel (or unverified stamp) may not write here.
    ChannelNotPermitted {
        namespace: Namespace,
        channel: Option<WriteChannel>,
        permitted: Vec<WriteChannel>,
        unverified_allowed: bool,
    },
    /// `memory_type` is not in the namespace's allowed set.
    TypeNotAllowed {
        namespace: Namespace,
        memory_type: MemoryType,
        allowed: Vec<MemoryType>,
    },
    /// `content` exceeds the size ceiling.
    ContentOversized {
        namespace: Namespace,
        bytes: usize,
        max_bytes: usize,
    },
    /// `context` exceeds the size ceiling.
    ContextOversized {
        namespace: Namespace,
        bytes: usize,
        max_bytes: usize,
    },
    /// The write carries fewer anchors than required.
    MissingAnchors {
        namespace: Namespace,
        found: usize,
        required: usize,
    },
}

impl WriteRejection {
    /// Stable machine-checkable code for this rejection class.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            WriteRejection::ChannelNotPermitted { .. } => "channel_not_permitted",
            WriteRejection::TypeNotAllowed { .. } => "type_not_allowed",
            WriteRejection::ContentOversized { .. } => "content_oversized",
            WriteRejection::ContextOversized { .. } => "context_oversized",
            WriteRejection::MissingAnchors { .. } => "anchors_required",
        }
    }

    /// Wire message: `[write-gate:<code>] <guidance>`. Validation errors
    /// travel verbatim by the wire-error hygiene rule — the message IS the
    /// guidance.
    fn message(&self) -> String {
        match self {
            WriteRejection::ChannelNotPermitted {
                namespace,
                channel,
                permitted,
                unverified_allowed,
            } => {
                let channel = channel
                    .map(|c| c.as_str().to_string())
                    .unwrap_or_else(|| WriteChannel::UNVERIFIED_LABEL.to_string());
                let permitted: Vec<&str> = permitted.iter().map(WriteChannel::as_str).collect();
                format!(
                    "write on channel '{channel}' is not permitted for namespace '{}' \
                     (permitted: {}; unverified peers {})",
                    namespace.as_db_string(),
                    permitted.join(", "),
                    if *unverified_allowed {
                        "allowed"
                    } else {
                        "rejected"
                    },
                )
            }
            WriteRejection::TypeNotAllowed {
                namespace,
                memory_type,
                allowed,
            } => {
                let allowed: Vec<&str> = allowed.iter().map(MemoryType::as_str).collect();
                format!(
                    "memory type '{}' is not allowed for namespace '{}' (allowed: {})",
                    memory_type.as_str(),
                    namespace.as_db_string(),
                    allowed.join(", "),
                )
            }
            WriteRejection::ContentOversized {
                namespace,
                bytes,
                max_bytes,
            } => format!(
                "content is {bytes} bytes; namespace '{}' allows at most {max_bytes}",
                namespace.as_db_string(),
            ),
            WriteRejection::ContextOversized {
                namespace,
                bytes,
                max_bytes,
            } => format!(
                "context is {bytes} bytes; namespace '{}' allows at most {max_bytes}",
                namespace.as_db_string(),
            ),
            WriteRejection::MissingAnchors {
                namespace,
                found,
                required,
            } => format!(
                "write carries {found} anchors; namespace '{}' requires at least {required}",
                namespace.as_db_string(),
            ),
        }
    }
}

impl std::fmt::Display for WriteRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[write-gate:{}] {}", self.code(), self.message())
    }
}

impl std::error::Error for WriteRejection {}

/// Validate one write against `policy` BEFORE anything reaches the store
/// (Vikunja #69). `channel` is the daemon-stamped channel (`None` = the
/// peer executable could not be verified). Checks, in order: permitted
/// channel, allowed type, content size, context size, anchor minimum. First
/// failure wins and is returned as the structured rejection.
pub fn validate_write(
    policy: &WriteGatePolicy,
    namespace: &Namespace,
    channel: Option<WriteChannel>,
    memory_type: &MemoryType,
    content: &str,
    context: Option<&str>,
    anchor_count: usize,
) -> Result<(), WriteRejection> {
    if !policy.channel_permitted(channel) {
        return Err(WriteRejection::ChannelNotPermitted {
            namespace: namespace.clone(),
            channel,
            permitted: policy.permitted_channels.clone(),
            unverified_allowed: policy.allow_unverified_channel,
        });
    }
    if !policy.type_allowed(memory_type) {
        return Err(WriteRejection::TypeNotAllowed {
            namespace: namespace.clone(),
            memory_type: *memory_type,
            allowed: policy.allowed_types.clone(),
        });
    }
    let bytes = content.len();
    if bytes > policy.max_content_bytes {
        return Err(WriteRejection::ContentOversized {
            namespace: namespace.clone(),
            bytes,
            max_bytes: policy.max_content_bytes,
        });
    }
    if let Some(ctx) = context {
        let bytes = ctx.len();
        if bytes > policy.max_context_bytes {
            return Err(WriteRejection::ContextOversized {
                namespace: namespace.clone(),
                bytes,
                max_bytes: policy.max_context_bytes,
            });
        }
    }
    if anchor_count < policy.min_anchors {
        return Err(WriteRejection::MissingAnchors {
            namespace: namespace.clone(),
            found: anchor_count,
            required: policy.min_anchors,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn ns() -> Namespace {
        Namespace::parse_db_string("project:gate").unwrap()
    }

    #[test]
    fn channel_strings_round_trip_and_unknown_fails_closed() {
        for c in WriteChannel::all() {
            assert_eq!(WriteChannel::parse(c.as_str()).unwrap(), c);
        }
        assert!(WriteChannel::parse("job").is_err());
        assert!(WriteChannel::parse("").is_err());
        assert!(WriteChannel::parse("HOOK").is_err());
    }

    #[test]
    fn default_policy_accepts_today_traffic() {
        // The default must keep every first-party write path working…
        for c in WriteChannel::all() {
            assert!(WriteGatePolicy::default().channel_permitted(Some(c)));
        }
        // …including unverified stamps (legacy/test binaries over UDS).
        assert!(WriteGatePolicy::default().channel_permitted(None));
        for t in MemoryType::all() {
            assert!(WriteGatePolicy::default().type_allowed(&t));
        }
    }

    #[test]
    fn channel_permitted_respects_lists_and_unverified_flag() {
        let policy = WriteGatePolicy {
            permitted_channels: vec![WriteChannel::Hook, WriteChannel::Cli],
            ..WriteGatePolicy::default()
        };
        assert!(policy.channel_permitted(Some(WriteChannel::Hook)));
        assert!(!policy.channel_permitted(Some(WriteChannel::Http)));
        // Unverified still allowed unless explicitly turned off.
        assert!(policy.channel_permitted(None));
        let strict_unverified = WriteGatePolicy {
            allow_unverified_channel: false,
            ..policy
        };
        assert!(!strict_unverified.channel_permitted(None));
    }

    #[test]
    fn validate_write_rejects_each_dimension_with_structured_error() {
        let ns = ns();
        let default = WriteGatePolicy::default();

        // Oversized content — the always-on default ceiling.
        let big = "x".repeat(DEFAULT_MAX_CONTENT_BYTES + 1);
        let err = validate_write(
            &default,
            &ns,
            Some(WriteChannel::Cli),
            &MemoryType::Insight,
            &big,
            None,
            0,
        )
        .unwrap_err();
        assert!(matches!(err, WriteRejection::ContentOversized { .. }));
        assert_eq!(err.code(), "content_oversized");
        assert!(err
            .to_string()
            .starts_with("[write-gate:content_oversized]"));
        assert!(err.to_string().contains("project:gate"));

        // Oversized context.
        let big_ctx = "y".repeat(DEFAULT_MAX_CONTEXT_BYTES + 1);
        let err = validate_write(
            &default,
            &ns,
            None,
            &MemoryType::Insight,
            "ok",
            Some(&big_ctx),
            0,
        )
        .unwrap_err();
        assert_eq!(err.code(), "context_oversized");

        // Type not allowed.
        let strict = WriteGatePolicy {
            allowed_types: vec![MemoryType::Insight],
            ..WriteGatePolicy::default()
        };
        let err = validate_write(
            &strict,
            &ns,
            Some(WriteChannel::Hook),
            &MemoryType::Preference,
            "ok",
            None,
            0,
        )
        .unwrap_err();
        assert_eq!(err.code(), "type_not_allowed");

        // Channel not permitted (http excluded).
        let no_http = WriteGatePolicy {
            permitted_channels: vec![WriteChannel::Hook, WriteChannel::Cli, WriteChannel::Mcp],
            ..WriteGatePolicy::default()
        };
        let err = validate_write(
            &no_http,
            &ns,
            Some(WriteChannel::Http),
            &MemoryType::Insight,
            "ok",
            None,
            0,
        )
        .unwrap_err();
        assert_eq!(err.code(), "channel_not_permitted");
        assert!(err.to_string().contains("'http'"));

        // Unverified rejected when disallowed.
        let mut verified_only = no_http.clone();
        verified_only.allow_unverified_channel = false;
        let err = validate_write(
            &verified_only,
            &ns,
            None,
            &MemoryType::Insight,
            "ok",
            None,
            0,
        )
        .unwrap_err();
        assert_eq!(err.code(), "channel_not_permitted");
        assert!(err.to_string().contains(WriteChannel::UNVERIFIED_LABEL));

        // Anchor minimum.
        let anchored = WriteGatePolicy {
            min_anchors: 1,
            ..WriteGatePolicy::default()
        };
        let err = validate_write(
            &anchored,
            &ns,
            Some(WriteChannel::Hook),
            &MemoryType::Insight,
            "ok",
            None,
            0,
        )
        .unwrap_err();
        assert_eq!(err.code(), "anchors_required");
        assert!(validate_write(
            &anchored,
            &ns,
            Some(WriteChannel::Hook),
            &MemoryType::Insight,
            "ok",
            None,
            1
        )
        .is_ok());

        // A plain conforming write passes the default policy.
        assert!(validate_write(
            &default,
            &ns,
            Some(WriteChannel::Hook),
            &MemoryType::Insight,
            "session summary",
            Some("ctx"),
            2
        )
        .is_ok());
    }

    #[test]
    fn policy_for_returns_override_then_default() {
        let ns = ns();
        let strict = WriteGatePolicy {
            max_content_bytes: 10,
            ..WriteGatePolicy::default()
        };
        let config = WriteGateConfig {
            default: WriteGatePolicy::default(),
            namespaces: vec![(ns.clone(), strict.clone())],
        };
        assert_eq!(config.policy_for(&ns), &strict);
        // Unknown namespace: the bounded default, never an escape hatch.
        let other = Namespace::parse_db_string("global").unwrap();
        assert_eq!(config.policy_for(&other), &WriteGatePolicy::default());
    }
}
