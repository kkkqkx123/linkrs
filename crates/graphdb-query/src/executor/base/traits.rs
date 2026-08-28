//! Search Provider Trait
//!
//! This module defines the common trait for search providers (fulltext, vector)
//! to enable unified management and discovery in the execution context.
//!
//! The [`SearchProvider`] trait covers metadata and lifecycle operations that
//! every search backend must support (listing, dropping, describing indexes).
//! Provider-specific operations (e.g. vector embedding, fulltext engine type)
//! remain on the concrete wrapper types ([`FulltextProvider`], [`VectorProvider`])
//! and are accessed through the `SearchContext` when the operator knows the
//! concrete backend.

use std::fmt::Debug;
use std::sync::Arc;

/// Search provider type enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SearchProviderType {
    Fulltext,
    Vector,
}

impl std::fmt::Display for SearchProviderType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SearchProviderType::Fulltext => write!(f, "FULLTEXT"),
            SearchProviderType::Vector => write!(f, "VECTOR"),
        }
    }
}

/// Minimal metadata about a search index returned by
/// [`SearchProvider::list_indexes`].
#[derive(Debug, Clone)]
pub struct IndexInfo {
    pub space_id: u64,
    pub tag_name: String,
    pub field_name: String,
    pub index_name: Option<String>,
    pub status: String,
}

/// Common trait for search providers
///
/// This trait provides a unified interface for different search backends
/// (fulltext, vector) to be discovered and managed through a common API.
/// It covers index metadata, lifecycle, and basic search operations that
/// every backend supports.
///
/// Provider-specific operations (vector embedding, fulltext engine type,
/// consistency options) remain on the concrete wrapper types and are
/// accessed through [`SearchContext`](super::SearchContext) when the
/// operator knows which backend it is talking to.
pub trait SearchProvider: Send + Sync + Debug + 'static {
    /// Return the name of this search provider
    fn name(&self) -> &str;

    /// Return the type of this search provider
    fn provider_type(&self) -> SearchProviderType;

    /// Return a human-readable description of this provider
    fn description(&self) -> String {
        format!("{} search provider: {}", self.provider_type(), self.name())
    }

    /// List search indexes for a given space.
    ///
    /// Returns all indexes (fulltext or vector) that belong to `space_id`.
    /// When `space_id` is `0` or not applicable, returns all indexes.
    fn list_indexes(&self, space_id: u64) -> Vec<IndexInfo>;

    /// Drop a search index identified by `(space_id, tag_name, field_name)`.
    ///
    /// Returns `Ok(())` on success, or an error string describing what went
    /// wrong.  The error is intentionally opaque (`String`) so that both
    /// `SearchError` and `VectorCoordinatorError` can be surfaced without
    /// introducing a shared error enum at the trait level.
    fn drop_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> Result<(), String>;
}

/// Newtype wrapper for FulltextIndexManager to implement SearchProvider
#[cfg(feature = "fulltext")]
pub struct FulltextProvider {
    manager: Arc<graphdb_fulltext::manager::FulltextIndexManager>,
}

#[cfg(feature = "fulltext")]
impl FulltextProvider {
    pub fn new(manager: Arc<graphdb_fulltext::manager::FulltextIndexManager>) -> Self {
        Self { manager }
    }

    pub fn manager(&self) -> &Arc<graphdb_fulltext::manager::FulltextIndexManager> {
        &self.manager
    }
}

#[cfg(feature = "fulltext")]
impl Debug for FulltextProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FulltextProvider")
            .field("index_count", &self.manager.list_indexes().len())
            .finish()
    }
}

#[cfg(feature = "fulltext")]
impl SearchProvider for FulltextProvider {
    fn name(&self) -> &str {
        "fulltext"
    }

    fn provider_type(&self) -> SearchProviderType {
        SearchProviderType::Fulltext
    }

    fn description(&self) -> String {
        format!(
            "Fulltext search provider ({} indexes)",
            self.manager.list_indexes().len()
        )
    }

    fn list_indexes(&self, space_id: u64) -> Vec<IndexInfo> {
        self.manager
            .list_indexes()
            .into_iter()
            .filter(|m| space_id == 0 || m.space_id == space_id)
            .map(|m| IndexInfo {
                space_id: m.space_id,
                tag_name: m.tag_name,
                field_name: m.field_name,
                index_name: Some(m.index_name.clone()),
                status: format!("{:?}", m.status),
            })
            .collect()
    }

    fn drop_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> Result<(), String> {
        // Block on the async drop_index from a sync context.
        let manager = self.manager.clone();
        let tag = tag_name.to_string();
        let field = field_name.to_string();
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async move { manager.drop_index(space_id, &tag, &field).await })
        })
        .map_err(|e| e.to_string())
    }
}

/// Newtype wrapper for VectorSyncCoordinator to implement SearchProvider
#[cfg(feature = "vector")]
pub struct VectorProvider {
    coordinator: Arc<graphdb_sync::vector_sync::VectorSyncCoordinator>,
}

#[cfg(feature = "vector")]
impl VectorProvider {
    pub fn new(coordinator: Arc<graphdb_sync::vector_sync::VectorSyncCoordinator>) -> Self {
        Self { coordinator }
    }

    pub fn coordinator(&self) -> &Arc<graphdb_sync::vector_sync::VectorSyncCoordinator> {
        &self.coordinator
    }
}

#[cfg(feature = "vector")]
impl Debug for VectorProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VectorProvider")
            .field("index_count", &self.coordinator.list_indexes().len())
            .finish()
    }
}

#[cfg(feature = "vector")]
impl SearchProvider for VectorProvider {
    fn name(&self) -> &str {
        "vector"
    }

    fn provider_type(&self) -> SearchProviderType {
        SearchProviderType::Vector
    }

    fn description(&self) -> String {
        format!(
            "Vector search provider ({} indexes)",
            self.coordinator.list_indexes().len()
        )
    }

    fn list_indexes(&self, space_id: u64) -> Vec<IndexInfo> {
        self.coordinator
            .list_indexes()
            .into_iter()
            .filter(|m| space_id == 0 || m.space_id == space_id)
            .map(|m| IndexInfo {
                space_id: m.space_id,
                tag_name: m.tag_name,
                field_name: m.field_name,
                index_name: m.index_name.clone(),
                status: "Active".to_string(),
            })
            .collect()
    }

    fn drop_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> Result<(), String> {
        let coordinator = self.coordinator.clone();
        let tag = tag_name.to_string();
        let field = field_name.to_string();
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
                coordinator
                    .drop_vector_index(space_id, &tag, &field)
                    .await
            })
        })
        .map_err(|e| e.to_string())
    }
}
