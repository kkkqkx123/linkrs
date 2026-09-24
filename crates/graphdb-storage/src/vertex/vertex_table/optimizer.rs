//! Vertex Table Optimizer
//!
//! Handles compaction and ID remapping.
//!
//! # Optimizations
//! - Batch timestamp checks during compaction via CompactionCoordinator
//! - Range-based column copying instead of row-by-row operations

use super::compaction::{CompactionCoordinator, CompactionJournal};
use super::core::VertexTable;
use crate::vertex::IdKey;
use graphdb_core::StorageResult;
use std::collections::HashMap;

impl VertexTable {
    /// Compact vertices deleted at or before `cutoff` and return the removed
    /// external keys, the old-to-new internal ID mapping, and the journal of
    /// the coordinated execution.
    ///
    /// The cutoff must be derived from the global GC watermarks. Callers
    /// pass the watermark safe timestamp, never a bare transaction stamp.
    /// The mapping is required by callers that propagate the remap to
    /// dependent row-indexed structures (edge CSR rows) so vertex references
    /// stay stable; the journal lets the maintenance layer extend the same
    /// commit record across the edge rewrite.
    pub fn compact_with_cutoff_collect_mapping(
        &mut self,
        cutoff: graphdb_core::types::Timestamp,
    ) -> StorageResult<(Vec<IdKey>, HashMap<u32, u32>, CompactionJournal)> {
        let deleted_ids: Vec<u32> = self.timestamps.iter_deleted(cutoff).collect();

        let mut removed_keys = Vec::with_capacity(deleted_ids.len());

        for id in &deleted_ids {
            if let Some(key) = self.id_indexer.get_key(*id) {
                self.id_indexer.remove(&key);
                removed_keys.push(key);
            }
        }

        let mut coordinator = CompactionCoordinator::new();
        coordinator.execute(self)?;
        if coordinator.journal().is_committed() {
            log::debug!(
                "vertex compact committed: remapped={} steps={:?}",
                !coordinator.id_mapping().is_empty(),
                coordinator.journal().steps(),
            );
        }

        Ok((
            removed_keys,
            coordinator.id_mapping().clone(),
            coordinator.journal().clone(),
        ))
    }

    /// Stable row-id collection: watermark-gated hole absorption without
    /// moving any live row.
    ///
    /// Long-term compaction policy: deleted keys are pushed to the free stack
    /// for reuse by new inserts, version chains are left to the caller's
    /// fold pass, and no timestamp/column/index remap runs. The returned
    /// mapping is always empty, so the maintenance layer must produce zero
    /// edge endpoint rewrites (see the zero-rewrite assertion there).
    /// Shard-count manifests stay the identifier-decoding anchor.
    pub fn compact_with_cutoff_stable_collect(
        &mut self,
        cutoff: graphdb_core::types::Timestamp,
    ) -> StorageResult<(Vec<IdKey>, HashMap<u32, u32>, CompactionJournal)> {
        let deleted_ids: Vec<u32> = self.timestamps.iter_deleted(cutoff).collect();
        let mut removed_keys = Vec::with_capacity(deleted_ids.len());
        for id in &deleted_ids {
            if let Some(key) = self.id_indexer.get_key(*id) {
                self.id_indexer.remove(&key);
                removed_keys.push(key);
            }
        }
        Ok((removed_keys, HashMap::new(), CompactionJournal::default()))
    }

    /// Compact the vertex table using the unified CompactionCoordinator
    ///
    /// Crate-internal re-layout step used by watermark-gated compaction
    /// paths. External callers must go through the watermark-gated
    /// collection mapping entry point instead of calling this directly.
    ///
    /// # Unified Coordination
    ///
    /// CompactionCoordinator ensures atomic coordination of three internal structures:
    /// - **id_indexer**: Key↔ID mapping (authoritative source)
    /// - **timestamps**: MVCC visibility tracking ([start_ts, end_ts) ranges)
    /// - **columns**: Property data in columnar format
    ///
    /// # Process
    ///
    /// 1. Get authoritative ID mapping from id_indexer.compact()
    /// 2. Propagate remapping to timestamps (if any IDs moved)
    /// 3. Propagate remapping to columns (if any IDs moved)
    /// 4. Resize columns to match new id_indexer size
    /// 5. Verify all invariants (debug builds only)
    ///
    /// # Atomicity Guarantee
    ///
    /// The coordinated execution is atomic within the table: the dense
    /// mapping is computed without mutating state, timestamp and column
    /// replacements are built before either is swapped in, and any failure
    /// restores the pre-compaction index snapshot. The caller must hold the
    /// commit barrier (shard write lock, extended to the write gate at the
    /// maintenance layer) across the vertex remap and the edge endpoint
    /// rewrite that consumes the returned mapping.
    ///
    /// # Invariants Maintained
    ///
    /// After successful compaction:
    /// - Every id_indexer entry has a corresponding timestamps entry
    /// - Every timestamps entry has a corresponding id_indexer entry (no orphans)
    /// - columns.row_count() == id_indexer.len()
    /// - All property data is preserved in new positions
    ///
    /// # Performance
    ///
    /// - Time complexity: O(n) in number of vertices
    /// - Space complexity: O(n) for temporary remapping structures
    /// - Exclusive access required (no concurrent reads)
    /// - Space reclamation is eager (arrays truncated immediately)
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Basic usage
    /// table.compact_coordinated()?;
    ///
    /// // With timing/logging
    /// let start = std::time::Instant::now();
    /// table.compact_coordinated()?;
    /// log::info!("Compaction took {:?}", start.elapsed());
    /// ```
    pub(crate) fn compact_coordinated(&mut self) -> StorageResult<()> {
        let mut coordinator = super::compaction::CompactionCoordinator::new();
        coordinator.execute(self)
    }
}
