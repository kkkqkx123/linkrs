use linkrs_core::columnar::MaterializedBatch;
use linkrs_core::error::QueryError;
use linkrs_core::Value;

use super::manager::SpillManager;
use super::run_format::fnv1a_64_update;
use super::run_io::{RunReader, RunWriter};
use super::run_format::SpilledRun;

// ── Hash partition spill ─────────────────────────────────────────────────────

/// Fixed hash algorithm for partition spill operations.
///
/// Uses a simple FNV-1a hash of the serialized row/key data.
/// The seed and algorithm version are part of the contract—changing
/// them will change partition assignments across all operators.
pub const HASH_PARTITION_VERSION: u32 = 1;
pub const HASH_PARTITION_SEED: u64 = 0xdeadbeefcafe;

/// Compute the partition index for a row of values.
///
/// Used by hash join, hash aggregate, and hash distinct to
/// distribute rows across partitions.
pub fn hash_row_partition(row: &[Value], num_partitions: u64) -> u64 {
    // Serialize the row to bytes and hash
    let encoded = postcard::to_allocvec(row).unwrap_or_default();
    let hash = fnv1a_64_update(HASH_PARTITION_SEED, &encoded);
    hash % num_partitions
}

/// Compute the partition index for pre-serialized key bytes.
///
/// Shared primitive behind [`hash_row_partition`] and join-key partitioning
/// so both sides of a Grace Hash Join agree on partition assignment.
pub fn hash_bytes_partition(bytes: &[u8], num_partitions: u64) -> u64 {
    if num_partitions == 0 {
        return 0;
    }
    fnv1a_64_update(HASH_PARTITION_SEED, bytes) % num_partitions
}

/// Compute one partition index per batch row over a column subset.
///
/// Key-column projection of [`hash_row_partition`]: rows are hashed by the
/// selected columns only (empty `cols` means the full row), so Distinct and
/// Aggregate spill by key instead of by full-row bytes.
pub fn hash_column_partition(
    batch: &MaterializedBatch,
    cols: &[usize],
    num_partitions: u64,
) -> Vec<u64> {
    batch
        .hash_rows(cols)
        .into_iter()
        .map(|h| {
            if num_partitions == 0 {
                0
            } else {
                h % num_partitions
            }
        })
        .collect()
}

/// Configuration for hash-based partition spill.
#[derive(Debug, Clone)]
pub struct HashPartitionConfig {
    /// Number of partitions to create.
    pub num_partitions: u64,
    /// Maximum rows per partition before triggering recursive repartition.
    pub max_rows_per_partition: u64,
    /// Maximum recursion depth for skew handling.
    pub max_recursion_depth: u32,
}

impl Default for HashPartitionConfig {
    fn default() -> Self {
        Self {
            num_partitions: 16,
            max_rows_per_partition: 1_000_000,
            max_recursion_depth: 3,
        }
    }
}

/// A partition spill writer that routes rows by hash into separate files.
///
/// Each partition gets its own `RunWriter` for sorted/typed row data.
/// The spiller handles skew detection and recursive repartitioning.
#[derive(Debug)]
pub struct HashPartitionSpiller {
    config: HashPartitionConfig,
    pub(super) writers: Vec<Option<RunWriter>>,
    counts: Vec<u64>,
    recursion_depth: u32,
    schema_fingerprint: u64,
}

impl HashPartitionSpiller {
    /// Create a new hash partition spiller.
    pub fn new(
        config: HashPartitionConfig,
        manager: &SpillManager,
        schema_fingerprint: u64,
    ) -> Result<Self, QueryError> {
        let n = config.num_partitions as usize;
        let mut writers = Vec::with_capacity(n);
        for _ in 0..n {
            writers.push(Some(manager.create_run_writer(schema_fingerprint)?));
        }
        Ok(Self {
            config,
            writers,
            counts: vec![0; n],
            recursion_depth: 0,
            schema_fingerprint,
        })
    }

    /// Insert a row into the appropriate partition.
    pub fn insert_row(&mut self, row: &[Value], manager: &SpillManager) -> Result<(), QueryError> {
        let partition = hash_row_partition(row, self.config.num_partitions) as usize;
        self.insert_row_to_partition(row, partition, manager)
    }

    /// Insert a row into a specific partition (caller computes the hash).
    ///
    /// Useful when the caller needs to partition by a derived key
    /// (e.g., group key for aggregate) rather than the row itself.
    pub fn insert_row_to_partition(
        &mut self,
        row: &[Value],
        partition: usize,
        manager: &SpillManager,
    ) -> Result<(), QueryError> {
        if let Some(Some(writer)) = self.writers.get_mut(partition) {
            writer.write_row(row)?;
            self.counts[partition] += 1;

            if self.counts[partition] > self.config.max_rows_per_partition
                && self.recursion_depth < self.config.max_recursion_depth
            {
                self.repartition(manager)?;
            }
        }
        Ok(())
    }

    /// Finalize all partitions through the manager so disk quota is enforced.
    pub fn finalize_with_manager(
        mut self,
        manager: &SpillManager,
    ) -> Result<Vec<Option<SpilledRun>>, QueryError> {
        let mut runs = Vec::with_capacity(self.writers.len());
        for writer in self.writers.drain(..) {
            match writer {
                Some(w) => runs.push(Some(manager.finalize_run(w)?)),
                None => runs.push(None),
            }
        }
        Ok(runs)
    }

    /// Recursively repartition when skew is detected.
    ///
    /// This splits the overflowing partition into sub-partitions by
    /// re-hashing with an increased partition count. Intermediate runs go
    /// through [`SpillManager::finalize_run`] so disk quota applies; replaced
    /// files are unlinked and their reservations released.
    fn repartition(&mut self, manager: &SpillManager) -> Result<(), QueryError> {
        self.recursion_depth += 1;
        let _old_count = self.config.num_partitions;
        self.config.num_partitions = self.config.num_partitions.saturating_mul(2);

        // Finalize current writers to get run files (quota-checked).
        let old_runs = self.finalize_current(manager)?;

        // Create new writers for doubled partitions
        let mut new_writers = Vec::with_capacity(self.config.num_partitions as usize);
        for _ in 0..self.config.num_partitions {
            new_writers.push(Some(manager.create_run_writer(self.schema_fingerprint)?));
        }
        let mut new_counts = vec![0u64; self.config.num_partitions as usize];

        // Read back and rehash all old partitions into new partitions
        for old_run in old_runs.into_iter().flatten() {
            let byte_size = old_run.byte_size;
            let mut reader = RunReader::open(&old_run)?;
            while let Some(row) = reader.read_row()? {
                let partition = hash_row_partition(&row, self.config.num_partitions) as usize;
                if let Some(Some(writer)) = new_writers.get_mut(partition) {
                    writer.write_row(&row)?;
                    new_counts[partition] += 1;
                }
            }
            // Delete old partition file and release its quota reservation:
            // the data now lives in the new partitions.
            let _ = std::fs::remove_file(&old_run.path);
            manager.disk_quota().release(byte_size);
        }

        self.writers = new_writers;
        self.counts = new_counts;
        Ok(())
    }

    /// Finalize all current partition writers and return the run metadata.
    ///
    /// Quota-checked via [`SpillManager::finalize_run`]; intermediate
    /// repartition files are accounted exactly like final runs.
    fn finalize_current(
        &mut self,
        manager: &SpillManager,
    ) -> Result<Vec<Option<SpilledRun>>, QueryError> {
        let mut runs = Vec::with_capacity(self.writers.len());
        for writer in self.writers.drain(..) {
            match writer {
                Some(w) => runs.push(Some(manager.finalize_run(w)?)),
                None => runs.push(None),
            }
        }
        Ok(runs)
    }

    /// Current partition row counts.
    pub fn partition_counts(&self) -> &[u64] {
        &self.counts
    }

    /// Number of partitions.
    pub fn num_partitions(&self) -> u64 {
        self.config.num_partitions
    }
}
