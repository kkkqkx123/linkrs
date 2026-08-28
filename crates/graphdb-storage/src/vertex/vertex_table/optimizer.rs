//! Vertex Table Optimizer
//!
//! Handles compaction and ID remapping.
//!
//! # Optimizations
//! - Batch timestamp checks during compaction via CompactionCoordinator
//! - Range-based column copying instead of row-by-row operations

use super::compaction::CompactionCoordinator;
use super::core::VertexTable;
use graphdb_core::StorageResult;
use crate::vertex::IdKey;
use std::collections::HashMap;

impl VertexTable {
    /// Compact vertices deleted at or before `ts` and return both the removed
    /// external keys and the old-to-new internal ID mapping.
    ///
    /// The mapping is required by callers that propagate the remap to
    /// dependent row-indexed structures (edge CSR rows, frozen segments,
    /// cold snapshots) so vertex references stay stable.
    pub fn compact_with_ts_collect_mapping(
        &mut self,
        ts: graphdb_core::types::Timestamp,
    ) -> StorageResult<(Vec<IdKey>, HashMap<u32, u32>)> {
        let deleted_ids: Vec<u32> = self.timestamps.iter_deleted(ts).collect();

        let mut removed_keys = Vec::with_capacity(deleted_ids.len());

        for id in &deleted_ids {
            if let Some(key) = self.id_indexer.get_key(*id) {
                self.id_indexer.remove(&key);
                removed_keys.push(key);
            }
        }

        let mut coordinator = CompactionCoordinator::new();
        coordinator.execute(self)?;

        Ok((removed_keys, coordinator.id_mapping().clone()))
    }

    /// Compact the vertex table using the unified CompactionCoordinator
    ///
    /// This is the **only** public compaction method. All table optimization,
    /// ID remapping, and consistency verification happens through this single entry point.
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
    /// All steps execute in sequence. If any step fails, an error is returned
    /// immediately and the table is left in the state after the last successful step.
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
    pub fn compact_coordinated(&mut self) -> StorageResult<()> {
        let mut coordinator = super::compaction::CompactionCoordinator::new();
        coordinator.execute(self)
    }
}
