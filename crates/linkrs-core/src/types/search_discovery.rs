//! Unified search index discovery trait.
//!
//! Both fulltext and vector backends need to discover existing indexes
//! at startup or on demand. This module defines the common discovery
//! interface so management code can enumerate indexes from either backend
//! through a single trait.

use crate::types::search::IndexDescriptor;

/// Unified discovery interface for secondary search indexes.
///
/// Implementations live in the respective backend crates.
pub trait SearchIndexDiscovery: Send + Sync {
    /// Discover all indexes known to this backend.
    fn discover_indexes(&self) -> Vec<IndexDescriptor>;

    /// Check if a specific index exists.
    fn index_exists(&self, space_id: u64, tag_name: &str, field_name: &str) -> bool;

    /// Get the descriptor for a specific index, if it exists.
    fn get_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> Option<IndexDescriptor>;
}
