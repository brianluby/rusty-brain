#![allow(clippy::unwrap_used, clippy::expect_used)]

use rb_embed::{DeterministicProvider, EmbeddingProvider, VoyageProvider};

#[tokio::test(flavor = "multi_thread")]
async fn deterministic_provider_satisfies_trait_contract_via_dyn() {
    // Object-safety: the trait must be usable behind a trait object, as the
    // engine/daemon will hold `Box<dyn EmbeddingProvider>` (or a generic param).
    let provider: Box<dyn EmbeddingProvider> = Box::new(DeterministicProvider::new(64));
    assert_eq!(provider.dim(), 64);
    assert_eq!(provider.model_id(), "deterministic");

    let inputs = vec![
        "one transaction one database".to_string(),
        "single writer thread".to_string(),
        "namespace isolation".to_string(),
    ];
    let out = provider.embed(&inputs).await.unwrap();

    // One vector per input, each of length dim().
    assert_eq!(out.len(), inputs.len());
    for v in &out {
        assert_eq!(v.len(), provider.dim());
    }

    // Determinism: re-embedding yields identical vectors.
    let out2 = provider.embed(&inputs).await.unwrap();
    assert_eq!(out, out2);

    // Distinctness: different inputs produce different vectors.
    assert_ne!(out[0], out[1]);
    assert_ne!(out[1], out[2]);
}

#[test]
fn voyage_provider_is_publicly_constructible() {
    // Reachable from the crate root and implements EmbeddingProvider; no network.
    let provider = VoyageProvider::with_model("voyage-3-lite", 1024);
    // Without VOYAGE_API_KEY this is an Embedding error; with it, a valid provider.
    match provider {
        Ok(p) => {
            let p: &dyn EmbeddingProvider = &p;
            assert_eq!(p.dim(), 1024);
            assert_eq!(p.model_id(), "voyage-3-lite");
        }
        Err(e) => assert!(matches!(e, rb_types::Error::Embedding(_))),
    }
}
