use crate::planning::plan::PartitionSpec;

/// Static configuration for partition selection. The default is disabled so
/// introducing the optimizer cannot change query results without an explicit
/// self-proven layout source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartitioningConfig {
    pub enabled: bool,
    pub min_rows_per_partition: u64,
    pub max_partitions: usize,
    /// Fallback vertex ID range used when the storage cannot self-prove a
    /// domain. Ranges use `i64` to match the real vertex ID type and avoid
    /// silent truncation of values >= 2^32.
    pub vertex_id_range: Option<std::ops::Range<i64>>,
    /// Maximum worker threads for intra-query parallelism.
    /// 1 means fully serial.
    pub max_workers: usize,
    /// Maximum queued chunks per partition worker for backpressure.
    pub max_buffered_chunks: usize,
}

impl Default for PartitioningConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            min_rows_per_partition: 100_000,
            max_partitions: 1,
            vertex_id_range: None,
            max_workers: 1,
            max_buffered_chunks: 10,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartitioningDecision {
    pub partition_spec: Option<PartitionSpec>,
    pub reason: String,
}

/// Storage-provided layout information read at optimize time.
///
/// The planner no longer trusts a caller-supplied vertex-id range blindly:
/// `vertex_id_range` is the storage's **self-proven** domain (see
/// `StorageReader::vertex_id_domain`), and `layout_version` is the storage's
/// monotonic physical layout version (see `StorageReader::layout_version`).
/// When the storage cannot prove a domain, the configured range is used as a
/// fallback; when neither exists, partitioning falls back (safe default).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PartitioningLayoutInfo {
    /// Monotonic storage layout version (0 = not provided).
    pub layout_version: u64,
    /// Storage self-proven vertex-id domain covering the space.
    pub vertex_id_range: Option<std::ops::Range<i64>>,
}
