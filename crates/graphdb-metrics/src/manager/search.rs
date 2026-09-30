//! Search, index and vector observability.
use super::core::StatsManager;
use super::metric_type::MetricType;

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
}
