use std::sync::Arc;
use std::time::Instant;

use crate::core::stats::StatsManager;
use crate::search::engine::ConsistencyState;
use crate::search::error::SearchError;
use crate::search::result::{IndexStats, SearchResult};
use crate::search::tantivy_index::TantivySearchEngine;

pub struct MetricsSearchEngine {
    inner: Arc<TantivySearchEngine>,
    stats_manager: Arc<StatsManager>,
    space_id: u64,
    index_name: String,
}

impl MetricsSearchEngine {
    pub fn new(
        inner: Arc<TantivySearchEngine>,
        stats_manager: Arc<StatsManager>,
        space_id: u64,
        index_name: String,
    ) -> Self {
        Self {
            inner,
            stats_manager,
            space_id,
            index_name,
        }
    }

    pub fn into_arc(self) -> Arc<Self> {
        Arc::new(self)
    }

    pub fn name(&self) -> &str {
        "tantivy"
    }

    pub fn version(&self) -> &str {
        "0.26.0"
    }

    pub async fn index(&self, doc_id: &str, content: &str) -> Result<(), SearchError> {
        let start = Instant::now();
        let result = self.inner.index(doc_id, content).await;
        let latency_ms = start.elapsed().as_millis() as u64;
        match &result {
            Ok(_) => {
                self.stats_manager.record_index_operation(
                    self.space_id,
                    &self.index_name,
                    latency_ms,
                    true,
                );
            }
            Err(_e) => {
                self.stats_manager.record_index_operation(
                    self.space_id,
                    &self.index_name,
                    latency_ms,
                    false,
                );
            }
        }
        result
    }

    pub async fn index_batch(&self, docs: Vec<(String, String)>) -> Result<(), SearchError> {
        let start = Instant::now();
        let result = self.inner.index_batch(docs).await;
        let latency_ms = start.elapsed().as_millis() as u64;
        match &result {
            Ok(_) => {
                self.stats_manager.record_index_operation(
                    self.space_id,
                    &self.index_name,
                    latency_ms,
                    true,
                );
            }
            Err(_e) => {
                self.stats_manager.record_index_operation(
                    self.space_id,
                    &self.index_name,
                    latency_ms,
                    false,
                );
            }
        }
        result
    }

    pub async fn search(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SearchResult>, SearchError> {
        let start = Instant::now();
        let result = self.inner.search(query, limit).await;
        let latency_ms = start.elapsed().as_millis() as u64;

        match &result {
            Ok(results) => {
                self.stats_manager
                    .record_search(self.space_id, &self.index_name, latency_ms, true);
                self.stats_manager
                    .record_search_result_count(self.space_id, results.len() as u64);
            }
            Err(_e) => {
                self.stats_manager.record_search(
                    self.space_id,
                    &self.index_name,
                    latency_ms,
                    false,
                );
            }
        }

        result
    }

    pub async fn delete(&self, doc_id: &str) -> Result<(), SearchError> {
        let start = Instant::now();
        let result = self.inner.delete(doc_id).await;
        let latency_ms = start.elapsed().as_millis() as u64;
        match &result {
            Ok(_) => {
                self.stats_manager.record_delete_operation(
                    self.space_id,
                    &self.index_name,
                    latency_ms,
                    true,
                );
            }
            Err(_e) => {
                self.stats_manager.record_delete_operation(
                    self.space_id,
                    &self.index_name,
                    latency_ms,
                    false,
                );
            }
        }
        result
    }

    pub async fn delete_batch(&self, doc_ids: Vec<&str>) -> Result<(), SearchError> {
        let start = Instant::now();
        let result = self.inner.delete_batch(doc_ids).await;
        let latency_ms = start.elapsed().as_millis() as u64;
        match &result {
            Ok(_) => {
                self.stats_manager.record_delete_operation(
                    self.space_id,
                    &self.index_name,
                    latency_ms,
                    true,
                );
            }
            Err(_e) => {
                self.stats_manager.record_delete_operation(
                    self.space_id,
                    &self.index_name,
                    latency_ms,
                    false,
                );
            }
        }
        result
    }

    pub async fn commit(&self) -> Result<(), SearchError> {
        self.inner.commit().await
    }

    pub async fn rollback(&self) -> Result<(), SearchError> {
        self.inner.rollback().await
    }

    pub async fn stats(&self) -> Result<IndexStats, SearchError> {
        self.inner.stats().await
    }

    pub async fn close(&self) -> Result<(), SearchError> {
        self.inner.close().await
    }

    pub fn consistency_state(&self) -> ConsistencyState {
        self.inner.consistency_state()
    }

    pub fn mark_inconsistent(&self) {
        self.inner.mark_inconsistent();
    }

    pub fn mark_consistent(&self) {
        self.inner.mark_consistent();
    }

    pub async fn clear(&self) -> Result<(), SearchError> {
        self.inner.clear().await
    }
}

impl std::fmt::Debug for MetricsSearchEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MetricsSearchEngine")
            .field("space_id", &self.space_id)
            .field("index_name", &self.index_name)
            .finish()
    }
}
