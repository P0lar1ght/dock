//! Embedding provider for memory vector search.
//!
//! Dock HTTP + API key via [`crate::config::MemoryEmbeddingConfig`]
//! (`[memory.embedding]` model/base/dimensions). No `xai-grok-auth`.
//! Missing config → callers stay FTS-only.

use async_trait::async_trait;

use crate::config::MemoryEmbeddingConfig;

const MAX_RETRIES: usize = 3;
const INITIAL_BACKOFF_MS: u64 = 1000;

#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    async fn embed_batch(
        &self,
        texts: &[&str],
    ) -> Result<Vec<Vec<f32>>, Box<dyn std::error::Error>>;

    fn model_name(&self) -> &str;

    fn dimensions(&self) -> usize;
}

/// OpenAI-compatible `/embeddings` provider.
pub struct ApiEmbeddingProvider {
    api_base: String,
    api_key: String,
    model: String,
    dimensions: usize,
    client: reqwest::Client,
    max_batch_size: usize,
}

impl ApiEmbeddingProvider {
    pub fn new(api_base: String, api_key: String, model: String, dimensions: usize) -> Self {
        // Soft-fail to FTS on hang: bound connect + per-request so sampler
        // `embed_query_if_configured` cannot stall forever.
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            api_base: api_base.trim_end_matches('/').to_string(),
            api_key,
            model,
            dimensions,
            client,
            max_batch_size: 32,
        }
    }

    /// Build from `[memory.embedding]` when model + base are set.
    /// API key from config or `DOCK_MEMORY_EMBEDDING_API_KEY` / `OPENAI_API_KEY`.
    pub fn from_config(config: &MemoryEmbeddingConfig) -> Option<Self> {
        if !config.is_configured() {
            return None;
        }
        let model = config.model.clone()?;
        let base = config.base.clone()?;
        let key = config
            .api_key
            .clone()
            .filter(|k| !k.is_empty())
            .or_else(|| std::env::var("DOCK_MEMORY_EMBEDDING_API_KEY").ok())
            .or_else(|| std::env::var("OPENAI_API_KEY").ok())
            .filter(|k| !k.is_empty())?;
        Some(Self::new(base, key, model, config.dimensions))
    }
}

#[async_trait]
impl EmbeddingProvider for ApiEmbeddingProvider {
    #[tracing::instrument(name = "memory.embed_batch", skip_all, fields(batch_size = texts.len()))]
    async fn embed_batch(
        &self,
        texts: &[&str],
    ) -> Result<Vec<Vec<f32>>, Box<dyn std::error::Error>> {
        if texts.is_empty() {
            return Ok(vec![]);
        }
        let mut all_embeddings = Vec::with_capacity(texts.len());
        for batch in texts.chunks(self.max_batch_size) {
            let input: Vec<&str> = batch.to_vec();
            let body_json = serde_json::json!({
                "model": self.model,
                "input": input,
                "dimensions": self.dimensions,
            });
            let mut last_err = String::new();
            let mut success = false;
            for attempt in 0..MAX_RETRIES {
                if attempt > 0 {
                    let delay = INITIAL_BACKOFF_MS * 2u64.pow(attempt as u32 - 1);
                    tracing::warn!(attempt, delay_ms = delay, "retrying embedding API");
                    tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                }
                let response = match self
                    .client
                    .post(format!("{}/embeddings", self.api_base))
                    .bearer_auth(&self.api_key)
                    .json(&body_json)
                    .send()
                    .await
                {
                    Ok(r) => r,
                    Err(e) => {
                        last_err = format!("request failed: {e}");
                        continue;
                    }
                };
                let status = response.status();
                if status.is_success() {
                    let body: serde_json::Value = response.json().await?;
                    let data = body
                        .get("data")
                        .and_then(|d| d.as_array())
                        .ok_or("embedding response missing 'data' array")?;
                    for item in data {
                        let embedding: Vec<f32> = item
                            .get("embedding")
                            .and_then(|e| e.as_array())
                            .ok_or("embedding item missing 'embedding' array")?
                            .iter()
                            .filter_map(|v| v.as_f64().map(|f| f as f32))
                            .collect();
                        all_embeddings.push(embedding);
                    }
                    success = true;
                    break;
                }
                if status.as_u16() == 429 || status.is_server_error() {
                    last_err = format!(
                        "HTTP {status}: {}",
                        response.text().await.unwrap_or_default()
                    );
                    continue;
                }
                let body = response.text().await.unwrap_or_default();
                return Err(format!("embedding API error {status}: {body}").into());
            }
            if !success {
                return Err(format!(
                    "embedding API failed after {MAX_RETRIES} attempts: {last_err}"
                )
                .into());
            }
        }
        Ok(all_embeddings)
    }

    fn model_name(&self) -> &str {
        &self.model
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }
}

/// Deterministic mock for tests / hybrid without network.
#[cfg(any(test, feature = "test-support"))]
pub struct MockEmbeddingProvider {
    pub dimensions: usize,
}

#[cfg(any(test, feature = "test-support"))]
#[async_trait]
impl EmbeddingProvider for MockEmbeddingProvider {
    async fn embed_batch(
        &self,
        texts: &[&str],
    ) -> Result<Vec<Vec<f32>>, Box<dyn std::error::Error>> {
        Ok(texts
            .iter()
            .map(|text| {
                let hash = blake3::hash(text.as_bytes());
                let bytes = hash.as_bytes();
                bytes
                    .iter()
                    .cycle()
                    .take(self.dimensions)
                    .map(|&b| b as f32 / 255.0)
                    .collect()
            })
            .collect())
    }

    fn model_name(&self) -> &str {
        "mock-embedding"
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }
}

/// Embed all chunks lacking vectors. Soft-fail per batch.
///
/// Prefer [`embed_missing_chunks_owned`] from `Send` async tasks: rusqlite
/// `Connection` is `Send` + !`Sync`, so `&MemoryIndex` across `.await` is not
/// `Send`.
pub async fn embed_missing_chunks(
    index: &crate::index::MemoryIndex,
    provider: &dyn EmbeddingProvider,
) -> usize {
    // Local !Send future is fine for tests / single-threaded callers.
    let chunks = match index.chunks_without_embeddings() {
        Ok(c) if c.is_empty() => return 0,
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "failed to query chunks without embeddings");
            return 0;
        }
    };
    embed_batches(index, provider, &chunks).await
}

/// `Send`-friendly: owns the index so the async future can move across threads.
pub async fn embed_missing_chunks_owned(
    index: crate::index::MemoryIndex,
    provider: &dyn EmbeddingProvider,
) -> usize {
    let chunks = match index.chunks_without_embeddings() {
        Ok(c) if c.is_empty() => return 0,
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "failed to query chunks without embeddings");
            return 0;
        }
    };
    let total = chunks.len();
    let mut embedded = 0;
    for batch in chunks.chunks(32) {
        let texts: Vec<&str> = batch.iter().map(|(_, text)| text.as_str()).collect();
        match provider.embed_batch(&texts).await {
            Ok(embeddings) => {
                for ((chunk_id, _), embedding) in batch.iter().zip(embeddings.iter()) {
                    if let Err(e) = index.upsert_embedding(chunk_id, embedding) {
                        tracing::warn!(chunk_id, error = %e, "failed to upsert embedding");
                    } else {
                        embedded += 1;
                    }
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, batch_size = texts.len(), "embedding batch failed");
            }
        }
    }
    if embedded > 0 {
        tracing::info!(embedded, total, "embedded missing chunks");
    }
    embedded
}

async fn embed_batches(
    index: &crate::index::MemoryIndex,
    provider: &dyn EmbeddingProvider,
    chunks: &[(String, String)],
) -> usize {
    let total = chunks.len();
    let mut embedded = 0;
    for batch in chunks.chunks(32) {
        let texts: Vec<&str> = batch.iter().map(|(_, text)| text.as_str()).collect();
        match provider.embed_batch(&texts).await {
            Ok(embeddings) => {
                for ((chunk_id, _), embedding) in batch.iter().zip(embeddings.iter()) {
                    if let Err(e) = index.upsert_embedding(chunk_id, embedding) {
                        tracing::warn!(chunk_id, error = %e, "failed to upsert embedding");
                    } else {
                        embedded += 1;
                    }
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, batch_size = texts.len(), "embedding batch failed");
            }
        }
    }
    if embedded > 0 {
        tracing::info!(embedded, total, "embedded missing chunks");
    }
    embedded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_embedding_deterministic() {
        let provider = MockEmbeddingProvider { dimensions: 4 };
        let r1 = provider.embed_batch(&["hello"]).await.unwrap();
        let r2 = provider.embed_batch(&["hello"]).await.unwrap();
        assert_eq!(r1, r2);
    }

    #[tokio::test]
    async fn mock_embedding_different_texts() {
        let provider = MockEmbeddingProvider { dimensions: 4 };
        let results = provider.embed_batch(&["hello", "world"]).await.unwrap();
        assert_ne!(results.first(), results.get(1));
    }

    #[tokio::test]
    async fn mock_correct_dimensions() {
        let provider = MockEmbeddingProvider { dimensions: 128 };
        let results = provider.embed_batch(&["test"]).await.unwrap();
        assert_eq!(results.first().map(Vec::len), Some(128));
    }

    #[test]
    fn from_config_requires_model_base_key() {
        let mut cfg = MemoryEmbeddingConfig::default();
        assert!(ApiEmbeddingProvider::from_config(&cfg).is_none());
        cfg.model = Some("text-embedding-3-small".into());
        cfg.base = Some("https://api.openai.com/v1".into());
        cfg.api_key = Some("sk-test".into());
        assert!(ApiEmbeddingProvider::from_config(&cfg).is_some());
    }
}
