//! Error types for embedding service

use thiserror::Error;

/// Embedding error type
#[derive(Debug, Error)]
pub enum EmbeddingError {
    /// Configuration error
    #[error("Configuration error: {0}")]
    Config(String),

    /// HTTP request error
    #[error("HTTP error: {0}")]
    Http(String),

    /// API error response
    #[error("API error: {0}")]
    Api(String),

    /// Invalid response format
    #[error("Invalid response: {0}")]
    InvalidResponse(String),

    /// Token limit exceeded
    #[error("Token limit exceeded: {0} > {1}")]
    TokenLimitExceeded(usize, usize),

    /// Internal error
    #[error("Internal error: {0}")]
    Internal(String),
}

/// Result type alias for embedding operations
pub type Result<T> = std::result::Result<T, EmbeddingError>;

impl From<reqwest::Error> for EmbeddingError {
    fn from(err: reqwest::Error) -> Self {
        if err.is_timeout() {
            EmbeddingError::Http("Request timeout".to_string())
        } else if err.is_connect() {
            EmbeddingError::Http("Connection failed".to_string())
        } else {
            EmbeddingError::Http(err.to_string())
        }
    }
}

impl From<serde_json::Error> for EmbeddingError {
    fn from(err: serde_json::Error) -> Self {
        EmbeddingError::InvalidResponse(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_display_config() {
        let err = EmbeddingError::Config("missing api key".into());
        assert_eq!(err.to_string(), "Configuration error: missing api key");
    }

    #[test]
    fn test_display_http() {
        let err = EmbeddingError::Http("connection refused".into());
        assert_eq!(err.to_string(), "HTTP error: connection refused");
    }

    #[test]
    fn test_display_api() {
        let err = EmbeddingError::Api("rate limit exceeded".into());
        assert_eq!(err.to_string(), "API error: rate limit exceeded");
    }

    #[test]
    fn test_display_invalid_response() {
        let err = EmbeddingError::InvalidResponse("bad json".into());
        assert_eq!(err.to_string(), "Invalid response: bad json");
    }

    #[test]
    fn test_display_token_limit_exceeded() {
        let err = EmbeddingError::TokenLimitExceeded(100, 50);
        assert_eq!(err.to_string(), "Token limit exceeded: 100 > 50");
    }

    #[test]
    fn test_display_internal() {
        let err = EmbeddingError::Internal("bug".into());
        assert_eq!(err.to_string(), "Internal error: bug");
    }

    #[test]
    fn test_from_serde_json_error() {
        let serde_err = serde_json::from_str::<serde_json::Value>("invalid json").unwrap_err();
        let err: EmbeddingError = serde_err.into();
        assert!(err.to_string().contains("Invalid response"));
    }
}
