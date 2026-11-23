//! Parallel query execution configuration
//!
//! Controls intra-query parallel partitioning. The master switch stays off,
//! while the proven rowless single-hop shape is admitted through its narrow
//! gray-release flag with a conservative worker budget.

use serde::{Deserialize, Serialize};

/// Parallel query execution configuration.
///
/// Flattened into the top-level `[parallel]` TOML section.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct ParallelConfig {
    /// Master switch for intra-query parallel partitioning.
    pub enabled: bool,
    /// Worker threads for the shared scheduler pool.
    pub workers: usize,
    /// Minimum rows per partition before a scan qualifies for partitioning.
    pub min_rows_per_partition: u64,
    /// Maximum number of partitions to create for one scan.
    pub max_partitions: usize,
    /// Maximum queued chunks per partition worker (backpressure).
    pub max_buffered_chunks: usize,
    /// Optional trusted lower bound of the vertex-id range covering the scan
    /// domain. Both bounds must be set for a range to be active.
    pub vertex_id_start: Option<i64>,
    /// Optional trusted upper bound (exclusive) of the vertex-id range.
    pub vertex_id_end: Option<i64>,
    /// Gray-release allowlist: admit pure-count chains through the disabled
    /// gate without enabling partitioning globally.
    #[serde(default)]
    pub allow_pure_count_chain: bool,
    /// Gray-release allowlist: admit other aggregate-rooted plans through
    /// the disabled gate.
    #[serde(default)]
    pub allow_global_aggregate: bool,
    /// Gray-release allowlist: admit the de-materialized single-hop expand
    /// front end feeding count, group-count, or ordered distinct.
    #[serde(default)]
    pub allow_ordered_parallel_expand: bool,
    /// Gray-release allowlist: partition count-only expand frontiers whose
    /// partial results are scalar counts. Defaults to false.
    #[serde(default)]
    pub allow_parallel_count_frontier: bool,
    /// Gray-release allowlist for shard-local metadata counts. Stays off
    /// until storage can prove shard ownership and layout identity.
    #[serde(default)]
    pub allow_partition_metadata_count: bool,
}

impl Default for ParallelConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            workers: 8,
            min_rows_per_partition: 100_000,
            max_partitions: 8,
            max_buffered_chunks: 10,
            vertex_id_start: None,
            vertex_id_end: None,
            allow_pure_count_chain: false,
            allow_global_aggregate: false,
            allow_ordered_parallel_expand: true,
            allow_parallel_count_frontier: false,
            allow_partition_metadata_count: false,
        }
    }
}

impl ParallelConfig {
    /// Return the configured trusted vertex-id range, if both bounds are set
    /// and the range is non-empty.
    pub fn vertex_id_range(&self) -> Option<std::ops::Range<i64>> {
        match (self.vertex_id_start, self.vertex_id_end) {
            (Some(start), Some(end)) if start < end => Some(start..end),
            _ => None,
        }
    }

    /// Validate the configuration.
    pub fn validate(&self) -> Result<(), String> {
        if self.workers == 0 {
            return Err("Parallel workers must be greater than 0".to_string());
        }
        if self.min_rows_per_partition == 0 {
            return Err("Parallel min_rows_per_partition must be greater than 0".to_string());
        }
        if self.max_partitions == 0 {
            return Err("Parallel max_partitions must be greater than 0".to_string());
        }
        if let Some(range) = self.vertex_id_range() {
            if range.start < 0 || range.end <= range.start {
                return Err("Parallel vertex-id range must be non-empty".to_string());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parallel_config_default() {
        let config = ParallelConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.workers, 8);
        assert_eq!(config.min_rows_per_partition, 100_000);
        assert_eq!(config.max_partitions, 8);
        assert_eq!(config.max_buffered_chunks, 10);
        assert!(config.vertex_id_range().is_none());
        assert!(!config.enabled);
        assert!(config.allow_ordered_parallel_expand);
        assert!(!config.allow_pure_count_chain);
        assert!(!config.allow_global_aggregate);
        assert!(!config.allow_parallel_count_frontier);
    }

    #[test]
    fn test_parallel_config_validate() {
        let config = ParallelConfig::default();
        assert!(config.validate().is_ok());

        let invalid = ParallelConfig {
            workers: 0,
            ..Default::default()
        };
        assert!(invalid.validate().is_err());

        let range = ParallelConfig {
            enabled: true,
            workers: 4,
            vertex_id_start: Some(0),
            vertex_id_end: Some(100_000),
            ..Default::default()
        };
        assert!(range.validate().is_ok());
        assert_eq!(range.vertex_id_range(), Some(0..100_000));
    }
}
