//! Unified search error classification for fulltext and vector backends.
//!
//! Both search paths need a common error vocabulary so the fusion layer,
//! API layer, and observability code can treat failures uniformly without
//! knowing which engine produced them. This module defines the classification
//! trait; engine-specific implementations live in `linkrs-api` to respect
//! the dependency DAG.

use super::codes::ErrorCode;

/// Classification of errors that any secondary search backend can produce.
///
/// This is not a replacement for engine-specific error types — those carry
/// detail strings and source chains. Instead, `SearchErrorKind` is the
/// normalised category that management and observability code matches on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchErrorKind {
    /// The requested index does not exist.
    IndexNotFound,
    /// An index with the same name already exists.
    IndexAlreadyExists,
    /// The engine is not available (feature disabled or backend down).
    EngineUnavailable,
    /// The index is corrupted or unreadable.
    IndexCorrupted,
    /// A rebuild is in progress and the operation conflicts with it.
    RebuildBusy,
    /// The query expression is invalid for this engine.
    QueryInvalid,
    /// The space does not exist.
    SpaceNotFound,
    /// The schema (tag or edge type) does not exist.
    SchemaNotFound,
    /// I/O failure during index operations.
    Io,
    /// Serialization or deserialization failure.
    Serialization,
    /// Configuration error.
    Config,
    /// Internal engine error (catch-all).
    Internal,
}

impl SearchErrorKind {
    /// Map to the external error code used in client responses.
    pub fn error_code(&self) -> ErrorCode {
        match self {
            Self::IndexNotFound => ErrorCode::ResourceNotFound,
            Self::IndexAlreadyExists => ErrorCode::ResourceAlreadyExists,
            Self::EngineUnavailable => ErrorCode::ServiceUnavailable,
            Self::IndexCorrupted => ErrorCode::InternalError,
            Self::RebuildBusy => ErrorCode::Conflict,
            Self::QueryInvalid => ErrorCode::ValidationError,
            Self::SpaceNotFound => ErrorCode::ResourceNotFound,
            Self::SchemaNotFound => ErrorCode::ResourceNotFound,
            Self::Io => ErrorCode::InternalError,
            Self::Serialization => ErrorCode::InternalError,
            Self::Config => ErrorCode::InvalidInput,
            Self::Internal => ErrorCode::InternalError,
        }
    }

    /// Human-readable category name for logging and metrics.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::IndexNotFound => "index_not_found",
            Self::IndexAlreadyExists => "index_already_exists",
            Self::EngineUnavailable => "engine_unavailable",
            Self::IndexCorrupted => "index_corrupted",
            Self::RebuildBusy => "rebuild_busy",
            Self::QueryInvalid => "query_invalid",
            Self::SpaceNotFound => "space_not_found",
            Self::SchemaNotFound => "schema_not_found",
            Self::Io => "io",
            Self::Serialization => "serialization",
            Self::Config => "config",
            Self::Internal => "internal",
        }
    }
}

impl std::fmt::Display for SearchErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Classification trait: engine error types implement this to produce
/// the unified [`SearchErrorKind`].
///
/// Implementations live in `linkrs-api` to respect the dependency DAG
/// (`linkrs-core` cannot depend on `linkrs-fulltext` or `linkrs-sync`).
pub trait ClassifySearchError {
    fn classify(&self) -> SearchErrorKind;
}
