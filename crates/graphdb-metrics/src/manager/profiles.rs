//! Query portrait ring and slow-query logging.
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::profile::QueryProfile;
use crate::utils::micros_to_millis;

use super::core::StatsManager;

/// Slow query statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlowQueryStats {
    pub total: u64,
    pub min_duration_secs: f64,
    pub max_duration_secs: f64,
    pub avg_duration_secs: f64,
}

/// Executor rollup for handler responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutorSummary {
    pub executor_type: String,
    pub count: u64,
    pub total_time_ms: u64,
    pub total_rows: u64,
    pub avg_time_ms: f64,
}

impl StatsManager {
    pub fn record_query_profile(&self, profile: QueryProfile) {
        if !self.monitoring_enabled {
            return;
        }

        let is_failed = matches!(profile.status, crate::profile::QueryStatus::Failed);
        let is_slow = profile.total_duration_us >= self.slow_query_threshold_us;
        if is_slow {
            self.write_slow_query_log(&profile);
        } else {
            self.aggregated_stats.record_query(&profile, false);
        }
        self.record_timeseries_query(profile.total_duration_us, is_failed);

        let mut profiles = self.query_profiles.write();
        if profiles.len() >= self.profile_cache_size {
            profiles.pop_front();
        }
        profiles.push_back(profile);
    }

    pub(crate) fn write_slow_query_log(&self, profile: &QueryProfile) {
        // Record to aggregated stats
        self.aggregated_stats.record_query(profile, true);

        if let Some(ref logger) = self.slow_query_logger {
            logger.log(profile);
        } else {
            self.write_slow_query_log_fallback(profile);
        }
    }

    fn write_slow_query_log_fallback(&self, profile: &QueryProfile) {
        let executor_summary: Vec<String> = profile
            .executor_stats
            .iter()
            .map(|stat| {
                format!(
                    "{}[id={}, {}ms, rows={}, mem={}]",
                    stat.executor_type,
                    stat.executor_id,
                    stat.duration_ms(),
                    stat.rows_processed(),
                    stat.memory_used()
                )
            })
            .collect();

        let error_str = if let Some(ref info) = profile.error_info {
            format!(
                " [error={} phase={}]: {}",
                info.error_type, info.error_phase, info.error_message
            )
        } else if let Some(ref msg) = profile.error_message {
            format!(" [error]: {}", msg)
        } else {
            String::new()
        };

        log::warn!(
            "Slow query [trace_id={}] [session_id={}] [duration={}ms] [status={}]\n\
Queries: {}\n\
Stage statistics: parse={}ms validate={}ms plan={}ms optimize={}ms execute={}ms\n\
Number of results: {} Number of executors: {} Total executor time: {}ms\n\\\
Executor details: {}{}",
            profile.trace_id,
            profile.session_id,
            micros_to_millis(profile.total_duration_us),
            match profile.status {
                crate::profile::QueryStatus::Success => "success",
                crate::profile::QueryStatus::Failed => "failed",
            },
            profile.query_text,
            profile.stages.parse_ms(),
            profile.stages.validate_ms(),
            profile.stages.plan_ms(),
            profile.stages.optimize_ms(),
            profile.stages.execute_ms(),
            profile.result_count,
            profile.executor_stats.len(),
            profile.total_executor_time_ms(),
            executor_summary.join(", "),
            error_str
        );
    }

    pub fn get_recent_queries(&self, limit: usize) -> Vec<QueryProfile> {
        let profiles = self.query_profiles.read();
        profiles.iter().rev().take(limit).cloned().collect()
    }

    pub fn get_slow_queries(&self, limit: usize) -> Vec<QueryProfile> {
        let profiles = self.query_profiles.read();
        profiles
            .iter()
            .filter(|p| p.total_duration_us >= self.slow_query_threshold_us)
            .rev()
            .take(limit)
            .cloned()
            .collect()
    }

    pub fn get_all_slow_queries(&self, limit: usize) -> SlowQueryStats {
        let profiles = self.query_profiles.read();
        let slow_queries: Vec<_> = profiles
            .iter()
            .filter(|p| p.total_duration_us >= self.slow_query_threshold_us)
            .take(limit)
            .cloned()
            .collect();

        let total = slow_queries.len() as u64;
        let durations: Vec<f64> = slow_queries
            .iter()
            .map(|p| p.total_duration_us as f64 / 1_000_000.0)
            .collect();

        let (min, max, avg) = if durations.is_empty() {
            (0.0, 0.0, 0.0)
        } else {
            let min = durations.iter().cloned().fold(f64::INFINITY, f64::min);
            let max = durations.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            let avg = durations.iter().sum::<f64>() / durations.len() as f64;
            (min, max, avg)
        };

        SlowQueryStats {
            total,
            min_duration_secs: min,
            max_duration_secs: max,
            avg_duration_secs: avg,
        }
    }

    pub fn get_query_profile(&self, trace_id: &str) -> Option<QueryProfile> {
        let profiles = self.query_profiles.read();
        profiles.iter().find(|p| p.trace_id == trace_id).cloned()
    }

    pub fn get_session_queries(&self, session_id: i64, limit: usize) -> Vec<QueryProfile> {
        let profiles = self.query_profiles.read();
        profiles
            .iter()
            .filter(|p| p.session_id == session_id)
            .rev()
            .take(limit)
            .cloned()
            .collect()
    }

    pub fn get_executor_stats_summary(&self) -> HashMap<String, (u64, u64, usize)> {
        let profiles = self.query_profiles.read();
        let mut stats: HashMap<String, (u64, u64, usize)> = HashMap::new();

        for profile in profiles.iter() {
            for exec_stat in &profile.executor_stats {
                let entry = stats
                    .entry(exec_stat.executor_type.clone())
                    .or_insert((0, 0, 0));
                entry.0 += exec_stat.duration_ms() as u64;
                entry.1 += exec_stat.rows_processed() as u64;
                entry.2 += 1;
            }
        }

        stats
    }

    pub fn executor_summary_snapshot(&self) -> Vec<ExecutorSummary> {
        let mut out: Vec<ExecutorSummary> = self
            .get_executor_stats_summary()
            .into_iter()
            .map(|(executor_type, (total_time_ms, total_rows, count))| {
                let avg_time_ms = if count == 0 {
                    0.0
                } else {
                    total_time_ms as f64 / count as f64
                };
                ExecutorSummary {
                    executor_type,
                    count: count as u64,
                    total_time_ms,
                    total_rows,
                    avg_time_ms,
                }
            })
            .collect();
        out.sort_by(|a, b| b.total_time_ms.cmp(&a.total_time_ms));
        out
    }

    pub fn profiles_in_window(
        &self,
        from_secs: Option<u64>,
        to_secs: Option<u64>,
        limit: usize,
    ) -> Vec<QueryProfile> {
        let profiles = self.query_profiles.read();
        profiles
            .iter()
            .rev()
            .filter(|p| {
                if let Some(from) = from_secs {
                    if p.started_at_secs < from {
                        return false;
                    }
                }
                if let Some(to) = to_secs {
                    if p.started_at_secs > to {
                        return false;
                    }
                }
                true
            })
            .take(limit)
            .cloned()
            .collect()
    }

    pub fn clear_query_cache(&self) {
        let mut profiles = self.query_profiles.write();
        profiles.clear();
    }

    pub fn query_cache_size(&self) -> usize {
        let profiles = self.query_profiles.read();
        profiles.len()
    }
}
