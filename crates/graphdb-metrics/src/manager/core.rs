//! Facade owner: state layout, construction and scoping keys.
//!
//! Holds every backing store but implements no recording policy itself.
//! Counter primitives live in `counters`, query observability in `profiles`
//! and `query_metrics`, error and pattern delegation in `errors` and
//! `aggregated`, and per-subsystem recorders in their own files.

use dashmap::DashMap;
use parking_lot::RwLock;
use std::collections::VecDeque;
use std::sync::Arc;

use crate::aggregated_stats::AggregatedStatsManager;
use crate::error_stats::ErrorStatsManager;
use crate::latency_histogram::LatencyHistogram;
use crate::metrics::QueryMetrics;
use crate::profile::QueryProfile;
use crate::slow_query_logger::{SlowQueryConfig, SlowQueryLogger};

use super::metric_type::MetricType;
use super::metric_value::SpaceMetrics;

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
        })
    }
}

impl Default for StatsManager {
    fn default() -> Self {
        Self::new()
    }
}
