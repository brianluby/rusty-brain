use async_trait::async_trait;
use rb_engine::{Enricher, Enrichment};
use rb_types::{Error, MemoryType};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use std::time::Duration;

/// Default model used for enrichment.
const DEFAULT_MODEL: &str = "claude-haiku-4-5";
/// Anthropic API base; `/messages` is appended per request.
const DEFAULT_BASE_URL: &str = "https://api.anthropic.com/v1";
/// Anthropic API version header value.
const ANTHROPIC_VERSION: &str = "2023-06-01";
/// Outbound request timeout (all enrichment calls are timed out).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Upper bound on tokens the model may produce for the enrichment JSON.
const MAX_TOKENS: u32 = 512;

/// Opt-in LLM enricher backed by the Anthropic Messages API. The key is held as
/// a `SecretString` and exposed only when building the request header; it never
/// appears in logs, errors, or Debug output (this type derives no `Debug`).
pub struct AnthropicEnricher {
    client: reqwest::Client,
    api_key: SecretString,
    model: String,
    base_url: String,
}

/// Anthropic `/messages` response — only the fields we read.
#[derive(Deserialize)]
struct MessagesResponse {
    content: Vec<ContentBlock>,
}

#[derive(Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: String,
}

/// The JSON object the model is prompted to emit.
#[derive(Deserialize)]
struct ModelEnrichment {
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    keywords: Vec<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    memory_type: Option<String>,
    #[serde(default)]
    importance: Option<u8>,
}

impl AnthropicEnricher {
    /// Build from the environment. Returns `Ok(None)` when `ANTHROPIC_API_KEY`
    /// is absent so the caller falls back to the heuristic enricher. Returns
    /// `Err(Error::Enrichment)` only if the HTTP client cannot be built.
    pub fn from_env() -> rb_types::Result<Option<Self>> {
        match std::env::var("ANTHROPIC_API_KEY") {
            Ok(key) if !key.is_empty() => {
                Ok(Some(Self::build(DEFAULT_MODEL, key, DEFAULT_BASE_URL)?))
            }
            _ => Ok(None),
        }
    }

    /// Test-only constructor: explicit key + base URL, no environment access.
    #[cfg(test)]
    pub(crate) fn for_test(model: &str, api_key: &str, base_url: &str) -> Self {
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            client,
            api_key: SecretString::from(api_key.to_string()),
            model: model.to_string(),
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    fn build(model: &str, api_key: String, base_url: &str) -> rb_types::Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|e| Error::Enrichment(format!("failed to build http client: {e}")))?;
        Ok(Self {
            client,
            api_key: SecretString::from(api_key),
            model: model.to_string(),
            base_url: base_url.trim_end_matches('/').to_string(),
        })
    }

    fn prompt(content: &str, context: Option<&str>) -> String {
        let ctx = context.unwrap_or("");
        format!(
            "You enrich a developer memory. Respond with ONLY a JSON object \
             (no prose, no code fences) with keys: summary (string, <=150 chars), \
             keywords (array of <=5 lowercase strings), tags (array of strings), \
             memory_type (one of: architecture_decision, code_pattern, bug_fix, \
             configuration, constraint, entity, insight, reference, preference), \
             importance (integer 1-10).\n\nCONTEXT:\n{ctx}\n\nCONTENT:\n{content}"
        )
    }
}

#[async_trait]
impl Enricher for AnthropicEnricher {
    async fn enrich(&self, content: &str, context: Option<&str>) -> rb_types::Result<Enrichment> {
        let url = format!("{}/messages", self.base_url);
        let body = serde_json::json!({
            "model": self.model,
            "max_tokens": MAX_TOKENS,
            "messages": [
                { "role": "user", "content": Self::prompt(content, context) }
            ]
        });

        let resp = self
            .client
            .post(&url)
            .header("x-api-key", self.api_key.expose_secret())
            .header("anthropic-version", ANTHROPIC_VERSION)
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Enrichment(format!("anthropic request failed: {e}")))?;

        let resp = resp
            .error_for_status()
            .map_err(|e| Error::Enrichment(format!("anthropic returned an error status: {e}")))?;

        let parsed: MessagesResponse = resp
            .json()
            .await
            .map_err(|e| Error::Enrichment(format!("failed to parse anthropic response: {e}")))?;

        let text = parsed
            .content
            .into_iter()
            .find(|b| b.kind == "text")
            .map(|b| b.text)
            .ok_or_else(|| Error::Enrichment("anthropic response had no text block".to_string()))?;

        let model: ModelEnrichment = serde_json::from_str(text.trim())
            .map_err(|e| Error::Enrichment(format!("model did not return valid JSON: {e}")))?;

        let memory_type =
            match model.memory_type {
                Some(s) => Some(MemoryType::parse(&s).map_err(|e| {
                    Error::Enrichment(format!("model returned bad memory_type: {e}"))
                })?),
                None => None,
            };
        let importance = match model.importance {
            Some(i) if (1..=10).contains(&i) => Some(i),
            Some(i) => {
                return Err(Error::Enrichment(format!(
                    "model returned importance {i} out of range 1..=10"
                )))
            }
            None => None,
        };

        Ok(Enrichment {
            summary: model.summary,
            keywords: model.keywords,
            tags: model.tags,
            memory_type,
            importance,
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use rb_engine::Enricher;
    use rb_types::MemoryType;
    use wiremock::matchers::{body_partial_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn enricher_for(base_url: &str) -> AnthropicEnricher {
        AnthropicEnricher::for_test("claude-haiku-4-5", "test-key", base_url)
    }

    // Anthropic returns content blocks; our enricher reads the first text block,
    // which the model is prompted to make a JSON object.
    fn message_response(json_text: &str) -> serde_json::Value {
        serde_json::json!({
            "id": "msg_1",
            "type": "message",
            "role": "assistant",
            "content": [ { "type": "text", "text": json_text } ]
        })
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn sends_correct_request_and_parses_enrichment() {
        let server = MockServer::start().await;
        let model_json = serde_json::json!({
            "summary": "single writer over sqlite wal",
            "keywords": ["sqlite", "wal", "writer"],
            "tags": ["architecture"],
            "memory_type": "architecture_decision",
            "importance": 8
        })
        .to_string();

        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .and(header("x-api-key", "test-key"))
            .and(header("anthropic-version", "2023-06-01"))
            .and(body_partial_json(serde_json::json!({
                "model": "claude-haiku-4-5"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(message_response(&model_json)))
            .mount(&server)
            .await;

        let base = format!("{}/v1", server.uri());
        let e = enricher_for(&base);
        let out = e
            .enrich(
                "agents share one sqlite db via a single writer",
                Some("ctx"),
            )
            .await
            .unwrap();

        assert_eq!(
            out.summary.as_deref(),
            Some("single writer over sqlite wal")
        );
        assert_eq!(
            out.keywords,
            vec![
                "sqlite".to_string(),
                "wal".to_string(),
                "writer".to_string()
            ]
        );
        assert_eq!(out.tags, vec!["architecture".to_string()]);
        assert_eq!(out.memory_type, Some(MemoryType::ArchitectureDecision));
        assert_eq!(out.importance, Some(8));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn http_error_status_is_enrichment_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(401).set_body_string("unauthorized"))
            .mount(&server)
            .await;

        let base = format!("{}/v1", server.uri());
        let e = enricher_for(&base);
        let err = e.enrich("x", None).await.unwrap_err();
        assert!(
            matches!(err, rb_types::Error::Enrichment(_)),
            "expected Error::Enrichment, got {err:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn unparseable_model_json_is_enrichment_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(message_response("not json at all")),
            )
            .mount(&server)
            .await;

        let base = format!("{}/v1", server.uri());
        let e = enricher_for(&base);
        let err = e.enrich("x", None).await.unwrap_err();
        assert!(matches!(err, rb_types::Error::Enrichment(_)));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn api_key_never_leaks_into_error_messages() {
        // Point at a closed port so the request fails at the transport layer; the
        // resulting error message must not contain the secret key. reqwest error
        // Display includes the URL + OS error but never request header values.
        let e = AnthropicEnricher::for_test(
            "claude-haiku-4-5",
            "super-secret-key-value",
            "http://127.0.0.1:1/v1",
        );
        let err = e.enrich("x", None).await.unwrap_err();
        let msg = err.to_string();
        assert!(
            !msg.contains("super-secret-key-value"),
            "error message leaked the api key: {msg}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn from_env_absent_key_returns_none() {
        // NOTE: this mutates the process-global ANTHROPIC_API_KEY. It is safe in
        // this crate only because no other (non-ignored) test reads that env var
        // concurrently; restore the prior value to avoid leaking into ignored
        // tests. (edition 2021: set_var/remove_var are not unsafe.)
        let prev = std::env::var("ANTHROPIC_API_KEY").ok();
        std::env::remove_var("ANTHROPIC_API_KEY");
        let got = AnthropicEnricher::from_env();
        if let Some(p) = prev {
            std::env::set_var("ANTHROPIC_API_KEY", p);
        }
        assert!(got.unwrap().is_none());
    }

    // Real-API smoke test. Ignored by default; run with:
    //   ANTHROPIC_API_KEY=... cargo test -p rb-enrich -- --ignored anthropic_real_api
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "requires ANTHROPIC_API_KEY and network access"]
    async fn anthropic_real_api_smoke() {
        let e = AnthropicEnricher::from_env().unwrap().unwrap();
        let out = e
            .enrich("use one sqlite database with a single writer thread", None)
            .await
            .unwrap();
        assert!(out.summary.is_some());
    }
}
