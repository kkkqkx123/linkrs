use crate::embedding::EmbeddingError;
use thiserror::Error;

pub type Result<T> = std::result::Result<T, VectorClientError>;

#[derive(Error, Debug)]
pub enum VectorClientError {
    #[error("Collection '{0}' not found")]
    CollectionNotFound(String),

    #[error("Collection '{0}' already exists")]
    CollectionAlreadyExists(String),

    #[error("Index '{0}' already exists")]
    IndexAlreadyExists(String),

    #[error("Point '{0}' not found in collection '{1}'")]
    PointNotFound(String, String),

    #[error("Invalid vector dimension: expected {expected}, got {actual}")]
    InvalidVectorDimension { expected: usize, actual: usize },

    #[error("Invalid collection name: '{0}'")]
    InvalidCollectionName(String),

    #[error("Invalid point ID: '{0}'")]
    InvalidPointId(String),

    #[error("Connection failed: {0}")]
    ConnectionFailed(String),

    #[error("Operation timeout: {0}")]
    Timeout(String),

    #[error("Invalid configuration: {0}")]
    InvalidConfig(String),

    #[error("Search error: {0}")]
    SearchError(String),

    #[error("Upsert error: {0}")]
    UpsertError(String),

    #[error("Delete error: {0}")]
    DeleteError(String),

    #[error("Payload error: {0}")]
    PayloadError(String),

    #[error("Filter error: {0}")]
    FilterError(String),

    #[error("Health check failed: {0}")]
    HealthCheckFailed(String),

    #[error("Engine not initialized")]
    EngineNotInitialized,

    #[error("Engine '{0}' is not available (feature not enabled)")]
    EngineNotAvailable(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),

    #[error("Qdrant gRPC error: {0}")]
    QdrantGrpcError(String),

    #[error("Qdrant HTTP error: status={status}, message={message}")]
    QdrantHttpError { status: u16, message: String },

    #[error("Operation not supported by this engine: {0}")]
    NotSupported(String),

    #[error("Internal error: {0}")]
    InternalError(String),
}

impl VectorClientError {
    pub fn is_not_found(&self) -> bool {
        matches!(
            self,
            VectorClientError::CollectionNotFound(_) | VectorClientError::PointNotFound(_, _)
        )
    }

    pub fn is_connection_error(&self) -> bool {
        matches!(
            self,
            VectorClientError::ConnectionFailed(_)
                | VectorClientError::Timeout(_)
                | VectorClientError::HealthCheckFailed(_)
        )
    }
}

impl From<EmbeddingError> for VectorClientError {
    fn from(err: EmbeddingError) -> Self {
        VectorClientError::InternalError(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_not_found_collection_not_found() {
        let err = VectorClientError::CollectionNotFound("test".into());
        assert!(err.is_not_found());
    }

    #[test]
    fn test_is_not_found_point_not_found() {
        let err = VectorClientError::PointNotFound("p1".into(), "c1".into());
        assert!(err.is_not_found());
    }

    #[test]
    fn test_is_not_found_other() {
        let err = VectorClientError::ConnectionFailed("timeout".into());
        assert!(!err.is_not_found());
    }

    #[test]
    fn test_is_connection_error_connection_failed() {
        let err = VectorClientError::ConnectionFailed("refused".into());
        assert!(err.is_connection_error());
    }

    #[test]
    fn test_is_connection_error_timeout() {
        let err = VectorClientError::Timeout("slow".into());
        assert!(err.is_connection_error());
    }

    #[test]
    fn test_is_connection_error_health_check() {
        let err = VectorClientError::HealthCheckFailed("down".into());
        assert!(err.is_connection_error());
    }

    #[test]
    fn test_is_connection_error_other() {
        let err = VectorClientError::SearchError("bad query".into());
        assert!(!err.is_connection_error());
    }

    #[test]
    fn test_display_collection_not_found() {
        let err = VectorClientError::CollectionNotFound("my_coll".into());
        assert_eq!(err.to_string(), "Collection 'my_coll' not found");
    }

    #[test]
    fn test_display_invalid_vector_dimension() {
        let err = VectorClientError::InvalidVectorDimension {
            expected: 384,
            actual: 128,
        };
        assert!(err.to_string().contains("384"));
        assert!(err.to_string().contains("128"));
    }

    #[test]
    fn test_display_qdrant_http_error() {
        let err = VectorClientError::QdrantHttpError {
            status: 400,
            message: "bad request".into(),
        };
        let s = err.to_string();
        assert!(s.contains("400"));
        assert!(s.contains("bad request"));
    }

    #[test]
    fn test_display_engine_not_available() {
        let err = VectorClientError::EngineNotAvailable("feature disabled".into());
        assert!(err.to_string().contains("feature disabled"));
    }
}
