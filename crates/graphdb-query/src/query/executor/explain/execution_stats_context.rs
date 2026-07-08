//! Execution Statistics Context
//!
//! Used to collect execution statistics for all nodes during EXPLAIN ANALYZE and PROFILE operations.
//! Provides global statistics management and per-node statistics collection.

use std::collections::HashMap;
use std::time::Instant;

use parking_lot::Mutex;

use crate::core::stats::utils::micros_to_millis;
use crate::query::executor::base::ExecutorStats;

/// Node-level execution statistics
#[derive(Debug, Clone)]
pub struct NodeExecutionStats {
    pub node_id: i64,
    pub executor_stats: ExecutorStats,
    pub startup_time_us: u64,
}

impl NodeExecutionStats {
    pub fn new(node_id: i64) -> Self {
        Self {
            node_id,
            executor_stats: ExecutorStats::default(),
            startup_time_us: 0,
        }
    }

    pub fn actual_rows(&self) -> usize {
        self.executor_stats.num_rows
    }

    pub fn actual_time_us(&self) -> u64 {
        self.executor_stats.exec_time_us
    }

    pub fn actual_time_ms(&self) -> f64 {
        micros_to_millis(self.executor_stats.exec_time_us)
    }

    pub fn memory_used(&self) -> usize {
        self.executor_stats.memory_peak
    }

    pub fn cache_hit_rate(&self) -> f64 {
        // Cache statistics removed in phase 1 of metrics migration
        // See docs/stat/metrics_migration_plan.md for details
        0.0
    }
}

impl Default for NodeExecutionStats {
    fn default() -> Self {
        Self::new(0)
    }
}

/// Global execution statistics
#[derive(Debug, Clone, Default)]
pub struct GlobalExecutionStats {
    pub planning_time_us: u64,
    pub execution_time_us: u64,
    pub total_rows: usize,
    pub peak_memory: usize,
    pub cache_hit_rate: f64,
}

impl GlobalExecutionStats {
    pub fn planning_time_ms(&self) -> f64 {
        micros_to_millis(self.planning_time_us)
    }

    pub fn execution_time_ms(&self) -> f64 {
        micros_to_millis(self.execution_time_us)
    }
}

/// Execution statistics context
///
/// Manages statistics collection for all plan nodes during query execution.
/// Used by EXPLAIN ANALYZE and PROFILE to gather actual execution data.
pub struct ExecutionStatsContext {
    node_stats: Mutex<HashMap<i64, NodeExecutionStats>>,
    global_stats: Mutex<GlobalExecutionStats>,
    start_time: Instant,
}

impl ExecutionStatsContext {
    pub fn new() -> Self {
        Self {
            node_stats: Mutex::new(HashMap::new()),
            global_stats: Mutex::new(GlobalExecutionStats::default()),
            start_time: Instant::now(),
        }
    }

    pub fn with_planning_time(planning_time_ms: f64) -> Self {
        let ctx = Self::new();
        ctx.global_stats.lock().planning_time_us = (planning_time_ms * 1000.0) as u64;
        ctx
    }

    pub fn on_node_start(&self, node_id: i64) {
        let mut stats = self.node_stats.lock();
        stats
            .entry(node_id)
            .or_insert_with(|| NodeExecutionStats::new(node_id));
    }

    pub fn on_node_complete(&self, node_id: i64, executor_stats: ExecutorStats) {
        let mut stats = self.node_stats.lock();
        let node_stats = NodeExecutionStats {
            node_id,
            executor_stats,
            startup_time_us: 0,
        };
        stats.insert(node_id, node_stats);
    }

    pub fn record_node_rows(&self, node_id: i64, rows: usize) {
        let mut stats = self.node_stats.lock();
        if let Some(s) = stats.get_mut(&node_id) {
            s.executor_stats.num_rows = rows;
        }
    }

    pub fn record_node_time(&self, node_id: i64, time_us: u64) {
        let mut stats = self.node_stats.lock();
        if let Some(s) = stats.get_mut(&node_id) {
            s.executor_stats.exec_time_us = time_us;
        }
    }

    pub fn record_startup_time(&self, node_id: i64, startup_time_us: u64) {
        let mut stats = self.node_stats.lock();
        if let Some(s) = stats.get_mut(&node_id) {
            s.startup_time_us = startup_time_us;
        }
    }

    pub fn record_global_execution_time(&self, time_us: u64) {
        self.global_stats.lock().execution_time_us = time_us;
    }

    pub fn collect_stats(&self) -> HashMap<i64, NodeExecutionStats> {
        self.node_stats.lock().clone()
    }

    pub fn get_node_stats(&self, node_id: i64) -> Option<NodeExecutionStats> {
        self.node_stats.lock().get(&node_id).cloned()
    }

    pub fn get_global_stats(&self) -> GlobalExecutionStats {
        self.global_stats.lock().clone()
    }

    pub fn total_elapsed_ms(&self) -> f64 {
        self.start_time.elapsed().as_micros() as f64 / 1000.0
    }
}

impl Default for ExecutionStatsContext {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_execution_stats_context() {
        let ctx = ExecutionStatsContext::new();

        ctx.on_node_start(1);
        let exec_stats = ExecutorStats {
            num_rows: 100,
            exec_time_us: 5500,
            ..ExecutorStats::default()
        };
        ctx.on_node_complete(1, exec_stats);

        let collected = ctx.collect_stats();
        assert_eq!(collected.get(&1).unwrap().actual_rows(), 100);
        assert!((collected.get(&1).unwrap().actual_time_ms() - 5.5).abs() < 0.001);
    }
}
