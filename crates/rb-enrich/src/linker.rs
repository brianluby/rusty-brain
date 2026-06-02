use rb_engine::Linker;
use rb_types::{LinkType, MemoryLink, MemoryNote};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use std::time::Duration;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_TOKENS: u32 = 512;

/// Opt-in semantic linker using the OpenAI chat-completions API (compatible with
/// OpenAI, Azure OpenAI, Ollama, llama.cpp, and any other OpenAI-compatible
/// server). Implements the SYNCHRONOUS `rb_engine::Linker`; it performs blocking
/// IO and is meant to be driven via `spawn_blocking`. Any failure degrades to an
/// empty link set (best-effort) and a `warn` log. The key never appears in logs
/// or output. This is a library primitive — not wired into the daemon; the active
/// linker in the engine is `SimilarityLinker`.
///
/// NOTE: Blocking-I/O callers must use `tokio::task::spawn_blocking`.
pub struct OpenAiCompatLinker {
    client: reqwest::blocking::Client,
    api_key: Option<SecretString>,
    model: String,
    base_url: String,
}

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

#[derive(Deserialize)]
struct ModelLinks {
    #[serde(default)]
    links: Vec<ModelLink>,
}

#[derive(Deserialize)]
struct ModelLink {
    index: usize,
    link_type: String,
    strength: f32,
}

impl OpenAiCompatLinker {
    /// Build from the environment. Requires both `RB_ENRICH_BASE_URL` and
    /// `RB_ENRICH_MODEL`; `RB_ENRICH_API_KEY` is optional. Returns `None` when
    /// either required var is absent.
    pub fn from_env() -> Option<Self> {
        let base_url = std::env::var("RB_ENRICH_BASE_URL")
            .ok()
            .filter(|v| !v.is_empty())?;
        let model = std::env::var("RB_ENRICH_MODEL")
            .ok()
            .filter(|v| !v.is_empty())?;
        let api_key = std::env::var("RB_ENRICH_API_KEY")
            .ok()
            .filter(|k| !k.is_empty())
            .map(SecretString::from);
        let client = reqwest::blocking::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .ok()?;
        Some(Self {
            client,
            api_key,
            model,
            base_url: base_url.trim_end_matches('/').to_string(),
        })
    }

    #[cfg(test)]
    pub(crate) fn for_test(model: &str, api_key: Option<&str>, base_url: &str) -> Self {
        let client = reqwest::blocking::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .unwrap_or_else(|_| reqwest::blocking::Client::new());
        Self {
            client,
            api_key: api_key.map(|k| SecretString::from(k.to_string())),
            model: model.to_string(),
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    fn system_prompt() -> &'static str {
        "Given a NEW memory and CANDIDATE memories, respond with ONLY JSON \
         (no prose) of the form {\"links\":[{\"index\":<int>,\"link_type\":\
         <one of extends|contradicts|implements|references|supersedes>,\
         \"strength\":<0.0-1.0>}]}. Omit weak relations."
    }

    fn user_prompt(new: &MemoryNote, candidates: &[(MemoryNote, f32)]) -> String {
        let mut lines = String::new();
        for (i, (note, dist)) in candidates.iter().enumerate() {
            lines.push_str(&format!("[{i}] (distance {dist:.3}) {}\n", note.content));
        }
        format!("NEW:\n{}\n\nCANDIDATES:\n{lines}", new.content)
    }

    /// Inner fallible body; `link` wraps this and degrades errors to empty.
    fn try_link(
        &self,
        new: &MemoryNote,
        candidates: &[(MemoryNote, f32)],
    ) -> rb_types::Result<Vec<MemoryLink>> {
        let url = format!("{}/chat/completions", self.base_url);
        // `response_format: json_object` asks JSON-mode-capable providers to emit
        // a bare JSON object instead of prose-wrapped text. Providers that don't
        // support the field may ignore it; parsing stays lenient and fail-closed
        // (a rejecting provider just yields an error and `link` degrades to an
        // empty link set).
        let body = serde_json::json!({
            "model": self.model,
            "max_tokens": MAX_TOKENS,
            "temperature": 0,
            "response_format": { "type": "json_object" },
            "messages": [
                { "role": "system", "content": Self::system_prompt() },
                { "role": "user",   "content": Self::user_prompt(new, candidates) }
            ]
        });

        let mut req = self.client.post(&url).json(&body);
        if let Some(key) = &self.api_key {
            req = req.header("Authorization", format!("Bearer {}", key.expose_secret()));
        }
        let resp = req
            .send()
            .map_err(|e| rb_types::Error::Enrichment(format!("link request failed: {e}")))?
            .error_for_status()
            .map_err(|e| rb_types::Error::Enrichment(format!("link error status: {e}")))?;

        let parsed: ChatResponse = resp
            .json()
            .map_err(|e| rb_types::Error::Enrichment(format!("link parse failed: {e}")))?;

        let text = parsed
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message.content)
            .ok_or_else(|| {
                rb_types::Error::Enrichment("link response had no content".to_string())
            })?;

        let model: ModelLinks = serde_json::from_str(text.trim())
            .map_err(|e| rb_types::Error::Enrichment(format!("link json invalid: {e}")))?;

        let now = chrono::Utc::now();
        let mut out = Vec::new();
        for ml in model.links {
            let Some((cand, _)) = candidates.get(ml.index) else {
                continue; // model referenced a non-existent candidate; skip.
            };
            if cand.id == new.id {
                continue; // never self-link.
            }
            let Ok(link_type) = LinkType::parse(&ml.link_type) else {
                continue; // unknown type; skip rather than fail.
            };
            out.push(MemoryLink {
                source_id: new.id.clone(),
                target_id: cand.id.clone(),
                link_type,
                strength: ml.strength.clamp(0.0, 1.0),
                reason: "llm".to_string(),
                created_at: now,
            });
        }
        Ok(out)
    }
}

impl Linker for OpenAiCompatLinker {
    fn link(&self, new: &MemoryNote, candidates: &[(MemoryNote, f32)]) -> Vec<MemoryLink> {
        if candidates.is_empty() {
            return Vec::new();
        }
        match self.try_link(new, candidates) {
            Ok(links) => links,
            Err(e) => {
                tracing::warn!(error = %e, "openai-compat linker failed; returning no links");
                Vec::new()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use rb_engine::Linker;
    use rb_types::{LinkType, MemoryNote, MemoryType, Namespace};
    use wiremock::matchers::{body_partial_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn note(content: &str) -> MemoryNote {
        MemoryNote::new(
            Namespace::Project("rb".into()),
            content.to_string(),
            MemoryType::Insight,
            5,
        )
    }

    fn chat_response(json_text: &str) -> serde_json::Value {
        serde_json::json!({
            "choices": [
                { "message": { "role": "assistant", "content": json_text } }
            ]
        })
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn maps_model_link_types_for_candidates() {
        let server = MockServer::start().await;
        let new = note("introduce single-writer daemon");
        let cand = note("agents shared a file via flock");
        let cand_id = cand.id.clone();
        let new_id = new.id.clone();

        // Model is told candidate index 0 EXTENDS the new memory with strength 0.9.
        let model_json = serde_json::json!({
            "links": [
                { "index": 0, "link_type": "extends", "strength": 0.9 }
            ]
        })
        .to_string();

        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(header("Authorization", "Bearer link-test-key"))
            .and(body_partial_json(serde_json::json!({
                "model": "gpt-4o-mini",
                "response_format": { "type": "json_object" }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(chat_response(&model_json)))
            .mount(&server)
            .await;

        let base = format!("{}/v1", server.uri());
        let candidates = vec![(cand, 0.4_f32)];
        let links = tokio::task::spawn_blocking(move || {
            let linker = OpenAiCompatLinker::for_test("gpt-4o-mini", Some("link-test-key"), &base);
            linker.link(&new, &candidates)
        })
        .await
        .unwrap();

        assert_eq!(links.len(), 1);
        assert_eq!(links[0].source_id, new_id);
        assert_eq!(links[0].target_id, cand_id);
        assert_eq!(links[0].link_type, LinkType::Extends);
        assert!((links[0].strength - 0.9).abs() < 1e-6);
        assert_eq!(links[0].reason, "llm");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn http_error_yields_empty_links_not_panic() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
            .mount(&server)
            .await;

        let base = format!("{}/v1", server.uri());
        let new = note("a");
        let candidates = vec![(note("b"), 0.5_f32)];
        let links = tokio::task::spawn_blocking(move || {
            let linker = OpenAiCompatLinker::for_test("gpt-4o-mini", Some("k"), &base);
            linker.link(&new, &candidates)
        })
        .await
        .unwrap();
        assert!(links.is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn out_of_range_index_is_skipped() {
        let server = MockServer::start().await;
        let model_json = serde_json::json!({
            "links": [ { "index": 99, "link_type": "references", "strength": 0.5 } ]
        })
        .to_string();
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(chat_response(&model_json)))
            .mount(&server)
            .await;

        let base = format!("{}/v1", server.uri());
        let new = note("a");
        let candidates = vec![(note("b"), 0.5_f32)];
        let links = tokio::task::spawn_blocking(move || {
            let linker = OpenAiCompatLinker::for_test("gpt-4o-mini", Some("k"), &base);
            linker.link(&new, &candidates)
        })
        .await
        .unwrap();
        assert!(links.is_empty());
    }

    #[test]
    fn empty_candidates_short_circuits_without_network() {
        // No server: if link() called out it would fail to connect. With no
        // candidates it must return empty immediately (no blocking IO).
        let linker =
            OpenAiCompatLinker::for_test("gpt-4o-mini", Some("k"), "http://127.0.0.1:1/v1");
        let new = note("a");
        let links = linker.link(&new, &[]);
        assert!(links.is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn api_key_never_leaks_into_error_messages() {
        // Drive the error path (a 401) and assert the failure string never
        // contains the secret key. `link` swallows errors into an empty Vec, so
        // we exercise the inner `try_link` to inspect the Error::Enrichment
        // message directly (this is also what the `warn!` log would render).
        const SENTINEL_KEY: &str = "super-secret-linker-key-value";
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(401).set_body_string("unauthorized"))
            .mount(&server)
            .await;

        let base = format!("{}/v1", server.uri());
        let new = note("a");
        let candidates = vec![(note("b"), 0.5_f32)];
        let msg = tokio::task::spawn_blocking(move || {
            let linker = OpenAiCompatLinker::for_test("gpt-4o-mini", Some(SENTINEL_KEY), &base);
            // `link` returns an empty Vec on failure; inspect the inner error.
            let err = linker
                .try_link(&new, &candidates)
                .expect_err("401 must produce an enrichment error");
            err.to_string()
        })
        .await
        .unwrap();

        assert!(
            !msg.contains(SENTINEL_KEY),
            "error message leaked the api key: {msg}"
        );
    }
}
