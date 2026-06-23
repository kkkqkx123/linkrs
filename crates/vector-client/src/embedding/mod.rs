//! Embedding Service for vector search
//!
//! Provides text-to-vector embedding capabilities using various providers:
//! - HTTP-based: OpenAI, Gemini, Azure, Ollama, and compatible endpoints

mod config;
mod error;
mod preprocessor;
mod provider;
mod providers;
mod service;

pub use config::EmbeddingConfig;
pub use error::EmbeddingError;
pub use preprocessor::{NomicTaskType, PreprocessorConfig, PreprocessorImpl, StellaTaskType};
pub use provider::EmbeddingProvider;
pub use service::EmbeddingService;

// Re-export providers for advanced usage
pub use providers::OpenAICompatibleProvider;
