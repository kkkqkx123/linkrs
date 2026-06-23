//! Embedding service wrapper

use super::config::EmbeddingConfig;
use super::error::{EmbeddingError, Result};
use super::provider::EmbeddingProvider;

use super::providers::OpenAICompatibleProvider;

/// Embedding service wrapper
///
/// This service provides a unified interface for HTTP-based embedding providers.
pub struct EmbeddingService {
    provider: OpenAICompatibleProvider,
    config: EmbeddingConfig,
    dimension: usize,
}

impl EmbeddingService {
    /// Create from configuration (HTTP-based provider)
    ///
    /// This creates an OpenAI-compatible HTTP provider.
    ///
    /// # Example
    ///
    /// ```
    /// use vector_client::EmbeddingService;
    ///
    /// let config = vector_client::EmbeddingConfig::new(
    ///     "http://localhost:11434/api/embeddings",
    ///     "all-minilm"
    /// ).with_dimension(384);
    /// let service = EmbeddingService::from_config(config).expect("failed");
    /// ```
    pub fn from_config(config: EmbeddingConfig) -> Result<Self> {
        config.validate()?;

        let provider = OpenAICompatibleProvider::new(config.clone())?;
        let dimension = provider.dimension();

        Ok(Self {
            provider,
            config,
            dimension,
        })
    }

    /// Embed a single text
    pub async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let embeddings = self.provider.embed(&[text]).await?;
        embeddings
            .into_iter()
            .next()
            .ok_or_else(|| EmbeddingError::InvalidResponse("No embedding returned".to_string()))
    }

    /// Embed multiple texts in batch
    pub async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        self.provider.embed(texts).await
    }

    /// Get the dimension
    pub fn dimension(&self) -> usize {
        self.provider.dimension()
    }

    /// Get the model name
    pub fn model_name(&self) -> &str {
        self.provider.model_name()
    }
}

impl std::fmt::Debug for EmbeddingService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmbeddingService")
            .field("model", &self.config.model)
            .field("dimension", &self.dimension)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_config_fails_without_dimension() {
        let config = EmbeddingConfig::new("http://localhost:11434/api/embeddings", "model");
        let result = EmbeddingService::from_config(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_service_debug() {
        let config = EmbeddingConfig::new("http://example.com", "my-model").with_dimension(768);
        let service = EmbeddingService::from_config(config).expect("from_config failed");
        let debug_str = format!("{:?}", service);
        assert!(debug_str.contains("my-model"));
        assert!(debug_str.contains("768"));
    }
}
