use thiserror::Error;

#[derive(Error, Debug)]
pub enum SearchError {
    #[error("Engine not found: {0}")]
    EngineNotFound(String),

    #[error("Index not found: {0}")]
    IndexNotFound(String),

    #[error("Index already exists: {0}")]
    IndexAlreadyExists(String),

    #[error("Space not found: {0}")]
    SpaceNotFound(u64),

    #[error("Tag not found: {0}")]
    TagNotFound(String),

    #[error("Field not found: {0}")]
    FieldNotFound(String),

    #[error("Engine unavailable")]
    EngineUnavailable,

    #[error("Index corrupted: {0}")]
    IndexCorrupted(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[cfg(feature = "fulltext")]
    #[error("Tantivy error: {0}")]
    TantivyError(#[from] tantivy::TantivyError),

    #[error("Serialization error: {0}")]
    SerializationError(String),

    #[error("Config error: {0}")]
    ConfigError(String),

    #[error("Query parse error: {0}")]
    QueryParseError(String),

    #[error("Invalid doc ID format: {0}")]
    InvalidDocId(String),

    #[error("Rebuild busy: {0}")]
    RebuildBusy(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

pub type Result<T> = std::result::Result<T, SearchError>;

impl linkrs_core::error::search::ClassifySearchError for SearchError {
    fn classify(&self) -> linkrs_core::error::search::SearchErrorKind {
        use linkrs_core::error::search::SearchErrorKind;
        match self {
            Self::IndexNotFound(_) => SearchErrorKind::IndexNotFound,
            Self::IndexAlreadyExists(_) => SearchErrorKind::IndexAlreadyExists,
            Self::EngineUnavailable => SearchErrorKind::EngineUnavailable,
            Self::IndexCorrupted(_) => SearchErrorKind::IndexCorrupted,
            Self::RebuildBusy(_) => SearchErrorKind::RebuildBusy,
            Self::QueryParseError(_) => SearchErrorKind::QueryInvalid,
            Self::SpaceNotFound(_) => SearchErrorKind::SpaceNotFound,
            Self::TagNotFound(_) => SearchErrorKind::SchemaNotFound,
            Self::FieldNotFound(_) => SearchErrorKind::SchemaNotFound,
            Self::IoError(_) => SearchErrorKind::Io,
            Self::SerializationError(_) => SearchErrorKind::Serialization,
            Self::ConfigError(_) => SearchErrorKind::Config,
            _ => SearchErrorKind::Internal,
        }
    }
}

impl From<SearchError> for linkrs_core::error::DBError {
    fn from(e: SearchError) -> Self {
        Self::search(e.to_string())
    }
}
