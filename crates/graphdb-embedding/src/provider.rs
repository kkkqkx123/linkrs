//! Embedding provider trait

use async_trait::async_trait;

use super::error::EmbeddingError;

/// Embedding provider trait
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Generate embeddings for a list of texts
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbeddingError>;

    /// Get the embedding dimension
    fn dimension(&self) -> usize;

    /// Get the model name
    fn model_name(&self) -> &str;
}
