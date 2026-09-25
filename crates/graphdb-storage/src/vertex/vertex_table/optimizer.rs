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
        let deleted_ids: Vec<u32> = self.timestamps.read().iter_deleted(cutoff).collect();

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
        let deleted_ids: Vec<u32> = self.timestamps.read().iter_deleted(cutoff).collect();
        let mut removed_keys = Vec::with_capacity(deleted_ids.len());
        for id in &deleted_ids {
            if let Some(key) = self.id_indexer.get_key(*id) {
                self.id_indexer.remove(&key);
                self.timestamps.write().invalidate_slot(*id);
                removed_keys.push(key);
            }
        }
        Ok((removed_keys, HashMap::new(), CompactionJournal::default()))
    }
}
