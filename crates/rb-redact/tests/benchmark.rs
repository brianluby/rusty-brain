//! W2.4 redaction benchmark over a labeled corpus of shape-true synthetic
//! secrets (the gitleaks rule families), deliberately-uncovered families, and
//! false-positive bait.
//!
//! The corpus is BUILT AT RUNTIME, not committed as a fixture: every secret is
//! assembled from split string literals (`"AKIA"` + filler, etc.) so no
//! committed byte ever forms a scanner-matchable token. The tokens are fake
//! (fixed filler, not real credentials) and exist only in the running test.
//!
//! The measured rates are documented in `docs/THREAT_MODEL.md`; this test pins
//! them so a rule regression (a covered family starting to leak, or a benign
//! dev artifact starting to get eaten) fails CI rather than drifting silently.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;

struct Case {
    family: &'static str,
    /// `true` = a secret that SHOULD be redacted; `false` = benign bait.
    redact: bool,
    text: String,
    note: &'static str,
}

/// Families the rule set deliberately does not cover today (bare 32-hex tokens
/// sit below the entropy gate's three-class requirement — the same property
/// that keeps git SHAs intact — and prose-stated passwords have no
/// machine-recognizable shape). Documented as residual risk in the threat
/// model; a fixture from any OTHER family failing to redact is a regression.
const KNOWN_UNCOVERED: &[&str] = &["twilio-auth-token", "mailchimp-key", "password-in-prose"];

/// `prefix` followed by `seed` padded to `total` chars with `'A'`. The full
/// token never appears as a source literal — only `prefix` and the filler do —
/// so secret scanners cannot match the committed bytes.
fn tok(prefix: &str, seed: &str, total: usize) -> String {
    let mut body = seed.to_string();
    while body.len() < total {
        body.push('A');
    }
    body.truncate(total);
    format!("{prefix}{body}")
}

fn pos(family: &'static str, text: String) -> Case {
    Case {
        family,
        redact: true,
        text,
        note: "",
    }
}

fn uncovered(family: &'static str, text: String, note: &'static str) -> Case {
    Case {
        family,
        redact: true,
        text,
        note,
    }
}

fn benign(text: &'static str) -> Case {
    Case {
        family: "benign",
        redact: false,
        text: text.to_string(),
        note: "",
    }
}

/// Build the full labeled corpus (62 positives + 10 benign = 72 cases).
fn corpus() -> Vec<Case> {
    let mut cases = Vec::new();

    // --- covered families: 3 shape-true samples each ---
    for i in 0..3 {
        let s = i.to_string();
        cases.push(pos(
            "aws-access-key",
            format!("aws_access_key_id is {} in ~/.aws", tok("AKIA", &s, 16)),
        ));
        cases.push(pos(
            "github-pat",
            format!("pushed with {} to origin", tok("ghp_", &s, 36)),
        ));
        cases.push(pos(
            "github-fine-grained-pat",
            format!("created {} for CI", tok("github_pat_", &s, 82)),
        ));
        cases.push(pos(
            "gitlab-pat",
            format!("export GL={} for the runner", tok("glpat-", &s, 20)),
        ));
        cases.push(pos(
            "slack-bot-token",
            format!(
                "slack bot uses {}-{}",
                tok("xoxb-", &"1".repeat(12), 12 + 5),
                tok("", &s, 24)
            ),
        ));
        cases.push(pos(
            "slack-webhook",
            format!(
                "posting to https://hooks.slack.com/services/{}/{}/{}",
                tok("T", &s, 8),
                tok("B", &s, 8),
                tok("", &s, 24)
            ),
        ));
        cases.push(pos(
            "stripe-secret-key",
            format!("charge with {} key", tok("sk_live_", &s, 24)),
        ));
        cases.push(pos(
            "google-api-key",
            format!("maps key {} configured", tok("AIza", &s, 35)),
        ));
        cases.push(pos(
            "openai-api-key",
            format!("OPENAI key {} found in env dump", tok("sk-proj-", &s, 40)),
        ));
        cases.push(pos(
            "anthropic-api-key",
            format!("the agent used {} for calls", tok("sk-ant-api03-", &s, 40)),
        ));
        cases.push(pos(
            "npm-token",
            format!(
                "//registry.npmjs.org/:_authToken bound {}",
                tok("npm_", &s, 36)
            ),
        ));
        cases.push(pos(
            "sendgrid-key",
            format!("mailer set to {}.{}", tok("SG.", &s, 22), tok("", &s, 43)),
        ));
        cases.push(pos(
            "jwt",
            format!(
                "session cookie {}.{}.{} captured",
                tok("eyJ", &s, 20),
                tok("eyJ", &s, 24),
                tok("", &s, 43)
            ),
        ));
        cases.push(pos(
            "url-password",
            format!(
                "connect via postgres://svc:{}@db{i}.internal:5432/app",
                tok("", &s, 14)
            ),
        ));
        cases.push(pos(
            "authorization-header",
            format!(
                "curl -H 'Authorization: Bearer {}' https://api{i}.example.com",
                tok("", &s, 32)
            ),
        ));
        cases.push(pos(
            "credential-assignment",
            format!("DATABASE_PASSWORD={} written to .env", tok("", &s, 18)),
        ));
        cases.push(pos(
            "credential-assignment-json",
            format!(r#"{{"api_key": "{}"}}"#, tok("", &s, 28)),
        ));
        // Generic high-entropy: no provider prefix; only the entropy sweep can
        // catch it. Built from two diverse halves (each < the length gate) so
        // neither half is itself a scanner-matchable high-entropy literal.
        cases.push(Case {
            family: "generic-high-entropy",
            redact: true,
            text: format!(
                "value {}{}{i} leaked in output",
                "aZ9bY8cX7dW6eV5fU4", "gT3hS2iR1jQ0kPmLnK"
            ),
            note: "no recognizable prefix; entropy sweep only",
        });
    }
    // PEM private keys (terminated + unterminated). The BEGIN/END markers are
    // split by `{}` in the source so the committed bytes are not a valid PEM
    // header; the runtime string is.
    cases.push(pos(
        "pem-private-key",
        format!(
            "-----BEGIN {} PRIVATE KEY-----\n{}\n-----END {} PRIVATE KEY-----",
            "RSA",
            "A".repeat(48),
            "RSA"
        ),
    ));
    cases.push(pos(
        "pem-private-key",
        format!(
            "-----BEGIN {} PRIVATE KEY-----\n{}",
            "OPENSSH",
            "A".repeat(40)
        ),
    ));

    // --- deliberately-uncovered families (documented FN classes) ---
    for i in 0..2 {
        cases.push(uncovered(
            "twilio-auth-token",
            format!(
                "twilio auth token {} in config",
                tok("", &i.to_string(), 32)
            ),
            "bare 32-char single-class token: below the 3-class entropy gate; NOT covered",
        ));
        cases.push(uncovered(
            "mailchimp-key",
            format!("mailchimp uses {}-us{i}", tok("", &i.to_string(), 32)),
            "hex+region suffix, below the entropy gate; NOT covered",
        ));
        cases.push(uncovered(
            "password-in-prose",
            format!("the admin password is correct-horse-battery-{i} apparently"),
            "no key=value separator, prose-stated; NOT covered",
        ));
    }

    // --- negatives: false-positive bait that must SURVIVE untouched ---
    for s in [
        "commit 0c8e7f763a4f4f7e9d3a1111222233334444aaaa fixed the writer race",
        "memory id 0c8e7f76-3a4f-4f7e-9d3a-111122223333 was archived",
        "run cargo test --workspace --all-targets before pushing",
        "the function rb_types::validate_confidence guards the range",
        "TODO: rename AbstractSingletonProxyFactoryBean before review",
        "downloaded all-MiniLM-L6-v2 weights on first use",
        "Reciprocal Rank Fusion blends keyword vector and graph ranks",
        "set RUSTY_BRAIN_IDLE_TIMEOUT_SECS to sixty for the e2e",
        "branch feat/p9-phase2-trust-security pushed upstream",
        "layer sha256 9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08 pulled",
    ] {
        cases.push(benign(s));
    }

    cases
}

#[test]
fn benchmark_measures_and_pins_detection_and_false_positive_rates() {
    let cases = corpus();
    let mut per_family: BTreeMap<&str, (usize, usize)> = BTreeMap::new(); // (caught, total)
    let (mut positives, mut caught, mut negatives, mut false_positives) = (0, 0, 0, 0);

    for case in &cases {
        let out = rb_redact::redact(&case.text);
        let changed = out != case.text;
        if case.redact {
            positives += 1;
            let entry = per_family.entry(case.family).or_insert((0, 0));
            entry.1 += 1;
            if changed {
                caught += 1;
                entry.0 += 1;
            } else if !KNOWN_UNCOVERED.contains(&case.family) {
                panic!(
                    "covered family '{}' leaked a secret: {:?} ({})",
                    case.family, case.text, case.note
                );
            }
        } else {
            negatives += 1;
            if changed {
                false_positives += 1;
                eprintln!(
                    "FALSE POSITIVE on benign text: {:?} -> {:?}",
                    case.text, out
                );
            }
        }
    }

    let detection = caught as f64 / positives as f64;
    eprintln!("redaction benchmark over {} cases:", cases.len());
    eprintln!(
        "  detection {caught}/{positives} = {:.1}% (false-negative rate {:.1}%)",
        detection * 100.0,
        (1.0 - detection) * 100.0
    );
    for (family, (c, t)) in &per_family {
        eprintln!("  {family}: {c}/{t}");
    }
    eprintln!("  false positives: {false_positives}/{negatives} benign samples");

    // Pin the measured floor (currently 56/62 ≈ 90%): the only acceptable
    // misses are the KNOWN_UNCOVERED families above.
    assert!(
        detection >= 0.90,
        "detection regressed below the documented floor: {detection:.3}"
    );
    // Conservative-by-design allows false positives in principle, but the
    // committed bait set (git SHAs, UUIDs, identifiers, prose) must survive:
    // each of these appearing in everyday captures would destroy recall value.
    assert_eq!(
        false_positives, 0,
        "benign dev artifacts must never be eaten"
    );
}

#[test]
fn every_known_uncovered_family_is_actually_still_uncovered() {
    // The KNOWN_UNCOVERED list must shrink the moment a rule starts covering a
    // family — otherwise the documented false-negative classes go stale.
    for case in corpus() {
        if case.redact && KNOWN_UNCOVERED.contains(&case.family) {
            assert_eq!(
                rb_redact::redact(&case.text),
                case.text,
                "family '{}' is now covered — remove it from KNOWN_UNCOVERED \
                 and update docs/THREAT_MODEL.md",
                case.family
            );
        }
    }
}
