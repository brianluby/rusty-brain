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
    /// Position of this embedding in the original input array.
    /// Voyage may return embeddings out of input order; we use this field to
    /// re-pair each embedding to its source text.
    ///
    /// For a multi-item batch, a missing `index` is a fail-closed error: we
    /// cannot safely assume array order, so we refuse rather than risk a silent
    /// text/embedding mis-pairing. A single-item response may omit it (the only
    /// possible position is 0, so the pairing is unambiguous).
    #[serde(default)]
    index: Option<usize>,
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

        // Re-order embeddings by their declared `index` field so that a Voyage
        // response returned out of input order is correctly re-paired.
        //
        // Fail closed on a multi-item batch missing `index`: without it we
        // cannot prove the array order matches the input order, so we refuse
        // rather than risk a silent mis-pairing. A single-item response may
        // omit `index` (position 0 is the only option, so it is unambiguous).
        let n = texts.len();
        let mut slots: Vec<Option<Vec<f32>>> = (0..n).map(|_| None).collect();
        for (pos, item) in parsed.data.into_iter().enumerate() {
            let slot_idx = match item.index {
                Some(idx) => idx,
                None if n == 1 => pos,
                None => {
                    return Err(Error::Embedding(
                        "voyage response missing index for multi-item batch".to_string(),
                    ));
                }
            };
            if slot_idx >= n {
                return Err(Error::Embedding(format!(
                    "voyage returned embedding with index {} but input length is {}",
                    slot_idx, n
                )));
            }
            if slots[slot_idx].is_some() {
                return Err(Error::Embedding(format!(
                    "voyage returned duplicate embedding for index {}",
                    slot_idx
                )));
            }
            if item.embedding.len() != self.dim {
                return Err(Error::Embedding(format!(
                    "embedding dimension mismatch: expected {}, got {}",
                    self.dim,
                    item.embedding.len()
                )));
            }
            slots[slot_idx] = Some(item.embedding);
        }

        // Every slot must be filled; the count check above ensures exactly
        // n items were returned, so any missing slot means a duplicate index.
        let out: Option<Vec<Vec<f32>>> = slots.into_iter().collect();
        out.ok_or_else(|| {
            Error::Embedding(
                "voyage response had missing embedding slots after index placement".to_string(),
            )
        })
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
        // Voyage always returns an `index` per item; we include it here to match
        // real API behavior (a multi-item batch without index now fails closed).
        let response = serde_json::json!({
            "data": [
                { "index": 0, "embedding": [0.1, 0.2, 0.3, 0.4] },
                { "index": 1, "embedding": [0.5, 0.6, 0.7, 0.8] }
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
                // Include a per-item `index` as the real Voyage API does.
                let data: Vec<serde_json::Value> = (0..n)
                    .map(|i| serde_json::json!({ "index": i, "embedding": [1.0, 0.0, 0.0, 0.0] }))
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

    #[tokio::test(flavor = "multi_thread")]
    async fn out_of_order_index_fields_are_correctly_re_paired() {
        let server = MockServer::start().await;
        // Voyage returns embeddings in reverse order (index 1 first, index 0 second).
        // Without honoring the `index` field the results would be silently swapped.
        let response = serde_json::json!({
            "data": [
                { "index": 1, "embedding": [0.5, 0.6, 0.7, 0.8] },
                { "index": 0, "embedding": [0.1, 0.2, 0.3, 0.4] }
            ]
        });
        Mock::given(method("POST"))
            .and(path("/v1/embeddings"))
            .and(header("authorization", "Bearer test-key"))
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
        // "first" must be paired with the embedding at index 0, regardless of
        // the order in which Voyage returned the items.
        assert_eq!(
            out[0],
            vec![0.1, 0.2, 0.3, 0.4],
            "index 0 embedding must be placed at position 0"
        );
        assert_eq!(
            out[1],
            vec![0.5, 0.6, 0.7, 0.8],
            "index 1 embedding must be placed at position 1"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn duplicate_index_in_response_is_an_embedding_error() {
        let server = MockServer::start().await;
        // Both returned items claim index 0 — this must be detected and rejected.
        let response = serde_json::json!({
            "data": [
                { "index": 0, "embedding": [0.1, 0.2, 0.3, 0.4] },
                { "index": 0, "embedding": [0.5, 0.6, 0.7, 0.8] }
            ]
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
        assert!(
            matches!(err, rb_types::Error::Embedding(_)),
            "duplicate index must be Error::Embedding, got {err:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn out_of_range_index_is_an_embedding_error() {
        let server = MockServer::start().await;
        // One item has an index beyond the input length.
        let response = serde_json::json!({
            "data": [
                { "index": 0, "embedding": [0.1, 0.2, 0.3, 0.4] },
                { "index": 99, "embedding": [0.5, 0.6, 0.7, 0.8] }
            ]
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
        assert!(
            matches!(err, rb_types::Error::Embedding(_)),
            "out-of-range index must be Error::Embedding, got {err:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn multi_item_response_missing_index_fails_closed() {
        let server = MockServer::start().await;
        // A 2-item response with NO `index` fields. We cannot prove the array
        // order matches the input order, so this must fail closed rather than
        // silently fall back to array order (the mis-pairing risk).
        let response = serde_json::json!({
            "data": [
                { "embedding": [0.1, 0.2, 0.3, 0.4] },
                { "embedding": [0.5, 0.6, 0.7, 0.8] }
            ]
        });
        Mock::given(method("POST"))
            .and(path("/v1/embeddings"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&response))
            .mount(&server)
            .await;

        let base = format!("{}/v1", server.uri());
        let p = provider_for(&base, 4);
        let err = p
            .embed(&["first".to_string(), "second".to_string()])
            .await
            .unwrap_err();
        // Assert it is an Embedding error whose message names the failure mode.
        let is_missing_index = matches!(
            &err,
            rb_types::Error::Embedding(msg) if msg.contains("missing index for multi-item batch")
        );
        assert!(
            is_missing_index,
            "expected the multi-item missing-index error, got: {err:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn single_item_response_without_index_is_accepted() {
        let server = MockServer::start().await;
        // A single-item response may omit `index`: position 0 is the only
        // possible slot, so the pairing is unambiguous and allowed.
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
        let out = p.embed(&["only".to_string()]).await.unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0], vec![0.1, 0.2, 0.3, 0.4]);
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
