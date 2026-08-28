//! Search Provider Trait
//!
//! This module defines the common trait for search providers (fulltext, vector)
//! to enable unified management and discovery in the execution context.

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

/// Common trait for search providers
///
/// This trait provides a unified interface for different search backends
/// (fulltext, vector) to be discovered and managed through a common API.
/// It enables the framework to enumerate available search providers without
/// knowing their concrete types.
pub trait SearchProvider: Send + Sync + Debug + 'static {
    /// Return the name of this search provider
    fn name(&self) -> &str;

    /// Return the type of this search provider
    fn provider_type(&self) -> SearchProviderType;

    /// Return a human-readable description of this provider
    fn description(&self) -> String {
        format!("{} search provider: {}", self.provider_type(), self.name())
    }
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
}
