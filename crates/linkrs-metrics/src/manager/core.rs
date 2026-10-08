//! Facade owner: state layout, construction and scoping keys.
//!
//! Holds every backing store but implements no recording policy itself.
//! Counter primitives live in `counters`, query observability in `profiles`
//! and `query_metrics`, error and pattern delegation in `errors` and
//! `aggregated`, and per-subsystem recorders in their own files.

use dashmap::DashMap;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use crate::aggregated_stats::AggregatedStatsManager;
use crate::error_stats::ErrorStatsManager;
use crate::latency_histogram::LatencyHistogram;
use crate::metrics::QueryMetrics;
use crate::profile::QueryProfile;
use crate::slow_query_logger::{SlowQueryConfig, SlowQueryLogger};

use super::metric_type::MetricType;
use super::metric_value::SpaceMetrics;

/// Per-second query bucket for the fixed-length ring.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct TimeseriesBucket {
    pub second: u64,
    pub queries: u64,
    pub total_latency_us: u64,
    pub errors: u64,
}

impl TimeseriesBucket {
    pub fn avg_latency_ms(&self) -> f64 {
        if self.queries == 0 {
            0.0
        } else {
            self.total_latency_us as f64 / self.queries as f64 / 1000.0
        }
    }
}

/// Resource sample point recorded by the server layer.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ResourceSample {
    pub second: u64,
    pub memory_used_bytes: u64,
    pub memory_total_bytes: u64,
    pub cpu_usage_percent: f64,
}

fn unix_secs_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Statistics Manager
///
/// Unified management of query metrics, query profiling and error statistics.
#[derive(Debug)]
pub struct StatsManager {
    pub(crate) metrics: Arc<DashMap<MetricType, Arc<super::metric_value::MetricValue>>>,
    pub(crate) space_metrics: Arc<DashMap<String, SpaceMetrics>>,
    pub(crate) index_metrics: Arc<DashMap<String, SpaceMetrics>>,
    pub(crate) last_query_metrics: Arc<RwLock<Option<QueryMetrics>>>,
    pub(crate) query_metrics_history: Arc<RwLock<VecDeque<QueryMetrics>>>,
    pub(crate) bloom_lock: Arc<parking_lot::Mutex<()>>,
    pub(crate) query_profiles: Arc<RwLock<VecDeque<QueryProfile>>>,
    pub(crate) query_latency_histogram: Arc<RwLock<LatencyHistogram>>,
    pub(crate) search_latency_histogram: Arc<RwLock<LatencyHistogram>>,
    pub(crate) monitoring_enabled: bool,
    pub(crate) profile_cache_size: usize,
    pub(crate) slow_query_threshold_us: u64,
    pub(crate) error_stats: ErrorStatsManager,
    pub(crate) slow_query_logger: Option<Arc<SlowQueryLogger>>,
    pub(crate) aggregated_stats: AggregatedStatsManager,
    pub(crate) timeseries: Arc<RwLock<VecDeque<TimeseriesBucket>>>,
    pub(crate) resource_series: Arc<RwLock<VecDeque<ResourceSample>>>,
    pub(crate) timeseries_capacity: Arc<AtomicUsize>,
}

impl StatsManager {
    /// Single key rule: `space_{id}` for numeric spaces, `space_name:{name}`
    /// for named collections, `target:{name}` for sync targets,
    /// `rebuild:{target}:{phase}` for rebuild phases, and
    /// `space_{id}.{index}` for per-space indexes. Distinct prefixes keep
    /// same-named entities in different subsystems from polluting each other
    /// while identical spaces share one key.
    pub fn space_key(space_id: u64) -> String {
        format!("space_{}", space_id)
    }

    pub fn space_key_for_name(name: &str) -> String {
        format!("space_name:{name}")
    }

    pub fn target_key(target: &str) -> String {
        format!("target:{target}")
    }

    pub fn rebuild_key(target: &str, phase: &str) -> String {
        format!("rebuild:{target}:{phase}")
    }

    pub fn index_key(space_id: u64, index_name: &str) -> String {
        format!("space_{}.{}", space_id, index_name)
    }

    pub fn unknown_index_key(index_name: &str) -> String {
        format!("unknown.{}", index_name)
    }

    pub fn new() -> Self {
        Self {
            metrics: Arc::new(DashMap::new()),
            space_metrics: Arc::new(DashMap::new()),
            index_metrics: Arc::new(DashMap::new()),
            last_query_metrics: Arc::new(RwLock::new(None)),
            query_metrics_history: Arc::new(RwLock::new(VecDeque::with_capacity(1000))),
            bloom_lock: Arc::new(parking_lot::Mutex::new(())),
            query_profiles: Arc::new(RwLock::new(VecDeque::with_capacity(1000))),
            query_latency_histogram: Arc::new(RwLock::new(LatencyHistogram::new(10000))),
            search_latency_histogram: Arc::new(RwLock::new(LatencyHistogram::new(10000))),
            monitoring_enabled: true,
            profile_cache_size: 1000,
            slow_query_threshold_us: 1_000_000,
            error_stats: ErrorStatsManager::new(),
            slow_query_logger: None,
            aggregated_stats: AggregatedStatsManager::new(),
            timeseries: Arc::new(RwLock::new(VecDeque::with_capacity(3600))),
            resource_series: Arc::new(RwLock::new(VecDeque::with_capacity(3600))),
            timeseries_capacity: Arc::new(AtomicUsize::new(3600)),
        }
    }

    pub fn with_config(
        monitoring_enabled: bool,
        profile_cache_size: usize,
        slow_query_threshold_us: u64,
    ) -> Self {
        Self {
            metrics: Arc::new(DashMap::new()),
            space_metrics: Arc::new(DashMap::new()),
            index_metrics: Arc::new(DashMap::new()),
            last_query_metrics: Arc::new(RwLock::new(None)),
            query_metrics_history: Arc::new(RwLock::new(VecDeque::with_capacity(1000))),
            bloom_lock: Arc::new(parking_lot::Mutex::new(())),
            query_profiles: Arc::new(RwLock::new(VecDeque::with_capacity(profile_cache_size))),
            query_latency_histogram: Arc::new(RwLock::new(LatencyHistogram::new(10000))),
            search_latency_histogram: Arc::new(RwLock::new(LatencyHistogram::new(10000))),
            monitoring_enabled,
            profile_cache_size,
            slow_query_threshold_us,
            error_stats: ErrorStatsManager::new(),
            slow_query_logger: None,
            aggregated_stats: AggregatedStatsManager::new(),
            timeseries: Arc::new(RwLock::new(VecDeque::with_capacity(3600))),
            resource_series: Arc::new(RwLock::new(VecDeque::with_capacity(3600))),
            timeseries_capacity: Arc::new(AtomicUsize::new(3600)),
        }
    }

    /// Create StatsManager with slow query logger
    pub fn with_slow_query_logger(
        monitoring_enabled: bool,
        profile_cache_size: usize,
        slow_query_threshold_us: u64,
        slow_query_config: SlowQueryConfig,
    ) -> Result<Self, std::io::Error> {
        let logger = Arc::new(SlowQueryLogger::new(slow_query_config)?);

        Ok(Self {
            metrics: Arc::new(DashMap::new()),
            space_metrics: Arc::new(DashMap::new()),
            index_metrics: Arc::new(DashMap::new()),
            last_query_metrics: Arc::new(RwLock::new(None)),
            query_metrics_history: Arc::new(RwLock::new(VecDeque::with_capacity(1000))),
            bloom_lock: Arc::new(parking_lot::Mutex::new(())),
            query_profiles: Arc::new(RwLock::new(VecDeque::with_capacity(profile_cache_size))),
            query_latency_histogram: Arc::new(RwLock::new(LatencyHistogram::new(10000))),
            search_latency_histogram: Arc::new(RwLock::new(LatencyHistogram::new(10000))),
            monitoring_enabled,
            profile_cache_size,
            slow_query_threshold_us,
            error_stats: ErrorStatsManager::new(),
            slow_query_logger: Some(logger),
            aggregated_stats: AggregatedStatsManager::new(),
            timeseries: Arc::new(RwLock::new(VecDeque::with_capacity(3600))),
            resource_series: Arc::new(RwLock::new(VecDeque::with_capacity(3600))),
            timeseries_capacity: Arc::new(AtomicUsize::new(3600)),
        })
    }

    pub fn slow_query_threshold_us(&self) -> u64 {
        self.slow_query_threshold_us
    }

    pub fn timeseries_capacity(&self) -> usize {
        self.timeseries_capacity.load(Ordering::Relaxed)
    }

    pub fn set_timeseries_capacity(&self, capacity: usize) {
        let capacity = capacity.clamp(60, 86400);
        self.timeseries_capacity.store(capacity, Ordering::Relaxed);
        let mut series = self.timeseries.write();
        while series.len() > capacity {
            series.pop_front();
        }
        let mut resources = self.resource_series.write();
        while resources.len() > capacity {
            resources.pop_front();
        }
    }

    pub fn set_histogram_max_samples(&self, max_samples: usize) {
        let max_samples = max_samples.clamp(100, 100_000);
        self.query_latency_histogram
            .write()
            .set_max_samples(max_samples);
        self.search_latency_histogram
            .write()
            .set_max_samples(max_samples);
    }

    pub(crate) fn record_timeseries_query(&self, latency_us: u64, is_error: bool) {
        let second = unix_secs_now();
        let capacity = self.timeseries_capacity();
        let mut series = self.timeseries.write();
        if let Some(back) = series.back_mut() {
            if back.second == second {
                back.queries += 1;
                back.total_latency_us += latency_us;
                if is_error {
                    back.errors += 1;
                }
                return;
            }
        }
        series.push_back(TimeseriesBucket {
            second,
            queries: 1,
            total_latency_us: latency_us,
            errors: if is_error { 1 } else { 0 },
        });
        while series.len() > capacity {
            series.pop_front();
        }
    }

    pub fn record_resource_sample(
        &self,
        memory_used_bytes: u64,
        memory_total_bytes: u64,
        cpu_usage_percent: f64,
    ) {
        let second = unix_secs_now();
        let capacity = self.timeseries_capacity();
        let mut series = self.resource_series.write();
        if let Some(back) = series.back_mut() {
            if back.second == second {
                back.memory_used_bytes = memory_used_bytes;
                back.memory_total_bytes = memory_total_bytes;
                back.cpu_usage_percent = cpu_usage_percent;
                return;
            }
        }
        series.push_back(ResourceSample {
            second,
            memory_used_bytes,
            memory_total_bytes,
            cpu_usage_percent,
        });
        while series.len() > capacity {
            series.pop_front();
        }
    }

    pub fn query_timeseries(&self, window_secs: u64) -> Vec<TimeseriesBucket> {
        let cutoff = unix_secs_now().saturating_sub(window_secs);
        self.timeseries
            .read()
            .iter()
            .filter(|b| b.second >= cutoff)
            .cloned()
            .collect()
    }

    pub fn resource_timeseries(&self, window_secs: u64) -> Vec<ResourceSample> {
        let cutoff = unix_secs_now().saturating_sub(window_secs);
        self.resource_series
            .read()
            .iter()
            .filter(|s| s.second >= cutoff)
            .cloned()
            .collect()
    }
}

impl Default for StatsManager {
    fn default() -> Self {
        Self::new()
    }
}
