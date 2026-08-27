//! Bridges vector engine metrics into the central [`StatsManager`].
//!
//! Two independent samplers coexist:
//!
//! - **Local**: reads per-collection [`vector_search::MetricsSnapshot`]
//!   from [`LocalVectorEngine`] and forwards deltas every 10 s.
//! - **Remote**: fetches `GET /telemetry` from the Qdrant server, parses
//!   the per-endpoint operation stats, and forwards deltas.
//!
//! Only one backend is active at a time; spawning both is harmless (the
//! inactive sampler simply never produces non-zero deltas).
//!
//! ## Remote metric mapping (delta per interval)
//!
//! | Qdrant endpoint | Global `MetricType` |
//! |---|---|
//! | `POST .../points/search` | `VectorSearchOps`, `VectorSearchErrors`, `VectorSearchLatencyMs` |
//! | `PUT .../points` (upsert) | `VectorUpsertOps`, `VectorUpsertErrors`, `VectorUpsertLatencyMs` |
//! | `POST .../points/delete` | `VectorDeleteOps`, `VectorDeleteErrors`, `VectorDeleteLatencyMs` |
//!
//! `VectorBufferFlush*` / `VectorEmbedding*` are populated by the
//! `EmbeddingService`, not by this sampler.

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

fn nanos_to_us(nanos_delta: u64) -> u64 {
    nanos_delta / 1_000
}

// ---------------------------------------------------------------------------
// Local engine sampler (unchanged logic)
// ---------------------------------------------------------------------------

/// Forwards local engine snapshot deltas into [`StatsManager`].
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
        let lock_ops = cur
            .adjacency_write_locks
            .saturating_sub(prev.adjacency_write_locks);
        let lock_us = nanos_to_us(
            cur.adjacency_lock_wait_nanos
                .saturating_sub(prev.adjacency_lock_wait_nanos),
        );
        let version_reloads = cur
            .search_version_reloads
            .saturating_sub(prev.search_version_reloads);

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
        stats.add_value_with_amount(MetricType::VectorLockOps, lock_ops);
        stats.add_value_with_amount(MetricType::VectorLockLatencyUs, lock_us);
        stats.add_value_with_amount(MetricType::VectorVersionReloads, version_reloads);

        stats.add_space_metric_with_amount(name, MetricType::VectorSearchOps, search_total);
        stats.add_space_metric_with_amount(name, MetricType::VectorSearchErrors, search_errors);
        stats.add_space_metric_with_amount(name, MetricType::VectorSearchLatencyMs, search_ms);
        stats.add_space_metric_with_amount(name, MetricType::VectorUpsertOps, upserts);
        stats.add_space_metric_with_amount(name, MetricType::VectorUpsertErrors, upsert_errors);
        stats.add_space_metric_with_amount(name, MetricType::VectorUpsertLatencyMs, apply_ms);
        stats.add_space_metric_with_amount(name, MetricType::VectorDeleteOps, deletes);
        stats.add_space_metric_with_amount(name, MetricType::VectorDeleteErrors, delete_errors);
        stats.add_space_metric_with_amount(name, MetricType::VectorDeleteLatencyMs, apply_ms);
        stats.add_space_metric_with_amount(name, MetricType::VectorLockOps, lock_ops);
        stats.add_space_metric_with_amount(name, MetricType::VectorLockLatencyUs, lock_us);
        stats.add_space_metric_with_amount(name, MetricType::VectorVersionReloads, version_reloads);
    }
}

// ---------------------------------------------------------------------------
// Remote (Qdrant) engine sampler — fetches /telemetry
// ---------------------------------------------------------------------------

/// Per-endpoint operation stats parsed from Qdrant's `/telemetry` response.
#[cfg(feature = "vector-qdrant")]
#[derive(Debug, Clone, Default)]
struct EndpointStats {
    count: u64,
    fail_count: u64,
    total_duration_micros: u64,
}

/// Snapshot of all relevant Qdrant operation stats at a point in time.
#[cfg(feature = "vector-qdrant")]
#[derive(Debug, Clone, Default)]
struct QdrantTelemetrySnapshot {
    search: EndpointStats,
    upsert: EndpointStats,
    delete: EndpointStats,
}

/// Fetches Qdrant server-side telemetry and forwards deltas into
/// [`StatsManager`].
///
/// Uses `GET /telemetry` (JSON) to obtain per-endpoint `count`,
/// `fail_count`, and `total_duration_micros`. Deltas between successive
/// samples are mapped to the global vector `MetricType`s.
#[cfg(feature = "vector-qdrant")]
pub struct QdrantTelemetrySampler {
    client: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
    stats: Arc<StatsManager>,
    last: QdrantTelemetrySnapshot,
}

#[cfg(feature = "vector-qdrant")]
impl QdrantTelemetrySampler {
    pub fn new(
        http_host: &str,
        http_port: u16,
        api_key: Option<String>,
        stats: Arc<StatsManager>,
    ) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .expect("failed to build reqwest client for telemetry");
        Self {
            client,
            base_url: format!("http://{}:{}", http_host, http_port),
            api_key,
            stats,
            last: QdrantTelemetrySnapshot::default(),
        }
    }

    /// Fetch `/telemetry`, parse, and forward deltas.
    pub fn sample_once(&mut self) {
        let cur = match self.fetch_telemetry() {
            Ok(snap) => snap,
            Err(e) => {
                tracing::debug!("failed to fetch Qdrant telemetry: {}", e);
                return;
            }
        };
        self.forward(&cur);
        self.last = cur;
    }

    fn fetch_telemetry(&self) -> Result<QdrantTelemetrySnapshot, reqwest::Error> {
        let url = format!("{}/telemetry?details_level=1", self.base_url);
        let mut req = self.client.get(&url);
        if let Some(ref key) = self.api_key {
            req = req.header("api-key", key.as_str());
        }
        let body = req.send().and_then(|r| r.error_for_status())?;
        let json: serde_json::Value = body.json()?;
        Self::parse_telemetry(&json)
    }

    fn parse_telemetry(
        json: &serde_json::Value,
    ) -> Result<QdrantTelemetrySnapshot, reqwest::Error> {
        let responses = json
            .pointer("/result/requests/rest/responses")
            .and_then(|v| v.as_object());

        let grpc_responses = json
            .pointer("/result/requests/grpc/responses")
            .and_then(|v| v.as_object());

        // Try REST endpoints first, fall back to gRPC.
        let search = Self::extract_endpoint(
            responses,
            &[
                "POST /collections/{collection_name}/points/search",
                "POST /collections/{name}/points/search",
            ],
        )
        .or_else(|| {
            Self::extract_endpoint(
                grpc_responses,
                &["/qdrant.Points/Search", "/qdrant.Points/SearchBatch"],
            )
        })
        .unwrap_or_default();

        let upsert = Self::extract_endpoint(
            responses,
            &[
                "PUT /collections/{collection_name}/points",
                "PUT /collections/{name}/points",
            ],
        )
        .or_else(|| {
            Self::extract_endpoint(
                grpc_responses,
                &["/qdrant.Points/Upsert", "/qdrant.Points/UpdateBatch"],
            )
        })
        .unwrap_or_default();

        let delete = Self::extract_endpoint(
            responses,
            &[
                "POST /collections/{collection_name}/points/delete",
                "POST /collections/{name}/points/delete",
            ],
        )
        .or_else(|| Self::extract_endpoint(grpc_responses, &["/qdrant.Points/Delete"]))
        .unwrap_or_default();

        Ok(QdrantTelemetrySnapshot {
            search,
            upsert,
            delete,
        })
    }

    /// Look up an endpoint in the responses map and sum stats across all
    /// status codes.
    fn extract_endpoint(
        responses: Option<&serde_json::Map<String, serde_json::Value>>,
        candidate_keys: &[&str],
    ) -> Option<EndpointStats> {
        let responses = responses?;
        let mut total = EndpointStats::default();

        for key in candidate_keys {
            if let Some(status_map) = responses.get(*key).and_then(|v| v.as_object()) {
                for (_status, stats_val) in status_map {
                    if let Some(obj) = stats_val.as_object() {
                        total.count += obj.get("count").and_then(|v| v.as_u64()).unwrap_or(0);
                        total.fail_count +=
                            obj.get("fail_count").and_then(|v| v.as_u64()).unwrap_or(0);
                        total.total_duration_micros += obj
                            .get("total_duration_micros")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0);
                    }
                }
            }
        }

        (total.count > 0).then(total)
    }

    fn forward(&self, cur: &QdrantTelemetrySnapshot) {
        let prev = &self.last;

        let search_ops = cur.search.count.saturating_sub(prev.search.count);
        let search_errors = cur.search.fail_count.saturating_sub(prev.search.fail_count);
        let search_ms = micros_to_ms(
            cur.search
                .total_duration_micros
                .saturating_sub(prev.search.total_duration_micros),
        );

        let upsert_ops = cur.upsert.count.saturating_sub(prev.upsert.count);
        let upsert_errors = cur.upsert.fail_count.saturating_sub(prev.upsert.fail_count);
        let upsert_ms = micros_to_ms(
            cur.upsert
                .total_duration_micros
                .saturating_sub(prev.upsert.total_duration_micros),
        );

        let delete_ops = cur.delete.count.saturating_sub(prev.delete.count);
        let delete_errors = cur.delete.fail_count.saturating_sub(prev.delete.fail_count);
        let delete_ms = micros_to_ms(
            cur.delete
                .total_duration_micros
                .saturating_sub(prev.delete.total_duration_micros),
        );

        let stats = &self.stats;
        stats.add_value_with_amount(MetricType::VectorSearchOps, search_ops);
        stats.add_value_with_amount(MetricType::VectorSearchErrors, search_errors);
        stats.add_value_with_amount(MetricType::VectorSearchLatencyMs, search_ms);
        stats.add_value_with_amount(MetricType::VectorUpsertOps, upsert_ops);
        stats.add_value_with_amount(MetricType::VectorUpsertErrors, upsert_errors);
        stats.add_value_with_amount(MetricType::VectorUpsertLatencyMs, upsert_ms);
        stats.add_value_with_amount(MetricType::VectorDeleteOps, delete_ops);
        stats.add_value_with_amount(MetricType::VectorDeleteErrors, delete_errors);
        stats.add_value_with_amount(MetricType::VectorDeleteLatencyMs, delete_ms);
    }
}

#[cfg(feature = "vector-qdrant")]
fn micros_to_ms(micros_delta: u64) -> u64 {
    micros_delta / 1_000
}

// ---------------------------------------------------------------------------
// Spawn helpers
// ---------------------------------------------------------------------------

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

/// Spawn the daemon thread driving [`QdrantTelemetrySampler`] for the
/// Qdrant backend. Only compiled when the `vector-qdrant` feature is active.
#[cfg(feature = "vector-qdrant")]
pub fn spawn_remote_vector_metrics_sampler(
    http_host: String,
    http_port: u16,
    api_key: Option<String>,
    stats: Arc<StatsManager>,
) -> JoinHandle<()> {
    ThreadBuilder::new()
        .name("vector-remote-metrics".to_string())
        .spawn(move || {
            let mut sampler = QdrantTelemetrySampler::new(&http_host, http_port, api_key, stats);
            loop {
                std::thread::sleep(SAMPLE_INTERVAL);
                sampler.sample_once();
            }
        })
        .expect("spawn vector-remote-metrics thread")
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

    #[cfg(feature = "vector-qdrant")]
    #[test]
    fn parse_telemetry_json() {
        let json = serde_json::json!({
            "result": {
                "requests": {
                    "rest": {
                        "responses": {
                            "POST /collections/{collection_name}/points/search": {
                                "200": {
                                    "count": 42,
                                    "fail_count": 2,
                                    "total_duration_micros": 51849
                                }
                            },
                            "PUT /collections/{collection_name}/points": {
                                "200": {
                                    "count": 100,
                                    "fail_count": 0,
                                    "total_duration_micros": 25000
                                }
                            },
                            "POST /collections/{collection_name}/points/delete": {
                                "200": {
                                    "count": 10,
                                    "fail_count": 1,
                                    "total_duration_micros": 5000
                                }
                            }
                        }
                    },
                    "grpc": { "responses": {} }
                }
            }
        });

        let snap = QdrantTelemetrySampler::parse_telemetry(&json).unwrap();
        assert_eq!(snap.search.count, 42);
        assert_eq!(snap.search.fail_count, 2);
        assert_eq!(snap.search.total_duration_micros, 51849);
        assert_eq!(snap.upsert.count, 100);
        assert_eq!(snap.upsert.fail_count, 0);
        assert_eq!(snap.delete.count, 10);
        assert_eq!(snap.delete.fail_count, 1);
    }
}
