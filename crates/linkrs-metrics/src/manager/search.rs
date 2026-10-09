//! Search, index and vector observability.
use serde::{Deserialize, Serialize};

use super::core::StatsManager;
use super::metric_type::MetricType;

/// Per-index breakdown for handlers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchIndexBreakdown {
    pub index: String,
    pub search_queries: u64,
    pub search_errors: u64,
    pub search_latency_ms: u64,
    pub avg_search_latency_ms: f64,
    pub index_operations: u64,
    pub index_errors: u64,
    pub index_latency_ms: u64,
}

impl StatsManager {
    /// Record a search query operation.
    /// Index metrics are keyed per-space (`space_{id}.{index}`) so same-named
    /// indexes in different spaces no longer pollute each other.
    pub fn record_search(&self, space_id: u64, index_name: &str, latency_ms: u64, success: bool) {
        let space_key = Self::space_key(space_id);
        let index_key = Self::index_key(space_id, index_name);
        self.add_value(MetricType::NumSearchQueries);
        self.add_space_metric(&space_key, MetricType::NumSearchQueries);
        self.add_index_metric(&index_key, MetricType::NumSearchQueries);

        if !success {
            self.add_value(MetricType::NumSearchErrors);
            self.add_space_metric(&space_key, MetricType::NumSearchErrors);
            self.add_index_metric(&index_key, MetricType::NumSearchErrors);
        }

        self.add_value_with_amount(MetricType::SearchLatencyMs, latency_ms);
        self.add_space_metric_with_amount(&space_key, MetricType::SearchLatencyMs, latency_ms);
        self.add_index_metric_with_amount(&index_key, MetricType::SearchLatencyMs, latency_ms);

        {
            let mut histogram = self.search_latency_histogram.write();
            histogram.record_micros(latency_ms * 1000);
        }
    }

    /// Record an index operation
    pub fn record_index_operation(
        &self,
        space_id: u64,
        index_name: &str,
        latency_ms: u64,
        success: bool,
    ) {
        let space_key = Self::space_key(space_id);
        let index_key = Self::index_key(space_id, index_name);
        self.add_value(MetricType::NumIndexOperations);
        self.add_space_metric(&space_key, MetricType::NumIndexOperations);
        self.add_index_metric(&index_key, MetricType::NumIndexOperations);

        if !success {
            self.add_value(MetricType::NumIndexErrors);
            self.add_space_metric(&space_key, MetricType::NumIndexErrors);
            self.add_index_metric(&index_key, MetricType::NumIndexErrors);
        }

        self.add_value_with_amount(MetricType::IndexLatencyMs, latency_ms);
        self.add_space_metric_with_amount(&space_key, MetricType::IndexLatencyMs, latency_ms);
        self.add_index_metric_with_amount(&index_key, MetricType::IndexLatencyMs, latency_ms);
    }

    /// Record an index operation without a known space.
    /// Used by storage paths that lack space context; records global and
    /// `unknown.{index}` buckets only, never polluting real `space_{id}` keys.
    pub fn record_index_operation_unknown_space(
        &self,
        index_name: &str,
        latency_ms: u64,
        success: bool,
    ) {
        let index_key = Self::unknown_index_key(index_name);
        self.add_value(MetricType::NumIndexOperations);
        self.add_index_metric(&index_key, MetricType::NumIndexOperations);
        if !success {
            self.add_value(MetricType::NumIndexErrors);
            self.add_index_metric(&index_key, MetricType::NumIndexErrors);
        }
        self.add_value_with_amount(MetricType::IndexLatencyMs, latency_ms);
        self.add_index_metric_with_amount(&index_key, MetricType::IndexLatencyMs, latency_ms);
    }

    /// Record a delete operation
    pub fn record_delete_operation(
        &self,
        space_id: u64,
        index_name: &str,
        latency_ms: u64,
        success: bool,
    ) {
        let space_key = Self::space_key(space_id);
        let index_key = Self::index_key(space_id, index_name);
        self.add_value(MetricType::NumDeleteOperations);
        self.add_space_metric(&space_key, MetricType::NumDeleteOperations);
        self.add_index_metric(&index_key, MetricType::NumDeleteOperations);

        if !success {
            self.add_value(MetricType::NumDeleteErrors);
            self.add_space_metric(&space_key, MetricType::NumDeleteErrors);
            self.add_index_metric(&index_key, MetricType::NumDeleteErrors);
        }

        self.add_value_with_amount(MetricType::DeleteLatencyMs, latency_ms);
        self.add_space_metric_with_amount(&space_key, MetricType::DeleteLatencyMs, latency_ms);
        self.add_index_metric_with_amount(&index_key, MetricType::DeleteLatencyMs, latency_ms);
    }

    /// Record search result count
    pub fn record_search_result_count(&self, space_id: u64, count: u64) {
        let space_key = Self::space_key(space_id);
        self.add_value_with_amount(MetricType::SearchResultCount, count);
        self.add_space_metric_with_amount(&space_key, MetricType::SearchResultCount, count);
    }

    /// Record cache hit or miss (global only).
    pub fn record_cache_hit_global(&self, hit: bool) {
        if hit {
            self.add_value(MetricType::SearchCacheHitCount);
        } else {
            self.add_value(MetricType::SearchCacheMissCount);
        }
    }

    pub fn record_vector_disabled_skips(&self, count: u64) {
        self.set_value(MetricType::VectorDisabledSkips, count);
    }

    /// Record one embedding call (global bucket only).
    ///
    /// The embedding path has no space context, so only global counters are
    /// updated; per-space keys are never synthesized here. Failed calls
    /// carry zero token usage.
    pub fn record_vector_embedding(
        &self,
        prompt_tokens: u64,
        total_tokens: u64,
        latency_ms: u64,
        success: bool,
    ) {
        self.add_value(MetricType::VectorEmbeddingOps);
        if !success {
            self.add_value(MetricType::VectorEmbeddingErrors);
        }
        self.add_value_with_amount(MetricType::VectorEmbeddingLatencyMs, latency_ms);
        self.add_value_with_amount(MetricType::VectorEmbeddingPromptTokens, prompt_tokens);
        self.add_value_with_amount(MetricType::VectorEmbeddingTotalTokens, total_tokens);
    }

    /// Record one issued rerank call (global bucket only).
    ///
    /// Skipped reranks (no service, no query text, thin coverage) are not
    /// recorded; only actual provider calls land here.
    pub fn record_vector_rerank(&self, latency_ms: u64, success: bool) {
        self.add_value(MetricType::VectorRerankOps);
        if !success {
            self.add_value(MetricType::VectorRerankErrors);
        }
        self.add_value_with_amount(MetricType::VectorRerankLatencyMs, latency_ms);
    }

    pub fn search_index_breakdown(&self) -> Vec<SearchIndexBreakdown> {
        let mut out = Vec::new();
        for entry in self.index_metrics.iter() {
            let name = entry.key().clone();
            let map = entry.value();
            let get = |m: MetricType| map.get(&m).map(|v| v.get()).unwrap_or(0);
            let search_queries = get(MetricType::NumSearchQueries);
            let index_operations = get(MetricType::NumIndexOperations);
            if search_queries == 0 && index_operations == 0 {
                continue;
            }
            let search_latency_ms = get(MetricType::SearchLatencyMs);
            out.push(SearchIndexBreakdown {
                index: name,
                search_queries,
                search_errors: get(MetricType::NumSearchErrors),
                search_latency_ms,
                avg_search_latency_ms: if search_queries == 0 {
                    0.0
                } else {
                    search_latency_ms as f64 / search_queries as f64
                },
                index_operations,
                index_errors: get(MetricType::NumIndexErrors),
                index_latency_ms: get(MetricType::IndexLatencyMs),
            });
        }
        out.sort_by_key(|s| std::cmp::Reverse(s.search_queries));
        out.truncate(50);
        out
    }
}
