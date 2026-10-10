//! Spill-to-disk infrastructure for blocking operators.
//!
//! Provides:
//! - `SpillConfig / SpillManager`: temp-file lifecycle management
//! - `SpilledRun / RunWriter / RunReader`: the single spill file format —
//!   columnar v2 run (`LRSC`): versioned header, schema fingerprint,
//!   per-section checksums, optional per-section zstd body (columns are
//!   contiguous postcard-encoded value slices, one section per column group)
//! - `HashPartitionSpiller`: per-partition run writers
//! - `DiskQuota`: separate disk usage tracking for spill operations

mod config;
mod hash_partition;
mod manager;
mod quota;
mod run_format;
mod run_io;

pub use config::{
    SpillConfig, COLLECTOR_RUN_ROWS_MAX, COLLECTOR_SPILL_ROWS_DEFAULT, HASH_JOIN_MAX_DEPTH,
    HASH_JOIN_PARTITIONS_DEFAULT,
};
pub use hash_partition::{
    hash_bytes_partition, hash_column_partition, hash_row_partition, HashPartitionConfig,
    HashPartitionSpiller, HASH_PARTITION_SEED, HASH_PARTITION_VERSION,
};
pub use manager::finalize_partitions_with_runtime;
pub use manager::SpillManager;
pub use quota::DiskQuota;
pub use run_format::{schema_fingerprint, RunCompression, RunHeader, SpilledRun};
pub use run_io::{RunReader, RunWriter};

#[cfg(test)]
mod tests;
