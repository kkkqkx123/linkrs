//! OpenAI-compatible HTTP provider for embeddings
//!
//! Supports OpenAI, Gemini, Azure, Ollama, and any OpenAI-compatible endpoint.
//!
//! This provider uses reqwest directly for HTTP operations without external LLM client dependencies.

use std::time::Duration;

use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::embedding::config::EmbeddingConfig;
use crate::embedding::error::{EmbeddingError, Result};
use crate::embedding::preprocessor::PreprocessorImpl;
use crate::embedding::provider::EmbeddingProvider;

/// OpenAI-compatible HTTP provider
///
/// This provider supports any OpenAI-compatible API endpoint including:
/// - OpenAI API
/// - Google Gemini (via OpenAI compatibility layer)
/// - Azure OpenAI
/// - Ollama
/// - Self-hosted embedding services
pub struct OpenAICompatibleProvider {
    client: Client,
    config: EmbeddingConfig,
    preprocessor: PreprocessorImpl,
    dimension: usize,
}

impl std::fmt::Debug for OpenAICompatibleProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAICompatibleProvider")
            .field("model", &self.config.model)
            .field("base_url", &self.config.base_url)
            .field("dimension", &self.dimension)
            .finish()
    }
}

#[derive(Debug, Serialize)]
struct EmbeddingRequest {
    model: String,
    input: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    encoding_format: Option<String>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingData>,
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingData {
    index: usize,
    embedding: Vec<f32>,
}

#[derive(Debug, Deserialize)]
struct Usage {
    prompt_tokens: usize,
    total_tokens: usize,
}

impl EmbeddingResponse {
    fn usage(&self) -> Option<&Usage> {
        self.usage.as_ref()
    }
}

impl Usage {
    fn prompt_tokens(&self) -> usize {
        self.prompt_tokens
    }

    fn total_tokens(&self) -> usize {
        self.total_tokens
    }
}

impl OpenAICompatibleProvider {
    /// Create a new OpenAI-compatible provider
    ///
    /// # Arguments
    ///
    /// * `config` - Configuration for the provider
    ///
    /// # Example
    ///
    /// ```
    /// use vector_client::embedding::{EmbeddingConfig, OpenAICompatibleProvider};
    ///
    /// let config = EmbeddingConfig::new(
    ///     "https://api.openai.com/v1/embeddings",
    ///     "text-embedding-3-small"
    /// ).with_api_key("sk-xxx").with_dimension(1536);
    ///
    /// let provider = OpenAICompatibleProvider::new(config).expect("failed");
    /// ```
    pub fn new(config: EmbeddingConfig) -> Result<Self> {
        config.validate()?;

        let client = Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .build()
            .map_err(|e| EmbeddingError::Config(format!("Failed to create HTTP client: {}", e)))?;

        // Dimension must be provided by user via config
        let dimension = config.dimension.ok_or_else(|| {
            EmbeddingError::Config(
                "Dimension must be specified in EmbeddingConfig. \
                 Use EmbeddingConfig::with_dimension() to set it."
                    .to_string(),
            )
        })?;

        // Create preprocessor based on config
        let preprocessor = PreprocessorImpl::from_config(&config.preprocessor);

        Ok(Self {
            client,
            config,
            preprocessor,
            dimension,
        })
    }

    /// Build embedding request
    fn build_request(&self, texts: &[&str]) -> EmbeddingRequest {
        let input = texts
            .iter()
            .map(|&t| self.preprocessor.preprocess(t))
            .collect();
        EmbeddingRequest {
            model: self.config.model.clone(),
            input,
            encoding_format: Some("float".to_string()),
        }
    }

    /// Parse response
    fn parse_response(&self, response: EmbeddingResponse) -> Result<Vec<Vec<f32>>> {
        let mut embeddings = vec![Vec::new(); response.data.len()];

        for item in response.data {
            if item.index >= embeddings.len() {
                return Err(EmbeddingError::InvalidResponse(
                    "Invalid index in response".to_string(),
                ));
            }
            embeddings[item.index] = item.embedding;
        }

        Ok(embeddings)
    }

    /// Create embeddings for texts
    ///
    /// This method applies the configured preprocessor to all texts before embedding.
    pub async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let request = self.build_request(texts);

        let mut req_builder = self.client.post(&self.config.base_url).json(&request);

        // Add authentication if API key is provided
        if let Some(api_key) = &self.config.api_key {
            req_builder = req_builder.header("Authorization", format!("Bearer {}", api_key));
        }

        let response = req_builder.send().await.map_err(|e| {
            if e.is_timeout() {
                EmbeddingError::Http("Request timeout".to_string())
            } else if e.is_connect() {
                EmbeddingError::Http("Connection failed".to_string())
            } else {
                EmbeddingError::Http(e.to_string())
            }
        })?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            return Err(EmbeddingError::Api(format!(
                "API error {}: {}",
                status, error_text
            )));
        }

        let embedding_response: EmbeddingResponse = response.json().await.map_err(|e| {
            EmbeddingError::InvalidResponse(format!("Failed to parse response: {}", e))
        })?;

        if let Some(usage) = embedding_response.usage() {
            debug!(
                "Embedding usage: prompt_tokens={}, total_tokens={}",
                usage.prompt_tokens(),
                usage.total_tokens()
            );
        }

        self.parse_response(embedding_response)
    }

    /// Embed a single text
    pub async fn embed_one(&self, text: &str) -> Result<Vec<f32>> {
        let result = self.embed(&[text]).await?;
        result
            .into_iter()
            .next()
            .ok_or_else(|| EmbeddingError::InvalidResponse("No embedding returned".to_string()))
    }

    /// Get the configuration
    pub fn config(&self) -> &EmbeddingConfig {
        &self.config
    }

    /// Get the preprocessor
    pub fn preprocessor(&self) -> &PreprocessorImpl {
        &self.preprocessor
    }
}

#[async_trait::async_trait]
impl EmbeddingProvider for OpenAICompatibleProvider {
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        self.embed(texts).await
    }

    fn dimension(&self) -> usize {
        self.dimension
    }

    fn model_name(&self) -> &str {
        &self.config.model
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedding::preprocessor::NomicTaskType;
    use crate::embedding::PreprocessorConfig;

    #[test]
    fn test_create_provider() {
        let config = EmbeddingConfig::new("http://localhost:11434/api/embeddings", "all-minilm")
            .with_dimension(384);
        let provider = OpenAICompatibleProvider::new(config);
        assert!(provider.is_ok());
    }

    #[test]
    fn test_create_provider_with_api_key() {
        let config = EmbeddingConfig::new(
            "https://api.openai.com/v1/embeddings",
            "text-embedding-3-small",
        )
        .with_api_key("sk-test")
        .with_dimension(1536);
        let provider = OpenAICompatibleProvider::new(config);
        assert!(provider.is_ok());
    }

    #[test]
    fn test_create_provider_with_preprocessor() {
        let config =
            EmbeddingConfig::new("http://localhost:11434/api/embeddings", "nomic-embed-text")
                .with_preprocessor(PreprocessorConfig::Nomic {
                    task_type: NomicTaskType::SearchDocument,
                })
                .with_dimension(768);
        let provider = OpenAICompatibleProvider::new(config);
        assert!(provider.is_ok());
    }

    #[test]
    fn test_dimension_required() {
        let config =
            EmbeddingConfig::new("http://localhost:11434/api/embeddings", "all-MiniLM-L6-v2");
        let result = OpenAICompatibleProvider::new(config);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Dimension must be specified"));
    }

    #[test]
    fn test_custom_dimension() {
        let config = EmbeddingConfig::new("http://localhost:11434/api/embeddings", "custom-model")
            .with_dimension(512);
        let provider = OpenAICompatibleProvider::new(config).expect("create failed");
        assert_eq!(provider.dimension(), 512);
    }

    #[test]
    fn test_build_request_single_text() {
        let config =
            EmbeddingConfig::new("http://example.com/embeddings", "test-model").with_dimension(128);
        let provider = OpenAICompatibleProvider::new(config).unwrap();
        let req = provider.build_request(&["hello world"]);
        assert_eq!(req.model, "test-model");
        assert_eq!(req.input, vec!["hello world".to_string()]);
        assert_eq!(req.encoding_format, Some("float".to_string()));
    }

    #[test]
    fn test_build_request_multiple_texts() {
        let config =
            EmbeddingConfig::new("http://example.com/embeddings", "test-model").with_dimension(128);
        let provider = OpenAICompatibleProvider::new(config).unwrap();
        let req = provider.build_request(&["a", "b", "c"]);
        assert_eq!(req.input.len(), 3);
        assert_eq!(req.input[0], "a");
        assert_eq!(req.input[2], "c");
    }

    #[test]
    fn test_build_request_with_preprocessor() {
        let config = EmbeddingConfig::new("http://example.com/embeddings", "nomic-embed")
            .with_dimension(768)
            .with_preprocessor(PreprocessorConfig::Nomic {
                task_type: NomicTaskType::SearchQuery,
            });
        let provider = OpenAICompatibleProvider::new(config).unwrap();
        let req = provider.build_request(&["rust"]);
        assert_eq!(req.input[0], "search_query: rust");
    }

    #[test]
    fn test_parse_response_ordered() {
        let response = EmbeddingResponse {
            data: vec![
                EmbeddingData {
                    index: 0,
                    embedding: vec![1.0, 2.0],
                },
                EmbeddingData {
                    index: 1,
                    embedding: vec![3.0, 4.0],
                },
            ],
            usage: None,
        };
        let config = EmbeddingConfig::new("http://example.com", "model").with_dimension(2);
        let provider = OpenAICompatibleProvider::new(config).unwrap();
        let result = provider.parse_response(response).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0], vec![1.0, 2.0]);
        assert_eq!(result[1], vec![3.0, 4.0]);
    }

    #[test]
    fn test_parse_response_unordered() {
        let response = EmbeddingResponse {
            data: vec![
                EmbeddingData {
                    index: 1,
                    embedding: vec![3.0],
                },
                EmbeddingData {
                    index: 0,
                    embedding: vec![1.0],
                },
            ],
            usage: None,
        };
        let config = EmbeddingConfig::new("http://example.com", "model").with_dimension(1);
        let provider = OpenAICompatibleProvider::new(config).unwrap();
        let result = provider.parse_response(response).unwrap();
        assert_eq!(result[0], vec![1.0]);
        assert_eq!(result[1], vec![3.0]);
    }

    #[test]
    fn test_parse_response_invalid_index() {
        let response = EmbeddingResponse {
            data: vec![EmbeddingData {
                index: 5,
                embedding: vec![1.0],
            }],
            usage: None,
        };
        let config = EmbeddingConfig::new("http://example.com", "model").with_dimension(1);
        let provider = OpenAICompatibleProvider::new(config).unwrap();
        let result = provider.parse_response(response);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid index"));
    }

    #[test]
    fn test_create_preprocessor_none() {
        let config = EmbeddingConfig::new("http://example.com", "model").with_dimension(128);
        let provider = OpenAICompatibleProvider::new(config).unwrap();
        let result = provider.preprocessor().preprocess("text");
        assert_eq!(result, "text");
    }

    #[test]
    fn test_create_preprocessor_prefix() {
        let config = EmbeddingConfig::new("http://example.com", "model")
            .with_dimension(128)
            .with_preprocessor(PreprocessorConfig::Prefix {
                prefix: ">> ".into(),
            });
        let provider = OpenAICompatibleProvider::new(config).unwrap();
        let result = provider.preprocessor().preprocess("text");
        assert_eq!(result, ">> text");
    }

    #[test]
    fn test_create_preprocessor_template() {
        let config = EmbeddingConfig::new("http://example.com", "model")
            .with_dimension(128)
            .with_preprocessor(PreprocessorConfig::Template {
                template: "[{{text}}]".into(),
            });
        let provider = OpenAICompatibleProvider::new(config).unwrap();
        let result = provider.preprocessor().preprocess("hello");
        assert_eq!(result, "[hello]");
    }

    #[test]
    fn test_model_name() {
        let config =
            EmbeddingConfig::new("http://example.com", "my-special-model").with_dimension(128);
        let provider = OpenAICompatibleProvider::new(config).unwrap();
        assert_eq!(provider.model_name(), "my-special-model");
    }

    #[test]
    fn test_provider_debug() {
        let config = EmbeddingConfig::new("http://example.com", "debug-model").with_dimension(256);
        let provider = OpenAICompatibleProvider::new(config).unwrap();
        let debug_str = format!("{:?}", provider);
        assert!(debug_str.contains("debug-model"));
        assert!(debug_str.contains("example.com"));
    }

    #[test]
    fn test_stella_preprocessor() {
        let config = EmbeddingConfig::new("http://example.com", "stella")
            .with_dimension(768)
            .with_preprocessor(PreprocessorConfig::Stella {
                task_type: crate::embedding::StellaTaskType::S2PQuery,
            });
        let provider = OpenAICompatibleProvider::new(config).unwrap();
        let result = provider.preprocessor().preprocess("test");
        assert!(result.contains("Instruct"));
        assert!(result.contains("test"));
    }
}
