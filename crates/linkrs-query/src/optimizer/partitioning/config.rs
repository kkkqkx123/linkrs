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
    /// Parallel fixed startup overhead in row-equivalents (fragment startup,
    /// exchange coordination). All-zero cost params disable the economic
    /// gate and leave the row-count threshold as the only gate.
    pub fixed_overhead_rows: u64,
    /// Per-partition fixed overhead in row-equivalents.
    pub per_partition_rows: u64,
    /// Exchange cost per output row in row-equivalents, per mille
    /// (50 = 0.05 row-equivalents per row crossing the gather).
    pub exchange_row_per_mille: u32,
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
            fixed_overhead_rows: 0,
            per_partition_rows: 0,
            exchange_row_per_mille: 0,
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
