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

    #[error(
        "Vector engine is disabled by configuration; \
         enable the vector engine (or check VectorEngineState) before issuing vector operations"
    )]
    EngineDisabled,

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

/// Classification of vector backend errors for retry policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VectorErrorKind {
    /// Transient failure that should be retried with exponential backoff.
    Retryable,
    /// Permanent failure that will never succeed (e.g. dimension mismatch).
    NonRetryable,
    /// Authentication / authorization failure that should pause delivery.
    Auth,
}

impl VectorErrorKind {
    /// Classify a [`VectorError`] into a retry bucket.
    pub fn classify(err: &VectorError) -> Self {
        match err {
            VectorError::DimensionMismatch { .. }
            | VectorError::InvalidVector(_)
            | VectorError::InvalidPointId(_)
            | VectorError::ConfigError(_)
            | VectorError::IndexAlreadyExists(_)
            | VectorError::CollectionNotFound(_)
            | VectorError::IndexNotFound(_) => Self::NonRetryable,
            VectorError::ConnectionFailed(_)
            | VectorError::EngineUnavailable(_)
            | VectorError::Timeout
            | VectorError::QdrantError(_)
            | VectorError::IndexCorrupted(_)
            | VectorError::Locked(_)
            | VectorError::Internal(_) => Self::Retryable,
            VectorError::Cancelled
            | VectorError::EmbeddingError(_)
            | VectorError::EngineNotFound { .. } => Self::NonRetryable,
        }
    }

    /// Classify a stringified error (used in `SyncManager::retry_outbox_sync`
    /// where the error has been erased to `String`).
    pub fn classify_str(msg: &str) -> Self {
        let lower = msg.to_ascii_lowercase();
        if lower.contains("dimensionmismatch")
            || lower.contains("dimension mismatch")
            || lower.contains("invalidconfig")
            || lower.contains("invalid vector")
            || lower.contains("invalid point id")
            || lower.contains("invalidargument")
            || lower.contains("configerror")
        {
            return Self::NonRetryable;
        }
        if lower.contains("auth")
            || lower.contains("unauthorized")
            || lower.contains("forbidden")
            || lower.contains("permission")
            || lower.contains("apikey")
            || lower.contains("api key")
        {
            return Self::Auth;
        }
        if lower.contains("engine is disabled") || lower.contains("enginedisabled") {
            return Self::Retryable;
        }
        if lower.contains("timeout")
            || lower.contains("unavailable")
            || lower.contains("connection")
            || lower.contains("transport")
            || lower.contains("deadline")
            || lower.contains("temporarily")
            || lower.contains("rate limit")
            || lower.contains("qdrant")
        {
            return Self::Retryable;
        }
        // Default to NonRetryable for unknowns to avoid infinite retries on
        // validation failures; callers may still retry with capped attempts.
        // However for safety in outbox path treat unknown as Retryable unless
        // it clearly looks like a validation failure.
        if lower.contains("not found") || lower.contains("already exists") {
            return Self::NonRetryable;
        }
        Self::Retryable
    }
}

impl VectorError {
    /// Convenience wrapper for [`VectorErrorKind::classify`].
    pub fn kind(&self) -> VectorErrorKind {
        VectorErrorKind::classify(self)
    }
}

impl VectorCoordinatorError {
    /// Classify a coordinator error into a retry bucket.
    pub fn kind(&self) -> VectorErrorKind {
        match self {
            VectorCoordinatorError::Vector(err) => VectorErrorKind::classify(err),
            VectorCoordinatorError::EngineDisabled => VectorErrorKind::Retryable,
            VectorCoordinatorError::CollectionConfigConflict { .. }
            | VectorCoordinatorError::FieldNotIndexed { .. }
            | VectorCoordinatorError::InvalidOperation(_)
            | VectorCoordinatorError::EmbeddingServiceNotAvailable
            | VectorCoordinatorError::EmbeddingError(_)
            | VectorCoordinatorError::NotInitialized
            | VectorCoordinatorError::ShuttingDown
            | VectorCoordinatorError::BufferError(_) => VectorErrorKind::NonRetryable,
            VectorCoordinatorError::IndexCreationFailed { .. }
            | VectorCoordinatorError::IndexDropFailed { .. }
            | VectorCoordinatorError::IndexRebuildFailed(_)
            | VectorCoordinatorError::VertexChangeFailed(_)
            | VectorCoordinatorError::SpaceNotFound(_)
            | VectorCoordinatorError::TagNotFound(_)
            | VectorCoordinatorError::Sync(_)
            | VectorCoordinatorError::Internal(_) => VectorErrorKind::Retryable,
        }
    }

    /// Classify from the display string of the error.
    pub fn kind_str(msg: &str) -> VectorErrorKind {
        VectorErrorKind::classify_str(msg)
    }
}

pub type VectorResult<T> = std::result::Result<T, VectorError>;
pub type VectorCoordinatorResult<T> = std::result::Result<T, VectorCoordinatorError>;

#[cfg(feature = "vector")]
impl From<vector_search::VectorSearchError> for VectorError {
    fn from(err: vector_search::VectorSearchError) -> Self {
        match err {
            vector_search::VectorSearchError::CollectionNotFound(name) => {
                VectorError::IndexNotFound(name)
            }
            vector_search::VectorSearchError::CollectionAlreadyExists(name) => {
                VectorError::IndexAlreadyExists(name)
            }
            vector_search::VectorSearchError::CollectionIncomplete { dir, file } => {
                VectorError::IndexCorrupted(format!(
                    "collection incomplete (missing {}): {}",
                    file,
                    dir.display()
                ))
            }
            vector_search::VectorSearchError::InvalidCollectionName(name) => {
                VectorError::ConfigError(format!("Invalid collection name: {}", name))
            }
            vector_search::VectorSearchError::InvalidConfig(msg) => VectorError::ConfigError(msg),
            vector_search::VectorSearchError::InvalidVectorDimension { expected, actual } => {
                VectorError::DimensionMismatch { expected, actual }
            }
            vector_search::VectorSearchError::InvalidPointId(id) => VectorError::InvalidPointId(id),
            vector_search::VectorSearchError::NonFiniteElement(index) => {
                VectorError::InvalidVector(format!("non-finite element at index {}", index))
            }
            vector_search::VectorSearchError::UnsupportedMetric(metric) => {
                VectorError::ConfigError(format!("metric not supported: {:?}", metric))
            }
            vector_search::VectorSearchError::Filter(msg) => VectorError::InvalidVector(msg),
            vector_search::VectorSearchError::CorruptData(msg) => VectorError::IndexCorrupted(msg),
            vector_search::VectorSearchError::Io(e) => VectorError::Internal(e.to_string()),
            vector_search::VectorSearchError::Serialization(e) => {
                VectorError::Internal(e.to_string())
            }
            vector_search::VectorSearchError::Json(e) => VectorError::Internal(e.to_string()),
            vector_search::VectorSearchError::Internal(msg) => VectorError::Internal(msg),
        }
    }
}

#[cfg(feature = "vector")]
impl From<vector_search::VectorSearchError> for VectorCoordinatorError {
    fn from(err: vector_search::VectorSearchError) -> Self {
        VectorCoordinatorError::Vector(VectorError::from(err))
    }
}

#[cfg(feature = "vector")]
impl From<vector_search::VectorEngineError> for VectorCoordinatorError {
    fn from(err: vector_search::VectorEngineError) -> Self {
        match err {
            vector_search::VectorEngineError::Local(msg) => {
                VectorCoordinatorError::Vector(VectorError::Internal(msg))
            }
            vector_search::VectorEngineError::Remote(msg) => {
                VectorCoordinatorError::Vector(VectorError::QdrantError(msg))
            }
            vector_search::VectorEngineError::Internal(msg) => {
                VectorCoordinatorError::Vector(VectorError::Internal(msg))
            }
            vector_search::VectorEngineError::NotSupported(op) => VectorCoordinatorError::Vector(
                VectorError::ConfigError(format!("Operation not supported: {}", op)),
            ),
        }
    }
}

#[cfg(feature = "vector-qdrant")]
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

#[cfg(feature = "vector-qdrant")]
impl From<vector_client::VectorClientError> for VectorCoordinatorError {
    fn from(err: vector_client::VectorClientError) -> Self {
        VectorCoordinatorError::Vector(VectorError::from(err))
    }
}
