//! Unified search index manager trait.
//!
//! Both fulltext and vector backends need the same index management
//! operations (create, drop, list, rebuild, clear). This module defines
//! the common trait so management and observability code can work with
//! either backend through a single interface.

use crate::error::search::SearchErrorKind;
use crate::types::search::IndexDescriptor;
use crate::types::IndexStats;
use std::future::Future;
use std::pin::Pin;

/// Result type for search index management operations.
pub type SearchManagerResult<T> = Result<T, SearchErrorKind>;

/// Unified management interface for secondary search indexes.
///
/// Implementations live in the respective backend crates:
/// - `linkrs-fulltext` for fulltext indexes
/// - `linkrs-sync` for vector indexes
///
/// The trait uses async-boxed futures to avoid requiring a specific
/// async runtime at the trait definition level.
pub trait SearchIndexManager: Send + Sync {
    /// Create a new index. Returns the index identifier.
    fn create_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> Pin<Box<dyn Future<Output = SearchManagerResult<String>> + Send + '_>>;

    /// Drop an index.
    fn drop_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> Pin<Box<dyn Future<Output = SearchManagerResult<()>> + Send + '_>>;

    /// List all indexes known to this backend.
    fn list_indexes(&self) -> Vec<IndexDescriptor>;

    /// Get statistics for a specific index.
    fn get_stats(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> Pin<Box<dyn Future<Output = SearchManagerResult<IndexStats>> + Send + '_>>;

    /// Clear all data from an index without dropping it.
    /// `force` must be true to confirm the destructive operation.
    fn clear_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        force: bool,
    ) -> Pin<Box<dyn Future<Output = SearchManagerResult<()>> + Send + '_>>;

    /// Rebuild an index from primary storage.
    fn rebuild_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> Pin<Box<dyn Future<Output = SearchManagerResult<u64>> + Send + '_>>;

    /// List indexes that need operator attention (failed rebuilds, etc.).
    fn inconsistent_indexes(&self) -> Vec<IndexDescriptor>;
}
