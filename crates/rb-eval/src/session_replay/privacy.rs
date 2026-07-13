//! Fail-closed, session-consistent de-identification.

use std::collections::{BTreeMap, BTreeSet};

use fake::faker::company::en::CompanyName;
use fake::faker::internet::en::Username;
use fake::faker::name::en::Name;
use fake::Fake;
use rand::rngs::StdRng;
use rand::SeedableRng;
use regex::{Captures, Regex};
use sha2::{Digest, Sha256};
use thiserror::Error;

use super::schema::{RedactionCategory, RejectionCategory};

const MAX_TEXT_BYTES: usize = 1_000_000;

const EMAIL_PATTERN: &str = r"(?i)\b[A-Z0-9.!#$%&'*+/=?^_`{|}~-]+@[A-Z0-9-]+(?:\.[A-Z0-9-]+)+\b";
const HOME_PATH_PATTERN: &str = r#"(?i)(?:/Users|/home)/[^/\s\"'`<>]+(?:/[^\s\"'`<>]*)?|[A-Z]:\\Users\\[^\\\s\"'`<>]+(?:\\[^\s\"'`<>]*)*"#;
const ABSOLUTE_PATH_PATTERN: &str =
    r#"(?x)(?:^|[\s(\[{:])(?P<path>/(?:private|Volumes|tmp|var|opt|srv|mnt|etc)/[^\s\"'`<>]*)"#;
const URL_PATTERN: &str = r#"(?i)\b(?:https?|ssh)://[^\s\"'`<>]+"#;
const DOMAIN_PATTERN: &str =
    r"(?i)\b(?:[a-z0-9-]+\.)+(?:com|org|net|io|dev|app|cloud|internal|local|lan|test)\b";
const IPV4_PATTERN: &str =
    r"\b(?:25[0-5]|2[0-4][0-9]|1?[0-9]{1,2})(?:\.(?:25[0-5]|2[0-4][0-9]|1?[0-9]{1,2})){3}\b";
const IPV6_PATTERN: &str = r"(?i)\b(?:[0-9a-f]{1,4}:){2,7}[0-9a-f]{1,4}\b";
const PHONE_PATTERN: &str =
    r"(?x)(?:\+?1[-.\s]?)?(?:\([0-9]{3}\)|[0-9]{3})[-.\s][0-9]{3}[-.\s][0-9]{4}\b";
const LABELED_HOST_PATTERN: &str =
    r"(?i)\b(host(?:name)?|server|machine)(\s*[:=]\s*)([a-z0-9][a-z0-9._-]*)";
const LABELED_NAME_PATTERN: &str =
    r"\b(name|owner|author|contact)(\s*[:=]\s*)([A-Z][a-z]+(?:[ -][A-Z][a-z]+){1,3})";
const LABELED_USER_PATTERN: &str =
    r"(?i)\b(user(?:name)?|account)(\s*[:=]\s*)([a-z0-9][a-z0-9._-]*)";
const LABELED_ORG_PATTERN: &str =
    r"\b(organization|company|client)(\s*[:=]\s*)([A-Z][A-Za-z0-9 &.-]{2,80})";
const REDACTION_MARKER_PATTERN: &str = r"\[REDACTED:([a-z0-9-]+)\]";

/// Redaction failed before sanitized text could be produced.
#[derive(Debug, Error)]
pub enum PrivacyError {
    #[error("privacy rules are unavailable")]
    RulesUnavailable,
    #[error("content exceeds the local replay safety limit")]
    OversizedContent,
    #[error("sensitive data remained after redaction")]
    ResidualSensitiveData,
}

impl PrivacyError {
    pub(crate) fn rejection_category(&self) -> RejectionCategory {
        match self {
            Self::RulesUnavailable => RejectionCategory::RedactionUnavailable,
            Self::OversizedContent => RejectionCategory::OversizedContent,
            Self::ResidualSensitiveData => RejectionCategory::ResidualSensitiveData,
        }
    }
}

/// Sanitized content plus aggregate-safe categories that were replaced.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SanitizedText {
    pub text: String,
    pub categories: Vec<RedactionCategory>,
}

struct Patterns {
    email: Regex,
    home_path: Regex,
    absolute_path: Regex,
    url: Regex,
    domain: Regex,
    ipv4: Regex,
    ipv6: Regex,
    phone: Regex,
    labeled_host: Regex,
    labeled_name: Regex,
    labeled_user: Regex,
    labeled_org: Regex,
    redaction_marker: Regex,
}

impl Patterns {
    fn compile() -> Result<Self, regex::Error> {
        Self::compile_with_email(EMAIL_PATTERN)
    }

    fn compile_with_email(email_pattern: &str) -> Result<Self, regex::Error> {
        Ok(Self {
            email: Regex::new(email_pattern)?,
            home_path: Regex::new(HOME_PATH_PATTERN)?,
            absolute_path: Regex::new(ABSOLUTE_PATH_PATTERN)?,
            url: Regex::new(URL_PATTERN)?,
            domain: Regex::new(DOMAIN_PATTERN)?,
            ipv4: Regex::new(IPV4_PATTERN)?,
            ipv6: Regex::new(IPV6_PATTERN)?,
            phone: Regex::new(PHONE_PATTERN)?,
            labeled_host: Regex::new(LABELED_HOST_PATTERN)?,
            labeled_name: Regex::new(LABELED_NAME_PATTERN)?,
            labeled_user: Regex::new(LABELED_USER_PATTERN)?,
            labeled_org: Regex::new(LABELED_ORG_PATTERN)?,
            redaction_marker: Regex::new(REDACTION_MARKER_PATTERN)?,
        })
    }

    fn has_residual(&self, text: &str) -> bool {
        self.email.is_match(text)
            || self.home_path.is_match(text)
            || self.absolute_path.is_match(text)
            || self.url.is_match(text)
            || self.domain.is_match(text)
            || self.ipv4.is_match(text)
            || self.ipv6.is_match(text)
            || self.phone.is_match(text)
            || self.labeled_host.is_match(text)
            || self.labeled_name.is_match(text)
            || self.labeled_user.is_match(text)
            || self.labeled_org.is_match(text)
    }
}

/// Session-scoped redactor. Each input value maps to one deterministic Faker alias.
pub struct StrictRedactor {
    patterns: Patterns,
    rng: StdRng,
    emails: BTreeMap<String, String>,
    homes: BTreeMap<String, String>,
    hosts: BTreeMap<String, String>,
    names: BTreeMap<String, String>,
    users: BTreeMap<String, String>,
    organizations: BTreeMap<String, String>,
}

impl StrictRedactor {
    /// Create a redactor whose aliases are stable for this session and seed.
    pub fn new(seed: u64, session_key: &str) -> Result<Self, PrivacyError> {
        let patterns = Patterns::compile().map_err(|_| PrivacyError::RulesUnavailable)?;
        Ok(Self::with_patterns(seed, session_key, patterns))
    }

    fn with_patterns(seed: u64, session_key: &str, patterns: Patterns) -> Self {
        Self {
            patterns,
            rng: StdRng::seed_from_u64(derived_seed(seed, session_key)),
            emails: BTreeMap::new(),
            homes: BTreeMap::new(),
            hosts: BTreeMap::new(),
            names: BTreeMap::new(),
            users: BTreeMap::new(),
            organizations: BTreeMap::new(),
        }
    }

    /// Sanitize one string. Any rule or residual-scan failure rejects the string.
    pub fn sanitize(&mut self, text: &str) -> Result<SanitizedText, PrivacyError> {
        if text.len() > MAX_TEXT_BYTES {
            return Err(PrivacyError::OversizedContent);
        }

        let mut categories = BTreeSet::new();
        let mut output = rb_redact::redact(text);
        for captures in self.patterns.redaction_marker.captures_iter(&output) {
            let category = if captures
                .get(1)
                .is_some_and(|value| value.as_str() == "high-entropy")
            {
                RedactionCategory::HighEntropy
            } else {
                RedactionCategory::Credential
            };
            categories.insert(category);
        }

        output = self.replace_home_paths(&output, &mut categories);
        output = self.replace_emails(&output, &mut categories);
        output = replace_constant(
            &self.patterns.phone,
            &output,
            "[REDACTED:phone]",
            RedactionCategory::Phone,
            &mut categories,
        );
        output = replace_constant(
            &self.patterns.ipv4,
            &output,
            "[REDACTED:ip-address]",
            RedactionCategory::IpAddress,
            &mut categories,
        );
        output = replace_constant(
            &self.patterns.ipv6,
            &output,
            "[REDACTED:ip-address]",
            RedactionCategory::IpAddress,
            &mut categories,
        );
        output = self.replace_urls(&output, &mut categories);
        output = self.replace_labeled_hosts(&output, &mut categories);
        output = self.replace_domains(&output, &mut categories);
        output = self.replace_labeled_names(&output, &mut categories);
        output = self.replace_labeled_users(&output, &mut categories);
        output = self.replace_labeled_organizations(&output, &mut categories);
        output = replace_absolute_paths(&self.patterns.absolute_path, &output, &mut categories);

        // A second shared secret pass is a backstop for values exposed by a
        // surrounding replacement. It must reach a fixpoint before output.
        let second_pass = rb_redact::redact(&output);
        if second_pass != output || self.patterns.has_residual(&output) {
            return Err(PrivacyError::ResidualSensitiveData);
        }

        Ok(SanitizedText {
            text: output,
            categories: categories.into_iter().collect(),
        })
    }

    fn replace_home_paths(
        &mut self,
        text: &str,
        categories: &mut BTreeSet<RedactionCategory>,
    ) -> String {
        let regex = self.patterns.home_path.clone();
        regex
            .replace_all(text, |captures: &Captures<'_>| {
                categories.insert(RedactionCategory::HomePath);
                let original = captures.get(0).map_or("", |value| value.as_str());
                let alias = alias_username(&mut self.rng, &mut self.homes, original);
                format!("<FAKE_HOME:{alias}>")
            })
            .into_owned()
    }

    fn replace_emails(
        &mut self,
        text: &str,
        categories: &mut BTreeSet<RedactionCategory>,
    ) -> String {
        let regex = self.patterns.email.clone();
        regex
            .replace_all(text, |captures: &Captures<'_>| {
                categories.insert(RedactionCategory::Email);
                let original = captures.get(0).map_or("", |value| value.as_str());
                let alias = alias_username(&mut self.rng, &mut self.emails, original);
                format!("<FAKE_EMAIL:{alias}>")
            })
            .into_owned()
    }

    fn replace_urls(&mut self, text: &str, categories: &mut BTreeSet<RedactionCategory>) -> String {
        let regex = self.patterns.url.clone();
        regex
            .replace_all(text, |captures: &Captures<'_>| {
                categories.insert(RedactionCategory::Url);
                let original = captures.get(0).map_or("", |value| value.as_str());
                let alias = alias_username(&mut self.rng, &mut self.hosts, original);
                format!("<FAKE_URL:{alias}>")
            })
            .into_owned()
    }

    fn replace_domains(
        &mut self,
        text: &str,
        categories: &mut BTreeSet<RedactionCategory>,
    ) -> String {
        let regex = self.patterns.domain.clone();
        regex
            .replace_all(text, |captures: &Captures<'_>| {
                categories.insert(RedactionCategory::Hostname);
                let original = captures.get(0).map_or("", |value| value.as_str());
                let alias = alias_username(&mut self.rng, &mut self.hosts, original);
                format!("<FAKE_HOST:{alias}>")
            })
            .into_owned()
    }

    fn replace_labeled_hosts(
        &mut self,
        text: &str,
        categories: &mut BTreeSet<RedactionCategory>,
    ) -> String {
        let regex = self.patterns.labeled_host.clone();
        regex
            .replace_all(text, |captures: &Captures<'_>| {
                categories.insert(RedactionCategory::Hostname);
                let original = captures.get(3).map_or("", |value| value.as_str());
                let alias = alias_username(&mut self.rng, &mut self.hosts, original);
                format!("{}{}<FAKE_HOST:{alias}>", &captures[1], &captures[2])
            })
            .into_owned()
    }

    fn replace_labeled_names(
        &mut self,
        text: &str,
        categories: &mut BTreeSet<RedactionCategory>,
    ) -> String {
        let regex = self.patterns.labeled_name.clone();
        regex
            .replace_all(text, |captures: &Captures<'_>| {
                categories.insert(RedactionCategory::PersonalName);
                let original = captures.get(3).map_or("", |value| value.as_str());
                let alias = if let Some(existing) = self.names.get(original) {
                    existing.clone()
                } else {
                    let generated: String = Name().fake_with_rng(&mut self.rng);
                    self.names.insert(original.to_string(), generated.clone());
                    generated
                };
                format!("{}{}<FAKE_NAME:{alias}>", &captures[1], &captures[2])
            })
            .into_owned()
    }

    fn replace_labeled_users(
        &mut self,
        text: &str,
        categories: &mut BTreeSet<RedactionCategory>,
    ) -> String {
        let regex = self.patterns.labeled_user.clone();
        regex
            .replace_all(text, |captures: &Captures<'_>| {
                categories.insert(RedactionCategory::UserIdentifier);
                let original = captures.get(3).map_or("", |value| value.as_str());
                let alias = alias_username(&mut self.rng, &mut self.users, original);
                format!("{}{}<FAKE_USER:{alias}>", &captures[1], &captures[2])
            })
            .into_owned()
    }

    fn replace_labeled_organizations(
        &mut self,
        text: &str,
        categories: &mut BTreeSet<RedactionCategory>,
    ) -> String {
        let regex = self.patterns.labeled_org.clone();
        regex
            .replace_all(text, |captures: &Captures<'_>| {
                categories.insert(RedactionCategory::Organization);
                let original = captures.get(3).map_or("", |value| value.as_str());
                let alias = if let Some(existing) = self.organizations.get(original) {
                    existing.clone()
                } else {
                    let generated: String = CompanyName().fake_with_rng(&mut self.rng);
                    self.organizations
                        .insert(original.to_string(), generated.clone());
                    generated
                };
                format!("{}{}<FAKE_ORG:{alias}>", &captures[1], &captures[2])
            })
            .into_owned()
    }
}

fn replace_constant(
    regex: &Regex,
    text: &str,
    replacement: &str,
    category: RedactionCategory,
    categories: &mut BTreeSet<RedactionCategory>,
) -> String {
    if regex.is_match(text) {
        categories.insert(category);
    }
    regex.replace_all(text, replacement).into_owned()
}

fn replace_absolute_paths(
    regex: &Regex,
    text: &str,
    categories: &mut BTreeSet<RedactionCategory>,
) -> String {
    regex
        .replace_all(text, |captures: &Captures<'_>| {
            categories.insert(RedactionCategory::AbsolutePath);
            let whole = captures.get(0).map_or("", |value| value.as_str());
            let path = captures.name("path").map_or("", |value| value.as_str());
            whole.replace(path, "<ABSOLUTE_PATH>")
        })
        .into_owned()
}

fn alias_username(
    rng: &mut StdRng,
    aliases: &mut BTreeMap<String, String>,
    original: &str,
) -> String {
    if let Some(existing) = aliases.get(original) {
        return existing.clone();
    }
    let generated: String = Username().fake_with_rng(rng);
    aliases.insert(original.to_string(), generated.clone());
    generated
}

fn derived_seed(seed: u64, session_key: &str) -> u64 {
    let mut hasher = Sha256::new();
    hasher.update(seed.to_le_bytes());
    hasher.update(session_key.as_bytes());
    let digest = hasher.finalize();
    u64::from_le_bytes(digest[..8].try_into().unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases_are_consistent_within_a_session() {
        let mut redactor = StrictRedactor::new(7, "session-a").unwrap();
        let first = redactor
            .sanitize("email dev@private.test then dev@private.test")
            .unwrap();
        let aliases: Vec<_> = first.text.match_indices("<FAKE_EMAIL:").collect();
        assert_eq!(aliases.len(), 2, "both occurrences must be replaced");
        let parts: Vec<_> = first.text.split(" then ").collect();
        assert_eq!(parts[0].trim_start_matches("email "), parts[1]);
    }

    #[test]
    fn credentials_paths_and_personal_data_are_removed() {
        let mut redactor = StrictRedactor::new(11, "session-b").unwrap();
        let token = format!("{}{}", "q7Zp2Xw9Lk4Tf8Hn1Vb", "5Rs0Yd3Gm6JcQe2Ua8Iz");
        let input = format!(
            "name: Casey Example email casey@private.test home /Users/casey/proj \
             host=build.internal phone 415-555-0199 secret={token}"
        );
        let result = redactor.sanitize(&input).unwrap();
        assert!(!result.text.contains("Casey Example"));
        assert!(!result.text.contains("casey@private.test"));
        assert!(!result.text.contains("/Users/casey"));
        assert!(!result.text.contains("build.internal"));
        assert!(!result.text.contains("415-555-0199"));
        assert!(!result.text.contains(&token));
    }

    #[test]
    fn invalid_rule_compilation_fails_closed() {
        assert!(Patterns::compile_with_email("[").is_err());
    }

    #[test]
    fn oversized_content_is_rejected() {
        let mut redactor = StrictRedactor::new(1, "session-c").unwrap();
        let error = redactor
            .sanitize(&"x".repeat(MAX_TEXT_BYTES + 1))
            .unwrap_err();
        assert!(matches!(error, PrivacyError::OversizedContent));
    }
}
