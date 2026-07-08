//! Configuration for embedding service

use serde::{Deserialize, Serialize};

use super::error::EmbeddingError;
use super::preprocessor::PreprocessorConfig;

/// Embedding service configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    /// API endpoint URL
    pub base_url: String,
    /// API key (optional for some providers)
    #[serde(default)]
    pub api_key: Option<String>,
    /// Model name to use
    pub model: String,
    /// Request timeout in seconds
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    /// Expected vector dimension (optional, will be auto-detected if not set)
    pub dimension: Option<usize>,
    /// Preprocessor configuration for text transformation
    #[serde(default)]
    pub preprocessor: PreprocessorConfig,
}

fn default_timeout() -> u64 {
    30
}

impl EmbeddingConfig {
    /// Create a new configuration
    pub fn new(base_url: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            api_key: None,
            model: model.into(),
            timeout_secs: default_timeout(),
            dimension: None,
            preprocessor: PreprocessorConfig::default(),
        }
    }

    /// Set API key
    pub fn with_api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }

    /// Set timeout
    pub fn with_timeout(mut self, timeout_secs: u64) -> Self {
        self.timeout_secs = timeout_secs;
        self
    }

    /// Set dimension
    pub fn with_dimension(mut self, dimension: usize) -> Self {
        self.dimension = Some(dimension);
        self
    }

    /// Set preprocessor
    pub fn with_preprocessor(mut self, preprocessor: PreprocessorConfig) -> Self {
        self.preprocessor = preprocessor;
        self
    }

    /// Validate the configuration
    pub fn validate(&self) -> Result<(), EmbeddingError> {
        if self.base_url.is_empty() {
            return Err(EmbeddingError::Config("base_url is required".to_string()));
        }

        if self.model.is_empty() {
            return Err(EmbeddingError::Config("model is required".to_string()));
        }

        // Validate URL format
        if let Err(e) = url::Url::parse(&self.base_url) {
            return Err(EmbeddingError::Config(format!("Invalid base_url: {}", e)));
        }

        Ok(())
    }
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            base_url: "http://localhost:11434/api/embeddings".to_string(),
            api_key: None,
            model: "all-minilm".to_string(),
            timeout_secs: default_timeout(),
            dimension: None,
            preprocessor: PreprocessorConfig::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_config() {
        let cfg = EmbeddingConfig::new("http://example.com", "test-model");
        assert_eq!(cfg.base_url, "http://example.com");
        assert_eq!(cfg.model, "test-model");
        assert!(cfg.api_key.is_none());
        assert_eq!(cfg.timeout_secs, 30);
    }

    #[test]
    fn test_with_api_key() {
        let cfg = EmbeddingConfig::new("http://example.com", "model").with_api_key("sk-test");
        assert_eq!(cfg.api_key, Some("sk-test".into()));
    }

    #[test]
    fn test_with_timeout() {
        let cfg = EmbeddingConfig::new("http://example.com", "model").with_timeout(60);
        assert_eq!(cfg.timeout_secs, 60);
    }

    #[test]
    fn test_with_dimension() {
        let cfg = EmbeddingConfig::new("http://example.com", "model").with_dimension(768);
        assert_eq!(cfg.dimension, Some(768));
    }

    #[test]
    fn test_with_preprocessor() {
        let cfg = EmbeddingConfig::new("http://example.com", "model").with_preprocessor(
            PreprocessorConfig::Prefix {
                prefix: "query: ".into(),
            },
        );
        match cfg.preprocessor {
            PreprocessorConfig::Prefix { ref prefix } => assert_eq!(prefix, "query: "),
            _ => panic!("expected Prefix preprocessor"),
        }
    }

    #[test]
    fn test_validate_valid() {
        let cfg = EmbeddingConfig::new("http://localhost:11434/api/embeddings", "model");
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn test_validate_empty_base_url() {
        let cfg = EmbeddingConfig::new("", "model");
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("base_url"));
    }

    #[test]
    fn test_validate_empty_model() {
        let cfg = EmbeddingConfig::new("http://example.com", "");
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("model"));
    }

    #[test]
    fn test_validate_invalid_url() {
        let cfg = EmbeddingConfig::new("not-a-url", "model");
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("Invalid base_url"));
    }

    #[test]
    fn test_default_config() {
        let cfg = EmbeddingConfig::default();
        assert_eq!(cfg.model, "all-minilm");
        assert!(cfg.api_key.is_none());
    }
}
