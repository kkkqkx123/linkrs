//! Provider implementations
//!
//! This module contains concrete provider implementations:
//! - HTTP-based providers (OpenAI, Gemini, Ollama, etc.)

pub mod http;

pub use http::openai_compatible_provider::OpenAICompatibleProvider;
