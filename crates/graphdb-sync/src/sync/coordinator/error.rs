use thiserror::Error;

use crate::search::error::SearchError;
use crate::sync::SyncError;

#[derive(Error, Debug, Clone)]
pub enum FulltextError {
    #[error("Index not found: {0}")]
    IndexNotFound(String),

    #[error("Index already exists: {0}")]
    IndexAlreadyExists(String),

    #[error("Engine not found for space {space_id}, tag {tag_name}, field {field_name}")]
    EngineNotFound {
        space_id: u64,
        tag_name: String,
        field_name: String,
    },

    #[error("Engine unavailable: {0}")]
    EngineUnavailable(String),

    #[error("Index corrupted: {0}")]
    IndexCorrupted(String),

    #[error("Query parse error: {0}")]
    QueryParseError(String),

    #[error("Invalid document ID: {0}")]
    InvalidDocId(String),

    #[error("Index configuration error: {0}")]
    ConfigError(String),

    #[error("Index operation timeout")]
    Timeout,

    #[error("Index is locked: {0}")]
    Locked(String),

    #[error("Index operation cancelled")]
    Cancelled,

    #[error("Internal error: {0}")]
    Internal(String),
}

#[derive(Error, Debug, Clone)]
pub enum CoordinatorError {
    #[error("Fulltext index error: {0}")]
    Fulltext(#[from] FulltextError),

    #[error("Sync error: {0}")]
    Sync(String),

    #[error("Index creation failed for {tag_name}.{field_name}: {reason}")]
    IndexCreationFailed {
        tag_name: String,
        field_name: String,
        reason: String,
    },

    #[error("Index drop failed for {tag_name}.{field_name}: {reason}")]
    IndexDropFailed {
        tag_name: String,
        field_name: String,
        reason: String,
    },

    #[error("Index rebuild failed: {0}")]
    IndexRebuildFailed(String),

    #[error("Vertex change processing failed: {0}")]
    VertexChangeFailed(String),

    #[error("Space not found: {0}")]
    SpaceNotFound(u64),

    #[error("Tag not found: {0}")]
    TagNotFound(String),

    #[error("Field not indexed: {tag_name}.{field_name}")]
    FieldNotIndexed {
        tag_name: String,
        field_name: String,
    },

    #[error("Coordinator not initialized")]
    NotInitialized,

    #[error("Coordinator is shutting down")]
    ShuttingDown,

    #[error("Invalid operation: {0}")]
    InvalidOperation(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

pub type FulltextResult<T> = std::result::Result<T, FulltextError>;
pub type CoordinatorResult<T> = std::result::Result<T, CoordinatorError>;

impl From<SearchError> for FulltextError {
    fn from(err: SearchError) -> Self {
        match err {
            SearchError::EngineNotFound(msg) => FulltextError::EngineNotFound {
                space_id: 0,
                tag_name: String::new(),
                field_name: msg,
            },
            SearchError::IndexNotFound(msg) => FulltextError::IndexNotFound(msg),
            SearchError::IndexAlreadyExists(msg) => FulltextError::IndexAlreadyExists(msg),
            SearchError::SpaceNotFound(space_id) => {
                FulltextError::Internal(format!("Space not found: {}", space_id))
            }
            SearchError::TagNotFound(tag) => {
                FulltextError::Internal(format!("Tag not found: {}", tag))
            }
            SearchError::FieldNotFound(field) => {
                FulltextError::Internal(format!("Field not found: {}", field))
            }
            SearchError::EngineUnavailable => {
                FulltextError::EngineUnavailable("engine unavailable".to_string())
            }
            SearchError::IndexCorrupted(msg) => FulltextError::IndexCorrupted(msg),
            #[cfg(feature = "fulltext-search")]
            SearchError::TantivyError(e) => FulltextError::Internal(e.to_string()),
            SearchError::IoError(e) => FulltextError::Internal(e.to_string()),
            SearchError::SerializationError(msg) => {
                FulltextError::Internal(format!("Serialization error: {}", msg))
            }
            SearchError::ConfigError(msg) => FulltextError::ConfigError(msg),
            SearchError::QueryParseError(msg) => FulltextError::QueryParseError(msg),
            SearchError::InvalidDocId(msg) => FulltextError::InvalidDocId(msg),
            SearchError::Internal(msg) => FulltextError::Internal(msg),
        }
    }
}

impl From<SyncError> for CoordinatorError {
    fn from(err: SyncError) -> Self {
        CoordinatorError::Sync(err.to_string())
    }
}

impl From<SearchError> for CoordinatorError {
    fn from(err: SearchError) -> Self {
        CoordinatorError::Fulltext(FulltextError::from(err))
    }
}
