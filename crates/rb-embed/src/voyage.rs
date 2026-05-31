use crate::provider::EmbeddingProvider;
use async_trait::async_trait;
use rb_types::Error;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Maximum number of inputs per HTTP request (Voyage batch ceiling).
const MAX_BATCH: usize = 128;
/// Default model and its embedding dimension.
const DEFAULT_MODEL: &str = "voyage-3-lite";
const DEFAULT_DIM: usize = 512;
/// Default API base; the `/embeddings` path is appended per request.
const DEFAULT_BASE_URL: &str = "https://api.voyageai.com/v1";
/// Outbound request timeout (all embedding calls are timed out).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Remote embedding provider backed by the Voyage AI embeddings API.
pub struct VoyageProvider {
    client: reqwest::Client,
    api_key: SecretString,
    model: String,
    dim: usize,
    output_dimension: Option<usize>,
    base_url: String,
}

/// Shape of the Voyage `/embeddings` request we send.
#[derive(Serialize)]
struct EmbeddingsRequest<'a> {
    model: &'a str,
    input: &'a [String],
    input_type: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    output_dimension: Option<usize>,
}

/// Shape of the Voyage `/embeddings` response we depend on.
#[derive(Deserialize)]
struct EmbeddingsResponse {
    data: Vec<EmbeddingData>,
}

#[derive(Deserialize)]
struct EmbeddingData {
    embedding: Vec<f32>,
}

impl VoyageProvider {
    /// Build a provider from the environment. Reads `VOYAGE_API_KEY` (an
    /// `Error::Embedding` if absent), defaults to model `voyage-3-lite` (dim 512),
    /// and constructs a reqwest client with a request timeout.
    pub fn from_env() -> rb_types::Result<Self> {
        let key = std::env::var("VOYAGE_API_KEY")
            .map_err(|_| Error::Embedding("VOYAGE_API_KEY is not set".to_string()))?;
        Self::build(DEFAULT_MODEL, DEFAULT_DIM, None, key, DEFAULT_BASE_URL)
    }

    /// Build a provider for a specific model + dimension, reading the key from
    /// `VOYAGE_API_KEY`. Use this when overriding the default model.
    pub fn with_model(model: &str, dim: usize) -> rb_types::Result<Self> {
        let key = std::env::var("VOYAGE_API_KEY")
            .map_err(|_| Error::Embedding("VOYAGE_API_KEY is not set".to_string()))?;
        Self::build(model, dim, Some(dim), key, DEFAULT_BASE_URL)
    }

    /// Test-only constructor: explicit key + base URL, no environment access.
    #[cfg(test)]
    pub(crate) fn for_test(model: &str, dim: usize, api_key: &str, base_url: &str) -> Self {
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            client,
            api_key: SecretString::from(api_key.to_string()),
            model: model.to_string(),
            dim,
            output_dimension: Some(dim),
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    /// Test-only constructor for default-model behavior that omits
    /// `output_dimension`, matching `from_env`.
    #[cfg(test)]
    pub(crate) fn for_test_default(api_key: &str, base_url: &str) -> Self {
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            client,
            api_key: SecretString::from(api_key.to_string()),
            model: DEFAULT_MODEL.to_string(),
            dim: DEFAULT_DIM,
            output_dimension: None,
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    fn build(
        model: &str,
        dim: usize,
        output_dimension: Option<usize>,
        api_key: String,
        base_url: &str,
    ) -> rb_types::Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|e| Error::Embedding(format!("failed to build http client: {e}")))?;
        Ok(Self {
            client,
            api_key: SecretString::from(api_key),
            model: model.to_string(),
            dim,
            output_dimension,
            base_url: base_url.trim_end_matches('/').to_string(),
        })
    }

    fn embeddings_request<'a>(&'a self, texts: &'a [String]) -> EmbeddingsRequest<'a> {
        EmbeddingsRequest {
            model: &self.model,
            input: texts,
            input_type: "document",
            output_dimension: self.output_dimension,
        }
    }

    /// POST a single chunk of inputs and return their embeddings in order.
    async fn embed_chunk(&self, texts: &[String]) -> rb_types::Result<Vec<Vec<f32>>> {
        let url = format!("{}/embeddings", self.base_url);
        let body = self.embeddings_request(texts);

        let resp = self
            .client
            .post(&url)
            .bearer_auth(self.api_key.expose_secret())
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Embedding(format!("voyage request failed: {e}")))?;

        let resp = resp
            .error_for_status()
            .map_err(|e| Error::Embedding(format!("voyage returned an error status: {e}")))?;

        let parsed: EmbeddingsResponse = resp
            .json()
            .await
            .map_err(|e| Error::Embedding(format!("failed to parse voyage response: {e}")))?;

        if parsed.data.len() != texts.len() {
            return Err(Error::Embedding(format!(
                "voyage returned {} embeddings for {} inputs",
                parsed.data.len(),
                texts.len()
            )));
        }

        let mut out = Vec::with_capacity(parsed.data.len());
        for item in parsed.data {
            if item.embedding.len() != self.dim {
                return Err(Error::Embedding(format!(
                    "embedding dimension mismatch: expected {}, got {}",
                    self.dim,
                    item.embedding.len()
                )));
            }
            out.push(item.embedding);
        }
        Ok(out)
    }
}

#[async_trait]
impl EmbeddingProvider for VoyageProvider {
    fn model_id(&self) -> &str {
        &self.model
    }

    fn dim(&self) -> usize {
        self.dim
    }

    async fn embed(&self, texts: &[String]) -> rb_types::Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let mut out = Vec::with_capacity(texts.len());
        for chunk in texts.chunks(MAX_BATCH) {
            let mut embeddings = self.embed_chunk(chunk).await?;
            out.append(&mut embeddings);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::provider::EmbeddingProvider;
    use wiremock::matchers::{body_partial_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // Build a provider pointed at a mock server with an explicit api key, so the
    // tests never touch the real Voyage endpoint or read the environment.
    // `for_test` is a #[cfg(test)]-only helper on VoyageProvider.
    fn provider_for(base_url: &str, dim: usize) -> VoyageProvider {
        VoyageProvider::for_test("voyage-3-lite", dim, "test-key", base_url)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn metadata_reports_model_and_dim() {
        let p = provider_for("http://127.0.0.1:1/v1", 4);
        assert_eq!(p.model_id(), "voyage-3-lite");
        assert_eq!(p.dim(), 4);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn embed_sends_correct_request_and_parses_response_in_order() {
        let server = MockServer::start().await;
        // The mock asserts the request shape: POST /v1/embeddings, JSON body with
        // the model, the inputs in order, and input_type "document", with bearer auth.
        let response = serde_json::json!({
            "data": [
                { "embedding": [0.1, 0.2, 0.3, 0.4] },
                { "embedding": [0.5, 0.6, 0.7, 0.8] }
            ]
        });
        Mock::given(method("POST"))
            .and(path("/v1/embeddings"))
            .and(header("authorization", "Bearer test-key"))
            .and(body_partial_json(serde_json::json!({
                "model": "voyage-3-lite",
                "input": ["first", "second"],
                "input_type": "document",
                "output_dimension": 4
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(&response))
            .mount(&server)
            .await;

        let base = format!("{}/v1", server.uri());
        let p = provider_for(&base, 4);
        let out = p
            .embed(&["first".to_string(), "second".to_string()])
            .await
            .unwrap();

        assert_eq!(out.len(), 2);
        assert_eq!(out[0], vec![0.1, 0.2, 0.3, 0.4]);
        assert_eq!(out[1], vec![0.5, 0.6, 0.7, 0.8]);
    }

    #[test]
    fn default_model_request_omits_output_dimension() {
        let p = VoyageProvider::for_test_default("test-key", "http://127.0.0.1:1/v1");
        let inputs = vec!["first".to_string(), "second".to_string()];
        let body = serde_json::to_value(p.embeddings_request(&inputs)).unwrap();

        assert_eq!(body["model"], "voyage-3-lite");
        assert_eq!(body["input_type"], "document");
        assert!(body.get("output_dimension").is_none());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn dim_mismatch_is_an_embedding_error() {
        let server = MockServer::start().await;
        // Server returns a 3-length vector but the provider expects dim=4.
        let response = serde_json::json!({
            "data": [ { "embedding": [0.1, 0.2, 0.3] } ]
        });
        Mock::given(method("POST"))
            .and(path("/v1/embeddings"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&response))
            .mount(&server)
            .await;

        let base = format!("{}/v1", server.uri());
        let p = provider_for(&base, 4);
        let err = p.embed(&["x".to_string()]).await.unwrap_err();
        assert!(
            matches!(err, rb_types::Error::Embedding(_)),
            "expected Error::Embedding, got {err:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn count_mismatch_is_an_embedding_error() {
        let server = MockServer::start().await;
        // Two inputs but the server returns only one embedding.
        let response = serde_json::json!({
            "data": [ { "embedding": [0.1, 0.2, 0.3, 0.4] } ]
        });
        Mock::given(method("POST"))
            .and(path("/v1/embeddings"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&response))
            .mount(&server)
            .await;

        let base = format!("{}/v1", server.uri());
        let p = provider_for(&base, 4);
        let err = p
            .embed(&["a".to_string(), "b".to_string()])
            .await
            .unwrap_err();
        assert!(matches!(err, rb_types::Error::Embedding(_)));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn http_error_status_is_an_embedding_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/embeddings"))
            .respond_with(ResponseTemplate::new(401).set_body_string("unauthorized"))
            .mount(&server)
            .await;

        let base = format!("{}/v1", server.uri());
        let p = provider_for(&base, 4);
        let err = p.embed(&["x".to_string()]).await.unwrap_err();
        assert!(matches!(err, rb_types::Error::Embedding(_)));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn empty_input_short_circuits_without_calling_server() {
        // No mock mounted: if embed() called the server it would 404 and error.
        let server = MockServer::start().await;
        let base = format!("{}/v1", server.uri());
        let p = provider_for(&base, 4);
        let out = p.embed(&[]).await.unwrap();
        assert!(out.is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn batches_over_128_are_chunked_and_recombined_in_order() {
        let server = MockServer::start().await;
        // The mock echoes a fixed vector per requested input by reading the request
        // body, so we can assert the total count and ordering after chunking.
        // The closure captures nothing, so it is Send + Sync as Respond requires.
        Mock::given(method("POST"))
            .and(path("/v1/embeddings"))
            .respond_with(|req: &wiremock::Request| {
                let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
                let n = body["input"].as_array().unwrap().len();
                let data: Vec<serde_json::Value> = (0..n)
                    .map(|_| serde_json::json!({ "embedding": [1.0, 0.0, 0.0, 0.0] }))
                    .collect();
                ResponseTemplate::new(200).set_body_json(serde_json::json!({ "data": data }))
            })
            .mount(&server)
            .await;

        let base = format!("{}/v1", server.uri());
        let p = provider_for(&base, 4);
        let inputs: Vec<String> = (0..200).map(|i| format!("item-{i}")).collect();
        let out = p.embed(&inputs).await.unwrap();
        assert_eq!(out.len(), 200);
        for v in &out {
            assert_eq!(v, &vec![1.0, 0.0, 0.0, 0.0]);
        }

        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 2);
        let batch_sizes: Vec<usize> = requests
            .iter()
            .map(|request| {
                let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
                body["input"].as_array().unwrap().len()
            })
            .collect();
        assert_eq!(batch_sizes, vec![128, 72]);
    }

    // Real-API smoke test. Ignored by default; run with:
    //   VOYAGE_API_KEY=... cargo test -p rb-embed -- --ignored voyage_real_api
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "requires VOYAGE_API_KEY and network access"]
    async fn voyage_real_api_smoke() {
        let p = VoyageProvider::from_env().unwrap();
        let out = p.embed(&["hello world".to_string()]).await.unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].len(), p.dim());
    }
}
