use thiserror::Error;

#[derive(Error, Debug, Clone)]
pub enum VectorError {
    #[error("Vector index not found: {0}")]
    IndexNotFound(String),

    #[error("Vector index already exists: {0}")]
    IndexAlreadyExists(String),

    #[error("Vector engine not found for space {space_id}, tag {tag_name}, field {field_name}")]
    EngineNotFound {
        space_id: u64,
        tag_name: String,
        field_name: String,
    },

    #[error("Vector engine unavailable: {0}")]
    EngineUnavailable(String),

    #[error("Vector index corrupted: {0}")]
    IndexCorrupted(String),

    #[error("Qdrant engine error: {0}")]
    QdrantError(String),

    #[error("Vector dimension mismatch: expected {expected}, got {actual}")]
    DimensionMismatch { expected: usize, actual: usize },

    #[error("Invalid vector: {0}")]
    InvalidVector(String),

    #[error("Invalid point ID: {0}")]
    InvalidPointId(String),

    #[error("Vector index configuration error: {0}")]
    ConfigError(String),

    #[error("Vector search timeout")]
    Timeout,

    #[error("Vector index is locked: {0}")]
    Locked(String),

    #[error("Vector operation cancelled")]
    Cancelled,

    #[error("Embedding service error: {0}")]
    EmbeddingError(String),

    #[error("Connection failed: {0}")]
    ConnectionFailed(String),

    #[error("Collection not found: {0}")]
    CollectionNotFound(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

#[derive(Error, Debug, Clone)]
pub enum VectorCoordinatorError {
    #[error("Vector index error: {0}")]
    Vector(#[from] VectorError),

    #[error("Sync error: {0}")]
    Sync(String),

    #[error("Vector index creation failed for {tag_name}.{field_name}: {reason}")]
    IndexCreationFailed {
        tag_name: String,
        field_name: String,
        reason: String,
    },

    #[error("Vector index drop failed for {tag_name}.{field_name}: {reason}")]
    IndexDropFailed {
        tag_name: String,
        field_name: String,
        reason: String,
    },

    #[error("Vector index rebuild failed: {0}")]
    IndexRebuildFailed(String),

    #[error("Vertex change processing failed: {0}")]
    VertexChangeFailed(String),

    #[error("Space not found: {0}")]
    SpaceNotFound(u64),

    #[error("Tag not found: {0}")]
    TagNotFound(String),

    #[error("Field not vector indexed: {tag_name}.{field_name}")]
    FieldNotIndexed {
        tag_name: String,
        field_name: String,
    },

    #[error("Vector coordinator not initialized")]
    NotInitialized,

    #[error("Vector coordinator is shutting down")]
    ShuttingDown,

    #[error("Invalid operation: {0}")]
    InvalidOperation(String),

    #[error("Embedding service not available")]
    EmbeddingServiceNotAvailable,

    #[error("Embedding error: {0}")]
    EmbeddingError(String),

    #[error("Collection config conflict for {collection_name}: existing {existing_size}/{existing_dist}, requested {requested_size}/{requested_dist}")]
    CollectionConfigConflict {
        collection_name: String,
        existing_size: usize,
        existing_dist: String,
        requested_size: usize,
        requested_dist: String,
    },

    #[error("Internal error: {0}")]
    Internal(String),

    #[error("Buffer error: {0}")]
    BufferError(String),
}

pub type VectorResult<T> = std::result::Result<T, VectorError>;
pub type VectorCoordinatorResult<T> = std::result::Result<T, VectorCoordinatorError>;

#[cfg(feature = "qdrant")]
impl From<vector_client::VectorClientError> for VectorError {
    fn from(err: vector_client::VectorClientError) -> Self {
        match err {
            vector_client::VectorClientError::ConnectionFailed(msg) => {
                VectorError::ConnectionFailed(msg)
            }
            vector_client::VectorClientError::CollectionNotFound(name) => {
                VectorError::CollectionNotFound(name)
            }
            vector_client::VectorClientError::CollectionAlreadyExists(name) => {
                VectorError::IndexAlreadyExists(name)
            }
            vector_client::VectorClientError::PointNotFound(id, _collection) => {
                VectorError::IndexNotFound(id)
            }
            vector_client::VectorClientError::InvalidVectorDimension { expected, actual } => {
                VectorError::DimensionMismatch { expected, actual }
            }
            vector_client::VectorClientError::InvalidCollectionName(name) => {
                VectorError::ConfigError(format!("Invalid collection name: {}", name))
            }
            vector_client::VectorClientError::InvalidPointId(id) => VectorError::InvalidPointId(id),
            vector_client::VectorClientError::Timeout(_msg) => VectorError::Timeout,
            vector_client::VectorClientError::InvalidConfig(msg) => VectorError::ConfigError(msg),
            vector_client::VectorClientError::SearchError(msg) => VectorError::QdrantError(msg),
            vector_client::VectorClientError::UpsertError(msg) => VectorError::QdrantError(msg),
            vector_client::VectorClientError::DeleteError(msg) => VectorError::QdrantError(msg),
            vector_client::VectorClientError::PayloadError(msg) => VectorError::QdrantError(msg),
            vector_client::VectorClientError::FilterError(msg) => VectorError::InvalidVector(msg),
            vector_client::VectorClientError::HealthCheckFailed(msg) => {
                VectorError::ConnectionFailed(msg)
            }
            vector_client::VectorClientError::EngineNotInitialized => {
                VectorError::EngineUnavailable("Engine not initialized".to_string())
            }
            vector_client::VectorClientError::EngineNotAvailable(name) => {
                VectorError::EngineUnavailable(format!("Engine {} not available", name))
            }
            vector_client::VectorClientError::IndexAlreadyExists(name) => {
                VectorError::IndexAlreadyExists(name)
            }
            vector_client::VectorClientError::IoError(e) => VectorError::Internal(e.to_string()),
            vector_client::VectorClientError::SerializationError(e) => {
                VectorError::Internal(e.to_string())
            }
            vector_client::VectorClientError::InternalError(msg) => VectorError::Internal(msg),
            vector_client::VectorClientError::QdrantHttpError { status, message } => {
                VectorError::QdrantError(format!("HTTP {}: {}", status, message))
            }
            vector_client::VectorClientError::QdrantGrpcError(msg) => VectorError::QdrantError(msg),
            vector_client::VectorClientError::NotSupported(op) => {
                VectorError::ConfigError(format!("Operation not supported: {}", op))
            }
        }
    }
}

#[cfg(feature = "qdrant")]
impl From<vector_client::VectorClientError> for VectorCoordinatorError {
    fn from(err: vector_client::VectorClientError) -> Self {
        VectorCoordinatorError::Vector(VectorError::from(err))
    }
}
