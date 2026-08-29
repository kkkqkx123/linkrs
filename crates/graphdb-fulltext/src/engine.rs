//! Backend-agnostic trait for fulltext search engines.
//!
//! This trait abstracts the core operations shared by all fulltext backends
//! (Tantivy, Elasticsearch, Meilisearch, etc.). The `FulltextIndexManager`
//! operates on `Arc<dyn FulltextSearchEngine>` so new backends can be added
//! without modifying the manager layer.

use crate::error::SearchError;
use crate::result::{IndexStats, SearchResult};
use crate::ConsistencyState;

use async_trait::async_trait;

/// Backend-agnostic fulltext search engine interface.
///
/// # Implementors
///
/// - [`TantivySearchEngine`](crate::tantivy_index::TantivySearchEngine) —
///   default local BM25 engine backed by Tantivy.
///
/// Future backends (Elasticsearch, Meilisearch, etc.) implement this trait
/// and are plugged into [`FulltextIndexManager`](crate::manager::FulltextIndexManager)
/// at construction time.
#[async_trait]
pub trait FulltextSearchEngine: Send + Sync + std::fmt::Debug + 'static {
    /// Engine name for logging and metrics (e.g., `"tantivy"`, `"elasticsearch"`).
    fn name(&self) -> &str;

    /// Engine version string.
    fn version(&self) -> &str;

    /// Index a single document. If `doc_id` already exists, it is replaced.
    async fn index(&self, doc_id: &str, content: &str) -> Result<(), SearchError>;

    /// Batch-index multiple documents. Default implementation calls `index`
    /// one by one; backends should override for bulk efficiency.
    async fn index_batch(&self, docs: Vec<(String, String)>) -> Result<(), SearchError> {
        for (doc_id, content) in docs {
            self.index(&doc_id, &content).await?;
        }
        Ok(())
    }

    /// Full-text search with a query string, returning at most `limit` results.
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>, SearchError>;

    /// Delete a single document by its id.
    async fn delete(&self, doc_id: &str) -> Result<(), SearchError>;

    /// Batch-delete documents by their ids.
    async fn delete_batch(&self, doc_ids: Vec<&str>) -> Result<(), SearchError> {
        for doc_id in doc_ids {
            self.delete(doc_id).await?;
        }
        Ok(())
    }

    /// Flush pending writes to durable storage.
    async fn commit(&self) -> Result<(), SearchError>;

    /// Flush with a payload string attached to the commit (used for
    /// consistency fence coordination in the sync layer).
    async fn commit_with_payload(&self, payload: String) -> Result<(), SearchError> {
        // Default: ignore payload, just commit.
        let _ = payload;
        self.commit().await
    }

    /// Read back the payload from the last committed transaction.
    fn commit_payload(&self) -> Result<Option<String>, SearchError> {
        Ok(None)
    }

    /// Roll back uncommitted writes.
    async fn rollback(&self) -> Result<(), SearchError> {
        Ok(())
    }

    /// Return current index statistics (doc count, index size, etc.).
    async fn stats(&self) -> Result<IndexStats, SearchError>;

    /// Return the current consistency state.
    fn consistency_state(&self) -> ConsistencyState;

    /// Mark the index as inconsistent (e.g., during rebuild).
    fn mark_inconsistent(&self);

    /// Mark the index as consistent (rebuild complete).
    fn mark_consistent(&self);

    /// Delete all documents from the index.
    async fn clear(&self) -> Result<(), SearchError>;

    /// Close the engine, flushing any pending writes.
    async fn close(&self) -> Result<(), SearchError>;
}
