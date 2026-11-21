use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;

use crate::{Embedder, EmbedderError};

pub struct HttpEmbedder {
    client: reqwest::Client,
    endpoint: String,
    model: String,
    api_key: Option<String>,
    batch_size: usize,
    dimensions: AtomicUsize,
}

impl std::fmt::Debug for HttpEmbedder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpEmbedder")
            .field("endpoint", &self.endpoint)
            .field("model", &self.model)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field("batch_size", &self.batch_size)
            .finish()
    }
}

#[derive(Deserialize)]
struct EmbeddingsResponse {
    data: Vec<EmbeddingItem>,
}

#[derive(Deserialize)]
struct EmbeddingItem {
    index: usize,
    embedding: Vec<f32>,
}

impl HttpEmbedder {
    pub fn new(base_url: &str, model: impl Into<String>, dimensions: usize) -> Self {
        let client = reqwest::Client::builder().timeout(Duration::from_secs(120)).build().unwrap_or_default();
        Self { client, endpoint: embeddings_endpoint(base_url), model: model.into(), api_key: None, batch_size: 64, dimensions: AtomicUsize::new(dimensions) }
    }

    pub fn with_api_key(mut self, api_key: impl Into<String>) -> Self {
        let api_key = api_key.into();
        self.api_key = (!api_key.trim().is_empty()).then_some(api_key);
        self
    }

    pub fn with_batch_size(mut self, batch_size: usize) -> Self {
        self.batch_size = batch_size.max(1);
        self
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    async fn embed_batch(&self, batch: &[String]) -> Result<Vec<Vec<f32>>, EmbedderError> {
        let mut request = self.client.post(&self.endpoint).json(&serde_json::json!({ "model": self.model, "input": batch }));
        if let Some(key) = &self.api_key {
            request = request.bearer_auth(key);
        }
        let response = request.send().await.map_err(|error| EmbedderError::Provider(format!("request to {} failed: {error}", self.endpoint)))?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            let excerpt: String = body.chars().take(300).collect();
            return Err(EmbedderError::Provider(format!("{} answered {status}: {excerpt}", self.endpoint)));
        }
        let mut parsed: EmbeddingsResponse =
            response.json().await.map_err(|error| EmbedderError::Provider(format!("unreadable embeddings response: {error}")))?;
        if parsed.data.len() != batch.len() {
            return Err(EmbedderError::Provider(format!("asked for {} embeddings, got {}", batch.len(), parsed.data.len())));
        }
        parsed.data.sort_by_key(|item| item.index);
        let vectors: Vec<Vec<f32>> = parsed.data.into_iter().map(|item| item.embedding).collect();
        self.check_dimensions(&vectors)?;
        Ok(vectors)
    }

    fn check_dimensions(&self, vectors: &[Vec<f32>]) -> Result<(), EmbedderError> {
        for vector in vectors {
            let known = self.dimensions.load(Ordering::Relaxed);
            if known == 0 {
                self.dimensions.store(vector.len(), Ordering::Relaxed);
            } else if known != vector.len() {
                return Err(EmbedderError::Provider(format!("expected {known} dimensions, got {}", vector.len())));
            }
        }
        Ok(())
    }
}

fn embeddings_endpoint(base_url: &str) -> String {
    let base = base_url.trim().trim_end_matches('/');
    if base.ends_with("/embeddings") {
        base.to_owned()
    } else if base.ends_with("/v1") {
        format!("{base}/embeddings")
    } else {
        format!("{base}/v1/embeddings")
    }
}

#[async_trait]
impl Embedder for HttpEmbedder {
    fn model(&self) -> &str {
        &self.model
    }

    fn dimensions(&self) -> usize {
        self.dimensions.load(Ordering::Relaxed)
    }

    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedderError> {
        let mut vectors = Vec::with_capacity(texts.len());
        for batch in texts.chunks(self.batch_size) {
            vectors.extend(self.embed_batch(batch).await?);
        }
        Ok(vectors)
    }
}
