use async_trait::async_trait;
use rb_engine::{Enricher, Enrichment};
use rb_types::{Error, MemoryType};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use std::time::Duration;

/// Outbound request timeout (all enrichment calls are timed out).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Upper bound on tokens the model may produce for the enrichment JSON.
const MAX_TOKENS: u32 = 512;

/// Opt-in LLM enricher using the OpenAI chat-completions API (compatible with
/// OpenAI, Azure OpenAI, Ollama, llama.cpp, and any other OpenAI-compatible
/// server). The base URL and model are caller-supplied; the API key is optional
/// (omit for local servers that need no auth). The key is held as a
/// `SecretString` and exposed only when building the Authorization header; it
/// never appears in logs, errors, or Debug output (this type derives no `Debug`).
pub struct OpenAiCompatEnricher {
    client: reqwest::Client,
    api_key: Option<SecretString>,
    model: String,
    base_url: String,
}

/// OpenAI `/chat/completions` response — only the fields we read.
#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: ChatMessage,
}

#[derive(Deserialize)]
struct ChatMessage {
    content: Option<String>,
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

impl OpenAiCompatEnricher {
    /// Build from the environment. Requires both `RB_ENRICH_BASE_URL` and
    /// `RB_ENRICH_MODEL` to be set (explicit opt-in). `RB_ENRICH_API_KEY` is
    /// optional (local servers like Ollama need none). Returns `Ok(None)` when
    /// either required var is absent — this is not an error, just no LLM path.
    /// Returns `Err(Error::Enrichment)` only if the HTTP client cannot be built.
    pub fn from_env() -> rb_types::Result<Option<Self>> {
        Self::from_env_with(|key| std::env::var(key).ok())
    }

    /// Pure core: build from an injected config getter so tests can drive the
    /// opt-in logic without mutating process-global env (unsound under parallel
    /// `#[test]` on Rust 1.81+). The real `from_env` injects a closure over
    /// `std::env::var`. Mirrors `daemon_command_with` on this branch.
    fn from_env_with<F>(get: F) -> rb_types::Result<Option<Self>>
    where
        F: Fn(&str) -> Option<String>,
    {
        let base_url = match get("RB_ENRICH_BASE_URL") {
            Some(v) if !v.is_empty() => v,
            _ => return Ok(None),
        };
        let model = match get("RB_ENRICH_MODEL") {
            Some(v) if !v.is_empty() => v,
            _ => return Ok(None),
        };
        let api_key = get("RB_ENRICH_API_KEY")
            .filter(|k| !k.is_empty())
            .map(SecretString::from);

        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|e| Error::Enrichment(format!("failed to build http client: {e}")))?;

        Ok(Some(Self {
            client,
            api_key,
            model,
            base_url: base_url.trim_end_matches('/').to_string(),
        }))
    }

    /// Test-only constructor: explicit config, no environment access.
    #[cfg(test)]
    pub(crate) fn for_test(model: &str, api_key: Option<&str>, base_url: &str) -> Self {
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            client,
            api_key: api_key.map(|k| SecretString::from(k.to_string())),
            model: model.to_string(),
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    fn system_prompt() -> &'static str {
        "You enrich a developer memory. Respond with ONLY a JSON object \
         (no prose, no code fences) with keys: summary (string, <=150 chars), \
         keywords (array of <=5 lowercase strings), tags (array of strings), \
         memory_type (one of: architecture_decision, code_pattern, bug_fix, \
         configuration, constraint, entity, insight, reference, preference), \
         importance (integer 1-10)."
    }

    fn user_prompt(content: &str, context: Option<&str>) -> String {
        let ctx = context.unwrap_or("");
        format!("CONTEXT:\n{ctx}\n\nCONTENT:\n{content}")
    }
}

#[async_trait]
impl Enricher for OpenAiCompatEnricher {
    async fn enrich(&self, content: &str, context: Option<&str>) -> rb_types::Result<Enrichment> {
        let url = format!("{}/chat/completions", self.base_url);
        // `response_format: json_object` asks JSON-mode-capable providers (e.g.
        // OpenAI) to emit a bare JSON object instead of prose-wrapped text.
        // Providers that don't support the field may ignore it; parsing stays
        // lenient and fail-closed (a provider that rejects the field just yields
        // an error and the engine degrades to the heuristic fallback).
        let body = serde_json::json!({
            "model": self.model,
            "max_tokens": MAX_TOKENS,
            "temperature": 0,
            "response_format": { "type": "json_object" },
            "messages": [
                { "role": "system", "content": Self::system_prompt() },
                { "role": "user",   "content": Self::user_prompt(content, context) }
            ]
        });

        let mut req = self.client.post(&url).json(&body);
        if let Some(key) = &self.api_key {
            req = req.header("Authorization", format!("Bearer {}", key.expose_secret()));
        }
        let resp = req
            .send()
            .await
            .map_err(|e| Error::Enrichment(format!("enrichment request failed: {e}")))?;

        let resp = resp
            .error_for_status()
            .map_err(|e| Error::Enrichment(format!("enrichment returned an error status: {e}")))?;

        let parsed: ChatResponse = resp
            .json()
            .await
            .map_err(|e| Error::Enrichment(format!("failed to parse enrichment response: {e}")))?;

        let text = parsed
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message.content)
            .ok_or_else(|| {
                Error::Enrichment("enrichment response had no content in choices".to_string())
            })?;

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

    fn enricher_for(base_url: &str) -> OpenAiCompatEnricher {
        OpenAiCompatEnricher::for_test("gpt-4o-mini", Some("test-key"), base_url)
    }

    fn enricher_no_key(base_url: &str) -> OpenAiCompatEnricher {
        OpenAiCompatEnricher::for_test("gpt-4o-mini", None, base_url)
    }

    // OpenAI returns chat choices; our enricher reads choices[0].message.content,
    // which the model is prompted to make a JSON object.
    fn chat_response(json_text: &str) -> serde_json::Value {
        serde_json::json!({
            "id": "chatcmpl-1",
            "object": "chat.completion",
            "choices": [
                {
                    "index": 0,
                    "message": { "role": "assistant", "content": json_text },
                    "finish_reason": "stop"
                }
            ]
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
            .and(path("/v1/chat/completions"))
            .and(header("Authorization", "Bearer test-key"))
            .and(body_partial_json(serde_json::json!({
                "model": "gpt-4o-mini",
                "response_format": { "type": "json_object" }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(chat_response(&model_json)))
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
    async fn no_authorization_header_when_key_absent() {
        // True negative assertion: inspect the recorded request and confirm no
        // `authorization` header was sent when the enricher has no key.
        let server = MockServer::start().await;
        let model_json = serde_json::json!({
            "summary": "ok",
            "keywords": [],
            "tags": [],
            "memory_type": null,
            "importance": null
        })
        .to_string();

        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(chat_response(&model_json)))
            .mount(&server)
            .await;

        let base = format!("{}/v1", server.uri());
        let e = enricher_no_key(&base);
        let out = e.enrich("content", None).await.unwrap();
        assert_eq!(out.summary.as_deref(), Some("ok"));

        // The keyless enricher must NOT have sent an Authorization header.
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1, "exactly one request recorded");
        assert!(
            requests[0].headers.get("authorization").is_none(),
            "no Authorization header must be sent when the enricher has no key"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn http_error_status_is_enrichment_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
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
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(chat_response("not json at all")),
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
        let e = OpenAiCompatEnricher::for_test(
            "gpt-4o-mini",
            Some("super-secret-key-value"),
            "http://127.0.0.1:1/v1",
        );
        let err = e.enrich("x", None).await.unwrap_err();
        let msg = err.to_string();
        assert!(
            !msg.contains("super-secret-key-value"),
            "error message leaked the api key: {msg}"
        );
    }

    // `from_env_with` tests drive the opt-in logic from an in-memory map so no
    // process-global env is mutated (sound under parallel `#[test]`). The real
    // `from_env` is a one-line closure over `std::env::var` exercised by the
    // ignored real-API smoke test.
    fn map_getter(pairs: &[(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        let map: std::collections::HashMap<&'static str, &'static str> =
            pairs.iter().copied().collect();
        move |key| map.get(key).map(|v| (*v).to_string())
    }

    #[test]
    fn from_env_with_both_required_present_returns_some_with_key() {
        let get = map_getter(&[
            ("RB_ENRICH_BASE_URL", "https://api.openai.com/v1"),
            ("RB_ENRICH_MODEL", "gpt-4o-mini"),
            ("RB_ENRICH_API_KEY", "sk-test"),
        ]);
        let e = OpenAiCompatEnricher::from_env_with(get).unwrap().unwrap();
        assert_eq!(e.base_url, "https://api.openai.com/v1");
        assert_eq!(e.model, "gpt-4o-mini");
        assert!(e.api_key.is_some());
    }

    #[test]
    fn from_env_with_key_absent_returns_some_without_key() {
        // Local servers (Ollama, llama.cpp) need no key: both required vars set
        // but no RB_ENRICH_API_KEY -> Some with api_key None.
        let get = map_getter(&[
            ("RB_ENRICH_BASE_URL", "http://localhost:11434/v1"),
            ("RB_ENRICH_MODEL", "llama3"),
        ]);
        let e = OpenAiCompatEnricher::from_env_with(get).unwrap().unwrap();
        assert_eq!(e.base_url, "http://localhost:11434/v1");
        assert_eq!(e.model, "llama3");
        assert!(e.api_key.is_none());
    }

    #[test]
    fn from_env_with_base_url_missing_returns_none() {
        let get = map_getter(&[("RB_ENRICH_MODEL", "gpt-4o-mini")]);
        assert!(OpenAiCompatEnricher::from_env_with(get).unwrap().is_none());
    }

    #[test]
    fn from_env_with_model_missing_returns_none() {
        let get = map_getter(&[("RB_ENRICH_BASE_URL", "https://api.openai.com/v1")]);
        assert!(OpenAiCompatEnricher::from_env_with(get).unwrap().is_none());
    }

    #[test]
    fn from_env_with_empty_required_values_returns_none() {
        // Empty-string values are treated as absent (not a valid opt-in).
        let get = map_getter(&[("RB_ENRICH_BASE_URL", ""), ("RB_ENRICH_MODEL", "")]);
        assert!(OpenAiCompatEnricher::from_env_with(get).unwrap().is_none());
    }

    // Real-API smoke test. Ignored by default; run with:
    //   RB_ENRICH_BASE_URL=https://api.openai.com/v1 \
    //   RB_ENRICH_MODEL=gpt-4o-mini \
    //   RB_ENRICH_API_KEY=sk-... \
    //   cargo test -p rb-enrich -- --ignored openai_compat_real_api
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "requires RB_ENRICH_BASE_URL, RB_ENRICH_MODEL, RB_ENRICH_API_KEY and network access"]
    async fn openai_compat_real_api_smoke() {
        let e = OpenAiCompatEnricher::from_env().unwrap().unwrap();
        let out = e
            .enrich("use one sqlite database with a single writer thread", None)
            .await
            .unwrap();
        assert!(out.summary.is_some());
    }
}
