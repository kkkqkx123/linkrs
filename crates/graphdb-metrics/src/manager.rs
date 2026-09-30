//! Statistics Manager (facade).
//!
//! Provides unified management of query metrics, query portraits and error statistics.
//!
//! The implementation is split by responsibility under `manager/`:
//! `metric_type` declares the registry, `metric_value` the atomic primitive,
//! `core` owns state and construction, `counters` the primitive store,
//! `profiles` and `query_metrics` the query observability, `errors` and
//! `aggregated` the delegation to dedicated managers, and `search`,
//! `storage`, `transaction`, `sync`, `checkpoint`, `migration` the
//! per-subsystem recorders. This file only declares submodules and
//! re-exports the public surface so existing `manager::` paths keep working.

mod aggregated;
mod checkpoint;
mod core;
mod counters;
mod errors;
mod metric_type;
mod metric_value;
mod migration;
mod profiles;
mod query_metrics;
mod search;
mod storage;
mod sync;
mod transaction;

pub use checkpoint::CheckpointTriggerReason;
pub use core::StatsManager;
pub use metric_type::MetricType;
pub use metric_value::MetricValue;
pub use profiles::SlowQueryStats;
pub use sync::OutboxState;
pub use transaction::TxnResourceMetrics;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::QueryProfile;

    #[test]
    fn test_stats_manager_creation() {
        let stats = StatsManager::new();
        assert_eq!(stats.get_value(MetricType::NumQueries), None);

        stats.add_value(MetricType::NumQueries);
        assert_eq!(stats.get_value(MetricType::NumQueries), Some(1));
    }

    #[test]
    fn test_add_value() {
        let stats = StatsManager::new();
        stats.add_value(MetricType::NumQueries);
        assert_eq!(stats.get_value(MetricType::NumQueries), Some(1));

        stats.add_value(MetricType::NumQueries);
        assert_eq!(stats.get_value(MetricType::NumQueries), Some(2));
    }

    #[test]
    fn test_add_value_with_amount() {
        let stats = StatsManager::new();
        stats.add_value_with_amount(MetricType::NumQueries, 5);
        assert_eq!(stats.get_value(MetricType::NumQueries), Some(5));

        stats.add_value_with_amount(MetricType::NumQueries, 3);
        assert_eq!(stats.get_value(MetricType::NumQueries), Some(8));
    }

    #[test]
    fn test_dec_value() {
        let stats = StatsManager::new();
        stats.add_value_with_amount(MetricType::NumQueries, 10);
        assert_eq!(stats.get_value(MetricType::NumQueries), Some(10));

        stats.dec_value(MetricType::NumQueries);
        assert_eq!(stats.get_value(MetricType::NumQueries), Some(9));

        stats.dec_value(MetricType::NumQueries);
        assert_eq!(stats.get_value(MetricType::NumQueries), Some(8));
    }

    #[test]
    fn test_space_metrics() {
        let stats = StatsManager::new();
        stats.add_space_metric("test_space", MetricType::NumQueries);
        assert_eq!(
            stats.get_space_value("test_space", MetricType::NumQueries),
            Some(1)
        );

        stats.add_space_metric("test_space", MetricType::NumQueries);
        assert_eq!(
            stats.get_space_value("test_space", MetricType::NumQueries),
            Some(2)
        );

        stats.add_space_metric("other_space", MetricType::NumQueries);
        assert_eq!(
            stats.get_space_value("other_space", MetricType::NumQueries),
            Some(1)
        );
    }

    #[test]
    fn test_get_all_metrics() {
        let stats = StatsManager::new();
        stats.add_value(MetricType::NumQueries);
        stats.add_value(MetricType::NumActiveQueries);

        let all_metrics = stats.get_all_metrics();
        assert_eq!(all_metrics.get(&MetricType::NumQueries), Some(&1));
        assert_eq!(all_metrics.get(&MetricType::NumActiveQueries), Some(&1));
    }

    #[test]
    fn test_reset_metric() {
        let stats = StatsManager::new();
        stats.add_value_with_amount(MetricType::NumQueries, 10);
        assert_eq!(stats.get_value(MetricType::NumQueries), Some(10));

        stats.reset_metric(MetricType::NumQueries);
        assert_eq!(stats.get_value(MetricType::NumQueries), Some(0));
    }

    #[test]
    fn test_reset_all_metrics() {
        let stats = StatsManager::new();
        stats.add_value_with_amount(MetricType::NumQueries, 10);
        stats.add_value_with_amount(MetricType::NumActiveQueries, 3);

        stats.reset_all_metrics();

        assert_eq!(stats.get_value(MetricType::NumQueries), Some(0));
        assert_eq!(stats.get_value(MetricType::NumActiveQueries), Some(0));
    }

    #[test]
    fn test_record_and_get_query_profile() {
        let stats = StatsManager::with_config(true, 10, 1_000_000);

        let mut profile = QueryProfile::new(123, "MATCH (n) RETURN n".to_string());
        profile.total_duration_us = 500;
        profile.result_count = 10;

        stats.record_query_profile(profile.clone());

        assert_eq!(stats.query_cache_size(), 1);

        let recent = stats.get_recent_queries(1);
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].session_id, 123);
    }

    #[test]
    fn test_get_slow_queries() {
        let stats = StatsManager::with_config(true, 10, 1_000_000);

        let mut slow_profile = QueryProfile::new(1, "MATCH (n) RETURN n".to_string());
        slow_profile.total_duration_us = 2_000_000; // 2000ms in microseconds
        stats.record_query_profile(slow_profile);

        let mut fast_profile = QueryProfile::new(2, "MATCH (n) RETURN n LIMIT 1".to_string());
        fast_profile.total_duration_us = 100_000; // 100ms in microseconds
        stats.record_query_profile(fast_profile);

        let slow_queries = stats.get_slow_queries(10);
        assert_eq!(slow_queries.len(), 1);
        assert_eq!(slow_queries[0].session_id, 1);
    }

    #[test]
    fn test_query_cache_size_limit() {
        let stats = StatsManager::with_config(true, 3, 1_000_000);

        for i in 0..5 {
            let profile = QueryProfile::new(i as i64, format!("Query {}", i));
            stats.record_query_profile(profile);
        }

        assert_eq!(stats.query_cache_size(), 3);

        let recent = stats.get_recent_queries(3);
        assert_eq!(recent[0].session_id, 4);
        assert_eq!(recent[2].session_id, 2);
    }

    #[test]
    fn test_disabled_monitoring() {
        let stats = StatsManager::with_config(false, 10, 1_000_000);

        let profile = QueryProfile::new(123, "MATCH (n) RETURN n".to_string());
        stats.record_query_profile(profile);

        assert_eq!(stats.query_cache_size(), 0);
    }

    #[test]
    fn test_aggregated_stats_integration() {
        let stats = StatsManager::new();

        // Align to a wall-clock second boundary so both records land in the
        // same second and the QPS counter is deterministic (the counter
        // resets on each second boundary).
        let unix_secs = || {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
        };
        let current = unix_secs();
        while unix_secs() == current {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }

        // Create test profiles
        let mut profile1 =
            QueryProfile::new(1, "MATCH (n:Person) WHERE n.id = 1 RETURN n".to_string());
        profile1.total_duration_us = 1000;

        let mut profile2 =
            QueryProfile::new(2, "MATCH (n:Person) WHERE n.id = 2 RETURN n".to_string());
        profile2.total_duration_us = 2000;

        // Record queries
        stats.record_aggregated_query(&profile1, false);
        stats.record_aggregated_query(&profile2, false);

        // Verify stats
        assert_eq!(stats.get_total_aggregated_queries(), 2);
        assert_eq!(stats.get_pattern_count(), 1); // Same pattern
        assert_eq!(stats.get_current_qps(), 2);
    }

    #[test]
    fn test_slow_query_aggregated_recording() {
        let stats = StatsManager::with_config(true, 10, 1_000_000);

        // Create a slow query profile
        let mut profile =
            QueryProfile::new(1, "MATCH (n:Person) WHERE n.id = 1 RETURN n".to_string());
        profile.total_duration_us = 2_000_000; // 2000ms

        // This should trigger aggregated recording via write_slow_query_log
        stats.record_query_profile(profile.clone());

        // Verify aggregated stats were recorded
        assert_eq!(stats.get_total_aggregated_queries(), 1);
        assert_eq!(stats.get_total_aggregated_slow_queries(), 1);
    }

    #[test]
    fn test_get_top_n_slow_query_patterns() {
        let stats = StatsManager::new();

        // Record multiple queries with different patterns
        for i in 0..10 {
            let mut profile =
                QueryProfile::new(i, format!("MATCH (n:Person) WHERE n.id = {} RETURN n", i));
            profile.total_duration_us = 1000 + (i * 100) as u64;
            stats.record_aggregated_query(&profile, false);
        }

        // Get top 5 patterns
        let top_patterns = stats.get_top_n_slow_query_patterns(5);
        assert_eq!(top_patterns.len(), 1); // All same pattern
        assert_eq!(top_patterns[0].execution_count, 10);
    }

    #[test]
    fn test_clear_aggregated_stats() {
        let stats = StatsManager::new();

        let profile = QueryProfile::new(1, "MATCH (n) RETURN n".to_string());
        stats.record_aggregated_query(&profile, false);

        assert_eq!(stats.get_total_aggregated_queries(), 1);

        stats.clear_aggregated_stats();

        assert_eq!(stats.get_total_aggregated_queries(), 0);
        assert_eq!(stats.get_pattern_count(), 0);
    }

    #[test]
    fn test_checkpoint_metrics_recorded() {
        let stats = StatsManager::new();
        let duration = std::time::Duration::from_millis(150);
        stats.record_checkpoint_success(duration, 4096, 2);
        assert_eq!(stats.get_value(MetricType::CheckpointSuccessCount), Some(1));
        assert!(stats.get_value(MetricType::CheckpointDurationUs).unwrap() > 0);
        assert_eq!(
            stats.get_value(MetricType::CheckpointDataFlushedBytes),
            Some(4096)
        );
        assert_eq!(
            stats.get_value(MetricType::CheckpointWalFilesTruncated),
            Some(2)
        );

        stats.record_checkpoint_failure();
        assert_eq!(stats.get_value(MetricType::CheckpointFailureCount), Some(1));
    }

    #[test]
    fn test_checkpoint_trigger_reasons() {
        let stats = StatsManager::new();
        let age = std::time::Duration::from_secs(5);
        stats.record_checkpoint_trigger(CheckpointTriggerReason::WalSizeExceeded, age);
        assert_eq!(
            stats.get_value(MetricType::CheckpointTriggeredByWalSize),
            Some(1)
        );
        assert_eq!(stats.get_value(MetricType::CheckpointTriggerCount), Some(1));

        stats.record_checkpoint_trigger(CheckpointTriggerReason::TimeSinceLastCheckpoint, age);
        assert_eq!(
            stats.get_value(MetricType::CheckpointTriggeredByInterval),
            Some(1)
        );

        stats.record_checkpoint_trigger(CheckpointTriggerReason::Explicit, age);
        assert_eq!(
            stats.get_value(MetricType::CheckpointTriggeredExplicit),
            Some(1)
        );
        assert_eq!(stats.get_value(MetricType::CheckpointTriggerCount), Some(3));
    }

    #[test]
    fn test_checkpoint_dedup_and_blocked() {
        let stats = StatsManager::new();
        stats.record_checkpoint_deduplicated();
        stats.record_checkpoint_deduplicated();
        assert_eq!(
            stats.get_value(MetricType::CheckpointRequestsDeduplicated),
            Some(2)
        );
        stats.record_checkpoint_blocked();
        assert_eq!(
            stats.get_value(MetricType::CheckpointRequestsBlocked),
            Some(1)
        );
    }
}
