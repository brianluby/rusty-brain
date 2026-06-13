//! `rb-redact`: the single shared secret-redaction pass (W2.4).
//!
//! One rule set used by BOTH producers: capture-time redaction in `rb-hooks`
//! (before any text is persisted) and the retroactive `rusty-brain scrub`
//! admin op in the daemon (rewriting rows that predate a rule).
//!
//! Design rules:
//! - **Conservative**: false positives are acceptable; leaked plaintext
//!   secrets are not. Replacements keep a `[REDACTED:kind]` marker so a human
//!   can see something was removed and why.
//! - **Fail closed**: if the rule set cannot compile (a programmer error,
//!   pinned by tests), the whole text is replaced rather than persisted raw.
//! - **Best-effort by construction**: pattern matching cannot catch every
//!   secret shape. The measured false-negative rate against the committed
//!   benchmark corpus (fixtures derived from the gitleaks rule families) is
//!   documented in `docs/THREAT_MODEL.md`; file permissions and purge are the
//!   backstops.

/// `(pattern, replacement)` pairs applied in order to every external text
/// before it is persisted. Order matters: specific, anchored token shapes run
/// before the generic credential-assignment rule so the marker names the kind.
const REDACT_RULES: &[(&str, &str)] = &[
    // PEM private-key blocks, including an unterminated block (e.g. a key cut
    // off mid-response): redact through the END marker or to end-of-text.
    (
        r"(?s)-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----.*?(?:-----END [A-Z0-9 ]*PRIVATE KEY-----|\z)",
        "[REDACTED:private-key]",
    ),
    // AWS access key ids (AKIA = long-lived, ASIA = STS temporary).
    (r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b", "[REDACTED:aws-key]"),
    // GitHub tokens: classic PATs and app tokens (ghp/gho/ghu/ghs/ghr) and
    // fine-grained PATs (github_pat_). Shapes per the gitleaks github rules.
    (
        r"\b(?:ghp|gho|ghu|ghs|ghr)_[0-9A-Za-z]{36,255}\b",
        "[REDACTED:github-token]",
    ),
    (
        r"\bgithub_pat_[0-9A-Za-z_]{82}\b",
        "[REDACTED:github-token]",
    ),
    // GitLab personal access tokens.
    (r"\bglpat-[0-9A-Za-z_\-]{20}\b", "[REDACTED:gitlab-token]"),
    // Slack bot/user/app/legacy tokens and webhook URLs.
    (
        r"\bxox[baprse]-[0-9A-Za-z\-]{10,250}\b",
        "[REDACTED:slack-token]",
    ),
    (
        r"https://hooks\.slack\.com/services/[A-Za-z0-9+/_\-]{8,}",
        "[REDACTED:slack-webhook]",
    ),
    // Stripe live/test secret + restricted keys.
    (
        r"\b[sr]k_(?:live|test)_[0-9A-Za-z]{16,128}\b",
        "[REDACTED:stripe-key]",
    ),
    // Google API keys.
    (r"\bAIza[0-9A-Za-z_\-]{35}\b", "[REDACTED:google-api-key]"),
    // OpenAI / Anthropic API keys.
    (
        r"\bsk-(?:proj-|ant-)?[0-9A-Za-z_\-]{20,250}\b",
        "[REDACTED:model-api-key]",
    ),
    // npm automation tokens.
    (r"\bnpm_[0-9A-Za-z]{36}\b", "[REDACTED:npm-token]"),
    // SendGrid API keys.
    (
        r"\bSG\.[0-9A-Za-z_\-]{22}\.[0-9A-Za-z_\-]{43}\b",
        "[REDACTED:sendgrid-key]",
    ),
    // JWTs (three dot-separated base64url segments, header always `eyJ`).
    (
        r"\beyJ[0-9A-Za-z_\-]{8,}\.[0-9A-Za-z_\-]{8,}\.[0-9A-Za-z_\-]{8,}\b",
        "[REDACTED:jwt]",
    ),
    // URL userinfo passwords (https://user:secret@host/...): keep the user,
    // drop the password.
    (r"(://[^/\s:@]+:)[^@/\s]+@", "${1}[REDACTED:url-password]@"),
    // HTTP Authorization headers (any scheme: Bearer, Basic, token, ...).
    (
        r"(?i)\bauthorization\s*:\s*\S+(?:[ \t]+\S+)?",
        "[REDACTED:authorization]",
    ),
    // Bare bearer tokens outside a header context.
    (r"(?i)\bbearer\s+[A-Za-z0-9._~+/=\-]+", "[REDACTED:bearer]"),
    // key=value / key: value where the key CONTAINS a credential word
    // (covers GITHUB_TOKEN=..., my_api_key: ..., passwd=...), including
    // JSON-quoted keys ({"password":"..."}): the optional closing quote before
    // the separator is matched too, or a serialized object form would smuggle
    // a structured credential past the rule. The key and separator are kept;
    // only the value is replaced.
    (
        r#"(?i)\b([A-Za-z0-9_\-]*(?:password|passwd|secret|token|api[_-]?key|authorization)[A-Za-z0-9_\-]*)(["']?\s*[:=]\s*)("[^"\n]*"|'[^'\n]*'|[^\s"']+)"#,
        "${1}${2}[REDACTED:credential]",
    ),
];

/// Rules compiled once per process. `None` if any pattern fails to compile
/// (a programmer error, pinned by `all_redaction_rules_compile`).
static REDACTIONS: std::sync::OnceLock<Option<Vec<(regex::Regex, &'static str)>>> =
    std::sync::OnceLock::new();

fn redactions() -> Option<&'static [(regex::Regex, &'static str)]> {
    REDACTIONS
        .get_or_init(|| {
            REDACT_RULES
                .iter()
                .map(|(pattern, replacement)| {
                    regex::Regex::new(pattern).ok().map(|re| (re, *replacement))
                })
                .collect()
        })
        .as_deref()
}

/// Replace recognizable secrets in `text` with `[REDACTED:kind]` markers,
/// then sweep remaining high-entropy tokens (W2.4 entropy heuristic).
/// Fail-closed: if the rule set is unavailable, the WHOLE text is replaced —
/// a capture is best-effort and must never persist a known-shape secret.
pub fn redact(text: &str) -> String {
    let Some(rules) = redactions() else {
        return "[REDACTED:unavailable]".to_string();
    };
    let mut out = text.to_string();
    for (re, replacement) in rules {
        if let std::borrow::Cow::Owned(replaced) = re.replace_all(&out, *replacement) {
            out = replaced;
        }
    }
    redact_high_entropy(&out)
}

// ---------------------------------------------------------------------------
// Entropy heuristic
// ---------------------------------------------------------------------------

/// Minimum token length the entropy sweep considers. Shorter strings cannot
/// be distinguished from ordinary identifiers by entropy alone.
const ENTROPY_MIN_LEN: usize = 24;
/// Minimum Shannon entropy (bits per char) for a token to be treated as a
/// machine-generated secret. Random base64/alnum of this length measures
/// ~4.5+; English text and identifiers sit well below 4.
const ENTROPY_MIN_BITS_PER_CHAR: f64 = 4.0;

/// Replace standalone high-entropy tokens with `[REDACTED:high-entropy]`.
///
/// A token qualifies when it is at least [`ENTROPY_MIN_LEN`] chars of
/// secret-alphabet characters (`A-Za-z0-9 + / = _ -`), draws from at least
/// THREE character classes (upper/lower/digit/symbol — this exclusion keeps
/// git SHAs and bare hex, which have at most two classes, out), is not
/// UUID-shaped (memory ids legitimately appear in captures), and measures at
/// least [`ENTROPY_MIN_BITS_PER_CHAR`] of Shannon entropy.
fn redact_high_entropy(text: &str) -> String {
    // Token boundaries: whitespace and characters outside the secret alphabet.
    // `/` is deliberately a SEPARATOR, not a token char: file paths are
    // pervasive in captured tool summaries and would otherwise form one long
    // 3-class token that clears the entropy gate (a catastrophic
    // false-positive class — every absolute path eaten). The cost is that a
    // base64 secret containing slashes is split into segments and only
    // triggers if a segment still clears the length gate; that residual miss
    // is documented in the benchmark corpus and the threat model.
    let is_token_char = |c: char| c.is_ascii_alphanumeric() || matches!(c, '+' | '=' | '_' | '-');
    let mut out = String::with_capacity(text.len());
    let mut token = String::new();
    for c in text.chars() {
        if is_token_char(c) {
            token.push(c);
            continue;
        }
        flush_token(&mut out, &token);
        token.clear();
        out.push(c);
    }
    flush_token(&mut out, &token);
    out
}

fn flush_token(out: &mut String, token: &str) {
    if looks_high_entropy(token) {
        out.push_str("[REDACTED:high-entropy]");
    } else {
        out.push_str(token);
    }
}

fn looks_high_entropy(token: &str) -> bool {
    if token.chars().count() < ENTROPY_MIN_LEN {
        return false;
    }
    if is_uuid_shaped(token) {
        return false;
    }
    let (mut upper, mut lower, mut digit, mut symbol) = (false, false, false, false);
    for c in token.chars() {
        match c {
            'A'..='Z' => upper = true,
            'a'..='z' => lower = true,
            '0'..='9' => digit = true,
            _ => symbol = true,
        }
    }
    let classes = [upper, lower, digit, symbol]
        .into_iter()
        .filter(|b| *b)
        .count();
    if classes < 3 {
        return false;
    }
    shannon_bits_per_char(token) >= ENTROPY_MIN_BITS_PER_CHAR
}

/// `8-4-4-4-12` lowercase/uppercase hex with dashes: a UUID (memory ids and
/// session ids appear legitimately in captured text).
fn is_uuid_shaped(token: &str) -> bool {
    let parts: Vec<&str> = token.split('-').collect();
    parts.len() == 5
        && [8usize, 4, 4, 4, 12]
            .iter()
            .zip(&parts)
            .all(|(len, part)| part.len() == *len && part.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Shannon entropy of the token's character distribution, in bits per char.
fn shannon_bits_per_char(token: &str) -> f64 {
    let mut counts = std::collections::HashMap::new();
    let mut len = 0f64;
    for c in token.chars() {
        *counts.entry(c).or_insert(0f64) += 1.0;
        len += 1.0;
    }
    counts
        .values()
        .map(|count| {
            let p = count / len;
            -p * p.log2()
        })
        .sum()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    /// Build a shape-true FAKE token (`prefix` + `'A'` filler to `total`
    /// chars). Assembled at runtime from split literals so no committed source
    /// byte forms a scanner-matchable token (GitHub push protection blocks
    /// otherwise). The tokens are not real credentials.
    fn tok(prefix: &str, total: usize) -> String {
        format!("{prefix}{}", "A".repeat(total))
    }

    #[test]
    fn all_redaction_rules_compile() {
        assert!(redactions().is_some(), "every redaction rule must compile");
        assert_eq!(redactions().unwrap().len(), REDACT_RULES.len());
    }

    #[test]
    fn redacts_each_known_token_family() {
        // One shape-true (fake) sample per family, BUILT at runtime (see
        // `tok`); the broad measured corpus lives in tests/benchmark.rs.
        let cases = [
            (tok("AKIA", 16), "aws-key"),
            (tok("ghp_", 36), "github-token"),
            (tok("glpat-", 20), "gitlab-token"),
            (
                format!("xoxb-{}-{}", "1".repeat(12), tok("", 24)),
                "slack-token",
            ),
            (tok("sk_live_", 24), "stripe-key"),
            (tok("AIza", 35), "google-api-key"),
            (tok("sk-proj-", 40), "model-api-key"),
            (tok("npm_", 36), "npm-token"),
        ];
        for (sample, kind) in cases {
            let out = redact(&format!("found {sample} in logs"));
            assert!(
                out.contains(&format!("[REDACTED:{kind}]")),
                "{kind} sample must redact: {out}"
            );
            assert!(!out.contains(&sample), "{kind} plaintext must be gone");
        }
    }

    #[test]
    fn redacts_jwt_and_url_password() {
        // Built from split parts (no committed JWT literal); three base64url
        // segments with the `eyJ` header the rule keys on.
        let jwt = format!("{}.{}.{}", tok("eyJ", 20), tok("eyJ", 24), tok("", 43));
        // No credential keyword before the JWT: only the jwt rule can catch it.
        let out = redact(&format!("the header carried {jwt} verbatim"));
        assert!(out.contains("[REDACTED:jwt]"), "{out}");

        let out = redact("postgres://app:hunter2-secret@db.internal:5432/prod");
        assert!(out.contains("[REDACTED:url-password]"), "{out}");
        assert!(!out.contains("hunter2"), "password gone: {out}");
        assert!(out.contains("://app:"), "username kept: {out}");
    }

    #[test]
    fn entropy_sweep_catches_unlabeled_machine_tokens() {
        // A generic mixed-class high-entropy token with no recognizable prefix:
        // only the entropy heuristic can catch it. Built from two diverse
        // halves (each below the length gate) so no committed literal is itself
        // a scanner-matchable high-entropy token.
        let token = format!("{}{}", "q7Zp2Xw9Lk4Tf8Hn1Vb", "5Rs0Yd3Gm6JcQe2Ua8Iz");
        let out = redact(&format!("value {token} end"));
        assert!(
            out.contains("[REDACTED:high-entropy]"),
            "entropy sweep must fire: {out}"
        );
        assert!(!out.contains(&token));
    }

    #[test]
    fn entropy_sweep_spares_dev_artifacts() {
        // git SHAs (hex, 2 classes), UUIDs (excluded by shape), ordinary
        // prose, and identifiers must survive untouched.
        for benign in [
            "0c8e7f763a4f4f7e9d3a1111222233334444aaaa", // 40-hex git SHA
            "0c8e7f76-3a4f-4f7e-9d3a-111122223333",     // UUID
            "the_quick_brown_fox_jumps_over_the_lazy_dog",
            "MemoryEngine::with_fixed_now",
            "supercalifragilisticexpialidocious",
        ] {
            let text = format!("see {benign} here");
            assert_eq!(redact(&text), text, "benign token must survive: {benign}");
        }
    }

    #[test]
    fn redact_is_idempotent() {
        let input = format!("key {} and {}", tok("AKIA", 16), tok("ghp_", 36));
        let once = redact(&input);
        assert_eq!(redact(&once), once, "second pass must be a no-op");
    }
}
