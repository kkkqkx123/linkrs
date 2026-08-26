//! Bridges local vector engine metrics into the central [`StatsManager`].
//!
//! The engine records wait-free counters and latency histograms per
//! collection ([`vector_search::MetricsSnapshot`]). A background thread
//! samples those snapshots periodically and forwards the deltas into the
//! global metric registry and the per-collection `space` map, so vector
//! activity appears alongside query/storage/fulltext statistics through the
//! usual `/v1/statistics` surface.
//!
//! Mapping (delta per interval):
//! - `search_total`          → [`MetricType::VectorSearchOps`]
//! - `search_errors`         → [`MetricType::VectorSearchErrors`]
//! - `search.total_nanos`    → [`MetricType::VectorSearchLatencyMs`] (cumulative ms)
//! - `points_upserted`       → [`MetricType::VectorUpsertOps`]
//! - `upsert_errors`         → [`MetricType::VectorUpsertErrors`]
//! - `points_deleted`        → [`MetricType::VectorDeleteOps`]
//! - `delete_errors`         → [`MetricType::VectorDeleteErrors`]
//! - `apply_txn.total_nanos` → upsert/delete latency accumulators (the WAL
//!   fsync inside the store write lock is shared by both write kinds)
//!
//! `VectorBufferFlush*` / `VectorEmbedding*` stay reserved for remote
//! engines; index-build and compaction counters remain engine-local
//! introspection via [`vector_search::LocalVectorEngine::collection_metrics`].

use std::collections::HashMap;
use std::sync::Arc;
use std::thread::{Builder as ThreadBuilder, JoinHandle};
use std::time::Duration;

use graphdb_metrics::MetricType;
use vector_search::{LocalVectorEngine, MetricsSnapshot};

use crate::core::stats::StatsManager;

/// How often engine snapshots are sampled and forwarded.
const SAMPLE_INTERVAL: Duration = Duration::from_secs(10);

fn nanos_to_ms(nanos_delta: u64) -> u64 {
    nanos_delta / 1_000_000
}

/// Forwards engine snapshot deltas into [`StatsManager`].
///
/// Stateful: keeps the previous snapshot per collection to compute deltas,
/// so re-sampling without traffic adds nothing.
pub struct VectorMetricsSampler {
    engine: Arc<LocalVectorEngine>,
    stats: Arc<StatsManager>,
    last: HashMap<String, MetricsSnapshot>,
}

impl VectorMetricsSampler {
    pub fn new(engine: Arc<LocalVectorEngine>, stats: Arc<StatsManager>) -> Self {
        Self {
            engine,
            stats,
            last: HashMap::new(),
        }
    }

    /// Sample every collection once. Collections discovered for the first
    /// time contribute their full accumulated totals; collections removed
    /// from the engine are dropped from the tracking set.
    pub fn sample_once(&mut self) {
        for name in self.engine.collection_names() {
            let Ok(snap) = self.engine.collection_metrics(&name) else {
                continue;
            };
            let prev = self.last.get(&name).copied().unwrap_or_default();
            self.forward(&name, &snap, &prev);
            self.last.insert(name, snap);
        }
        let live: Vec<String> = self.engine.collection_names();
        self.last.retain(|name, _| live.contains(name));
    }

    fn forward(&self, name: &str, cur: &MetricsSnapshot, prev: &MetricsSnapshot) {
        let search_total = cur.search_total.saturating_sub(prev.search_total);
        let search_errors = cur.search_errors.saturating_sub(prev.search_errors);
        let search_ms = nanos_to_ms(
            cur.search
                .total_nanos
                .saturating_sub(prev.search.total_nanos),
        );
        let upserts = cur.points_upserted.saturating_sub(prev.points_upserted);
        let upsert_errors = cur.upsert_errors.saturating_sub(prev.upsert_errors);
        let deletes = cur.points_deleted.saturating_sub(prev.points_deleted);
        let delete_errors = cur.delete_errors.saturating_sub(prev.delete_errors);
        let apply_ms = nanos_to_ms(
            cur.apply_txn
                .total_nanos
                .saturating_sub(prev.apply_txn.total_nanos),
        );

        let stats = &self.stats;
        stats.add_value_with_amount(MetricType::VectorSearchOps, search_total);
        stats.add_value_with_amount(MetricType::VectorSearchErrors, search_errors);
        stats.add_value_with_amount(MetricType::VectorSearchLatencyMs, search_ms);
        stats.add_value_with_amount(MetricType::VectorUpsertOps, upserts);
        stats.add_value_with_amount(MetricType::VectorUpsertErrors, upsert_errors);
        stats.add_value_with_amount(MetricType::VectorUpsertLatencyMs, apply_ms);
        stats.add_value_with_amount(MetricType::VectorDeleteOps, deletes);
        stats.add_value_with_amount(MetricType::VectorDeleteErrors, delete_errors);
        stats.add_value_with_amount(MetricType::VectorDeleteLatencyMs, apply_ms);

        stats.add_space_metric_with_amount(name, MetricType::VectorSearchOps, search_total);
        stats.add_space_metric_with_amount(name, MetricType::VectorSearchErrors, search_errors);
        stats.add_space_metric_with_amount(name, MetricType::VectorSearchLatencyMs, search_ms);
        stats.add_space_metric_with_amount(name, MetricType::VectorUpsertOps, upserts);
        stats.add_space_metric_with_amount(name, MetricType::VectorUpsertErrors, upsert_errors);
        stats.add_space_metric_with_amount(name, MetricType::VectorUpsertLatencyMs, apply_ms);
        stats.add_space_metric_with_amount(name, MetricType::VectorDeleteOps, deletes);
        stats.add_space_metric_with_amount(name, MetricType::VectorDeleteErrors, delete_errors);
        stats.add_space_metric_with_amount(name, MetricType::VectorDeleteLatencyMs, apply_ms);
    }
}

/// Spawn the daemon thread driving [`VectorMetricsSampler`]. Lives until
/// process exit; drops out silently if the runtime shuts down first.
pub fn spawn_vector_metrics_sampler(
    engine: Arc<LocalVectorEngine>,
    stats: Arc<StatsManager>,
) -> JoinHandle<()> {
    ThreadBuilder::new()
        .name("vector-metrics".to_string())
        .spawn(move || {
            let mut sampler = VectorMetricsSampler::new(engine, stats);
            loop {
                std::thread::sleep(SAMPLE_INTERVAL);
                sampler.sample_once();
            }
        })
        .expect("spawn vector-metrics thread")
}

#[cfg(test)]
mod tests {
    use super::*;
    use vector_search::{CollectionConfig, DistanceMetric, SearchQuery, VectorPoint};

    fn point(id: u64, dim: usize) -> VectorPoint {
        VectorPoint::new(
            id,
            (0..dim)
                .map(|i| ((id as usize * 31 + i) % 100) as f32 / 100.0)
                .collect(),
        )
    }

    #[test]
    fn forwards_deltas_into_stats_manager() {
        let dir = tempfile::tempdir().unwrap();
        let engine = Arc::new(LocalVectorEngine::open(dir.path()).unwrap());
        engine
            .create_collection("col", &CollectionConfig::new(4, DistanceMetric::Cosine))
            .unwrap();
        let points: Vec<VectorPoint> = (0..5).map(|i| point(i, 4)).collect();
        engine.upsert_batch("col", &points).unwrap();
        engine
            .search("col", &SearchQuery::new(vec![0.5; 4], 3))
            .unwrap();

        let stats = Arc::new(StatsManager::new());
        let mut sampler = VectorMetricsSampler::new(Arc::clone(&engine), Arc::clone(&stats));
        sampler.sample_once();

        assert_eq!(stats.get_value(MetricType::VectorSearchOps), Some(1));
        assert_eq!(stats.get_value(MetricType::VectorUpsertOps), Some(5));
        assert_eq!(stats.get_value(MetricType::VectorDeleteOps), Some(0));
        assert!(stats.get_value(MetricType::VectorSearchLatencyMs).is_some());
        assert!(
            stats.get_value(MetricType::VectorUpsertLatencyMs).is_some(),
            "apply latency recorded"
        );
        // Per-collection breakdown lands under the collection name.
        assert_eq!(
            stats.get_space_value("col", MetricType::VectorUpsertOps),
            Some(5)
        );

        // Re-sampling without activity contributes nothing.
        sampler.sample_once();
        assert_eq!(stats.get_value(MetricType::VectorUpsertOps), Some(5));

        // New activity is picked up incrementally.
        engine.delete("col", "0").unwrap();
        sampler.sample_once();
        assert_eq!(stats.get_value(MetricType::VectorDeleteOps), Some(1));
    }
}
