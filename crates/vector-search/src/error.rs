//! Vector search engine error types.

use crate::types::DistanceMetric;

/// Errors produced by the local vector search engine.
#[derive(Debug, thiserror::Error)]
pub enum VectorSearchError {
    #[error("collection not found: {0}")]
    CollectionNotFound(String),
    #[error("collection already exists: {0}")]
    CollectionAlreadyExists(String),
    #[error("invalid collection name: {0}")]
    InvalidCollectionName(String),
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),
    #[error("invalid vector dimension: expected {expected}, got {actual}")]
    InvalidVectorDimension { expected: usize, actual: usize },
    #[error("invalid point id: {0}")]
    InvalidPointId(String),
    #[error("non-finite vector element at index {0}")]
    NonFiniteElement(usize),
    #[error("metric not supported by local engine: {0:?}")]
    UnsupportedMetric(DistanceMetric),
    #[error("filter error: {0}")]
    Filter(String),
    #[error("corrupt data: {0}")]
    CorruptData(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization error: {0}")]
    Serialization(#[from] postcard::Error),
    #[error("json serialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("internal error: {0}")]
    Internal(String),
}

/// Result type alias for the vector search engine.
pub type Result<T> = std::result::Result<T, VectorSearchError>;
