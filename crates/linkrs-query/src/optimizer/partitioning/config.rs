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
    /// Gray-release allowlist for aggregate-rooted shapes.
    ///
    /// The master `enabled` switch stays default-off; turning on one of
    /// these flags additionally admits that shape through the disabled
    /// gate so a single query class can be partitioned without enabling
    /// partitioning globally. Every other gate (storage self-proven
    /// domain, row thresholds, structural rejects) still applies.
    /// Pure-count chains go first because they carry no grouping state;
    /// global aggregates follow once per-partition aggregation is proven.
    /// Both default to false, so default behavior is unchanged.
    pub allow_pure_count_chain: bool,
    pub allow_global_aggregate: bool,
    /// Gray-release switch for the narrow rowless expand front end: a
    /// single-step de-materialized hop feeding count, group-count, or ordered
    /// distinct. Other shapes keep the serial path even when this is on.
    pub allow_ordered_parallel_expand: bool,
    /// Admit count-only expand frontiers whose partition-local result is a
    /// scalar count merged by the existing `SUM(_expand_count)` rewrite.
    pub allow_parallel_count_frontier: bool,
    /// Admit storage metadata counts that are proven disjoint per partition.
    /// The storage proof is a separate prerequisite; this switch alone never
    /// changes a plan.
    pub allow_partition_metadata_count: bool,
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
            allow_pure_count_chain: false,
            allow_global_aggregate: false,
            allow_ordered_parallel_expand: false,
            allow_parallel_count_frontier: false,
            allow_partition_metadata_count: false,
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
