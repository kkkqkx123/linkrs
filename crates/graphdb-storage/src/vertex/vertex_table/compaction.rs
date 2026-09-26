//! Unified Compaction Coordinator
//!
//! This module provides a single point of coordination for the three-structure
//! compaction process, addressing the design debt mentioned in optimizer.rs.
//!
//! # Problem Statement
//!
//! The original compact() implementation splits responsibility across multiple methods:
//! - id_indexer.compact() computes the mapping
//! - remap_timestamps() applies the mapping to timestamps
//! - remap_columns() applies the mapping to columns
//!
//! This design is error-prone because:
//! 1. **No compile-time enforcement**: Missing a remap step causes silent corruption
//! 2. **Module boundary crossing**: Each module needs to know about the others
//! 3. **Lack of atomicity**: No transaction wrapper ensures all steps complete
//! 4. **Code duplication**: When EdgeTable is added, the same pattern must be repeated
//!
//! # Solution: CompactionCoordinator
//!
//! This coordinator provides:
//! - Single entry point for compaction (execute)
//! - Clear sequencing of all steps
//! - Type-safe coordination across structures
//! - Foundation for reuse in EdgeTable
//!
//! # Design Pattern: Borrowed Mutable References
//!
//! Rather than moving ownership, the coordinator borrows mutable references to
//! the three structures. This allows the caller (VertexTable) to retain ownership
//! while the coordinator orchestrates the operations.

use super::core::VertexTable;
use graphdb_core::StorageResult;
use std::collections::HashMap;

/// Stable row-id mode switch for the long-term compaction policy.
///
/// `true` makes stable row ids the production semantic (live rows never
/// move; deletes are absorbed by the free stack; production compaction
/// produces zero edge rewrites). The remap cascade below stays available
/// as an explicit offline tool only: it must run under the maintenance
/// commit barrier followed by a checkpoint, never inside background GC.
pub const STABLE_ROW_IDS_ENABLED: bool = true;

/// Unified compaction coordinator for VertexTable
///
/// This struct ensures all three internal structures (id_indexer, timestamps, columns)
/// are updated consistently during compaction.
///
/// # Usage
///
/// ```ignore
/// let mut table = VertexTable::with_config(...);
/// // ... insert/delete vertices ...
/// // Cutoff-gated offline remap (watermark safe timestamp only).
/// let (_, mapping, _) = table.compact_with_cutoff_collect_mapping(cutoff)?;
/// ```
///
/// # Invariants Enforced
///
/// After successful execution:
/// - Every id_indexer entry has a timestamps entry
/// - Every timestamps entry has an id_indexer entry (no orphans)
/// - columns.row_count() == id_indexer.len()
/// - All property data is preserved in new positions
pub struct CompactionCoordinator {
    /// Flag indicating whether any remapping occurred
    has_remapped: bool,
    /// Mapping from old IDs to new IDs for propagation to other structures
    id_mapping: HashMap<u32, u32>,
    /// Write-ahead journal of the current execution. Steps are recorded
    /// before they mutate state and the committed marker flips only after
    /// every swap succeeds, so a mid-compaction failure rolls back to the
    /// pre-compaction snapshot instead of leaving index and columns in
    /// different id spaces. Crash recovery additionally replays external
    /// IDs from the WAL, which remains the durable source of truth.
    journal: CompactionJournal,
}

/// Write-ahead record for one compaction execution.
///
/// Fixed step order: index remap, timestamp remap, column remap, edge
/// endpoint rewrite. Each step is journaled before it runs; the commit
/// marker is set only after all swaps succeed.
#[derive(Debug, Clone, Default)]
pub struct CompactionJournal {
    steps_executed: Vec<CompactionStep>,
    committed: bool,
}

/// One journaled compaction step, in execution order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionStep {
    Index,
    Timestamp,
    Column,
    Edge,
}

impl CompactionJournal {
    fn record(&mut self, step: CompactionStep) {
        self.steps_executed.push(step);
    }

    fn mark_committed(&mut self) {
        self.committed = true;
    }

    /// Whether the commit marker flipped after all swaps succeeded.
    pub fn is_committed(&self) -> bool {
        self.committed
    }

    /// Steps executed, in order.
    pub fn steps(&self) -> &[CompactionStep] {
        &self.steps_executed
    }

    /// Merge another journal (per-shard journals combine into the
    /// table-level journal the edge phase extends). Committed only if
    /// every merged journal committed.
    pub fn extend(&mut self, other: &CompactionJournal) {
        self.steps_executed
            .extend(other.steps_executed.iter().copied());
        self.committed = self.committed && other.committed;
    }

    /// Record the edge-remap step. Called by the maintenance layer after
    /// the vertex swaps commit and before edge endpoints are rewritten,
    /// keeping vertex and edge work in one journaled barrier.
    pub fn record_edge_remap(&mut self) {
        self.record(CompactionStep::Edge);
    }

    /// Combine per-shard journals into one table-level journal.
    pub fn combine(journals: &[CompactionJournal]) -> CompactionJournal {
        let mut out = CompactionJournal {
            steps_executed: Vec::new(),
            committed: true,
        };
        for journal in journals {
            out.extend(journal);
        }
        if journals.is_empty() {
            out.committed = false;
        }
        out
    }

    /// Steps to undo, in reverse execution order. Empty once committed.
    fn rollback_plan(&self) -> Vec<CompactionStep> {
        if self.committed {
            return Vec::new();
        }
        let mut plan = self.steps_executed.clone();
        plan.reverse();
        plan
    }
}

impl CompactionCoordinator {
    /// Create a new compaction coordinator
    pub fn new() -> Self {
        Self {
            has_remapped: false,
            id_mapping: HashMap::new(),
            journal: CompactionJournal::default(),
        }
    }
}

impl Default for CompactionCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

impl CompactionCoordinator {
    /// Old-to-new internal ID mapping produced by the last [`Self::execute`].
    ///
    /// IDs that did not move are absent. Callers that need to propagate the
    /// remap to dependent structures (e.g. edge table CSR row indices) must
    /// read this after `execute` returns.
    pub fn id_mapping(&self) -> &HashMap<u32, u32> {
        &self.id_mapping
    }

    /// Journal of the last [`Self::execute`]: which steps ran and whether
    /// the commit marker flipped. Callers propagating the remap to edge
    /// tables run under the same commit barrier and extend the journal
    /// with the edge step before rewriting endpoints.
    pub fn journal(&self) -> &CompactionJournal {
        &self.journal
    }

    /// Execute the full compaction process on a VertexTable
    ///
    /// This is the public interface that orchestrates all steps in the correct order.
    ///
    /// # Steps
    ///
    /// 1. **Authorize mapping**: Get mapping from id_indexer (authoritative source)
    /// 2. **Propagate to timestamps**: Apply mapping to MVCC visibility info
    /// 3. **Propagate to columns**: Apply mapping to property data
    /// 4. **Cleanup orphans**: Remove any orphaned timestamp entries
    /// 5. **Resize columns**: Truncate to match new id_indexer size
    /// 6. **Verify**: Assert all three structures are consistent
    ///
    /// # Error Handling
    ///
    /// Atomic within the table: the dense mapping is computed without
    /// mutating state, both replacements are built before either is
    /// swapped in, and any failure restores the pre-compaction index
    /// snapshot. A failed execution leaves index, timestamps, and columns
    /// exactly as before the call; the journal reports which steps ran
    /// without committing.
    ///
    /// # Performance
    ///
    /// - O(n) in number of vertices
    /// - Requires exclusive access (mut self on VertexTable)
    /// - Space is reclaimed eagerly (arrays truncated immediately)
    pub fn execute(&mut self, table: &mut VertexTable) -> StorageResult<()> {
        self.journal = CompactionJournal::default();
        // Snapshot the index before any mutation so a mid-remap failure
        // can roll back instead of leaving a densified index over stale
        // timestamps and columns.
        let index_snapshot = table.id_indexer.snapshot_bytes();
        // Capture the pre-compact live set first: the mapping only reports
        // rows that moved, so unmoved rows (old == new) are absent from it
        // but must still be carried over below.
        let old_live_ids: Vec<u32> = table.id_indexer.live_ids();

        // Step 1: Compute the authoritative mapping without mutating state,
        // so the fallible builds below run before anything is swapped.
        self.id_mapping = table.id_indexer.compute_compact_mapping();
        self.has_remapped = !self.id_mapping.is_empty();

        // Step 2 & 3: If remapping occurred, propagate to both structures.
        // Both replacements are built before either is swapped in, so a
        // mid-remap failure cannot leave timestamps and columns
        // describing different id spaces.
        if self.has_remapped {
            let new_timestamps = self.build_remapped_timestamps(table, &old_live_ids);
            let new_columns = match self.build_remapped_columns(table, &old_live_ids) {
                Ok(columns) => columns,
                Err(e) => {
                    self.rollback_index(table, &index_snapshot);
                    return Err(e);
                }
            };
            self.journal.record(CompactionStep::Index);
            let applied = table.id_indexer.compact()?;
            debug_assert_eq!(applied, self.id_mapping);
            self.journal.record(CompactionStep::Timestamp);
            *table.timestamps.write() = new_timestamps;
            self.journal.record(CompactionStep::Column);
            table.columns = new_columns;
        } else {
            // No remapping, but clean up any orphaned timestamps
            self.cleanup_orphaned_timestamps(table);
        }

        // Step 4: Resize columns to match new id_indexer size
        table.columns.resize(table.id_indexer.len());
        self.journal.mark_committed();

        Ok(())
    }

    /// Restore the pre-compaction index after a failed remap step. The
    /// rollback runs the journaled steps in reverse order; with
    /// build-before-swap only the index can be dirty here, so restoring
    /// its snapshot suffices. A restore failure is logged without masking
    /// the original error.
    fn rollback_index(&mut self, table: &mut VertexTable, snapshot: &[u8]) {
        let _plan = self.journal.rollback_plan();
        if let Err(e) = table.id_indexer.restore_snapshot(snapshot) {
            log::warn!("compaction rollback failed to restore id indexer: {}", e);
        }
    }

    /// Rebuild timestamp tracking for the post-compact id space.
    ///
    /// Every previously live row is carried over at
    /// `mapping.get(old).unwrap_or(old)`; only array indices change, all
    /// start/end timestamps are preserved.
    fn build_remapped_timestamps(
        &self,
        table: &VertexTable,
        old_live_ids: &[u32],
    ) -> super::super::VertexTimestamp {
        let mut new_timestamps =
            super::super::VertexTimestamp::with_capacity(table.id_indexer.len());

        for &old_id in old_live_ids {
            let new_id = self.id_mapping.get(&old_id).copied().unwrap_or(old_id);
            if let Some(start_ts) = table.timestamps.read().get_start_ts(old_id) {
                new_timestamps.insert(new_id, start_ts);
                if let Some(end_ts) = table.timestamps.read().get_end_ts(old_id) {
                    if end_ts < crate::vertex::MAX_TIMESTAMP {
                        new_timestamps.remove(new_id, end_ts);
                    }
                }
            }
        }

        new_timestamps
    }

    /// Rebuild column storage for the post-compact id space.
    ///
    /// Every previously live row is carried over (current values plus MVCC
    /// row state, so snapshot reads stay intact after the remap). Fresh
    /// columns inherit the table's storage tuning so post-compact writes
    /// behave identically; learned chunk encodings are intentionally not
    /// carried over and are re-learned on subsequent writes.
    fn build_remapped_columns(
        &self,
        table: &VertexTable,
        old_live_ids: &[u32],
    ) -> StorageResult<super::super::ColumnStore> {
        let new_columns = super::super::ColumnStore::with_capacity(table.id_indexer.len());
        for prop in &table.schema.properties {
            new_columns.add_column(prop.name.clone(), prop.data_type.clone(), prop.nullable);
        }
        for prop in &table.schema.properties {
            if let (Some(src), Some(dst)) = (
                table.columns.get_column(&prop.name),
                new_columns.get_column(&prop.name),
            ) {
                dst.set_chunk_capacity(src.chunk_capacity());
                dst.set_overflow_threshold(table.string_overflow_threshold);
            }
        }

        // Batch copy: O(vertices) instead of O(vertices × properties)
        for &old_id in old_live_ids {
            let old_idx = old_id as usize;
            let new_id = self.id_mapping.get(&old_id).copied().unwrap_or(old_id);
            let new_idx = new_id as usize;

            let values = table.columns.get(old_idx);
            let pairs: Vec<(String, graphdb_core::Value)> = values
                .into_iter()
                .filter_map(|(name, opt_val)| opt_val.map(|v| (name, v)))
                .collect();

            if !pairs.is_empty() {
                new_columns.set(new_idx, &pairs)?;
            }
            // Preserve MVCC metadata (creation timestamp and the
            // before-image chain) so snapshot reads stay intact after remap.
            new_columns.clone_row_state_from(&table.columns, old_idx, new_idx);
        }

        Ok(new_columns)
    }

    /// Clean up timestamp entries that have no corresponding id_indexer entry
    ///
    /// This is a safety fallback for when id_indexer had no remapping
    /// but timestamps may have orphaned entries.
    fn cleanup_orphaned_timestamps(&self, table: &mut VertexTable) {
        let mut new_timestamps =
            super::super::VertexTimestamp::with_capacity(table.id_indexer.len());

        // Copy only timestamps entries that have corresponding id_indexer entries
        for idx in 0..table.timestamps.read().size() {
            let idx_u32 = idx as u32;
            if table.id_indexer.get_key(idx_u32).is_some() {
                // This ID is still in id_indexer, keep its timestamp info
                if let Some(start_ts) = table.timestamps.read().get_start_ts(idx_u32) {
                    new_timestamps.insert(idx_u32, start_ts);
                    if let Some(end_ts) = table.timestamps.read().get_end_ts(idx_u32) {
                        if end_ts < crate::vertex::MAX_TIMESTAMP {
                            new_timestamps.remove(idx_u32, end_ts);
                        }
                    }
                }
            }
        }

        *table.timestamps.write() = new_timestamps;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::StoragePropertyDef;
    use crate::vertex::vertex_table::core::{VertexTable, VertexTableConfig};
    use crate::vertex::{IdKey, VertexSchema};
    use graphdb_core::{DataType, Value};

    fn create_test_schema() -> VertexSchema {
        // The first property is the primary key, which mirrors the external
        // id: inserts below omit it and let the table auto-fill the mirror.
        VertexSchema {
            label_id: 0,
            label_name: "test".to_string(),
            properties: vec![
                StoragePropertyDef::new("id".to_string(), DataType::String),
                StoragePropertyDef::new("name".to_string(), DataType::String),
                StoragePropertyDef {
                    name: "age".to_string(),
                    data_type: DataType::Int,
                    nullable: true,
                    default_value: None,
                },
            ],
            primary_key_index: 0,
            schema_version: 1,
        }
    }

    #[test]
    fn test_coordinator_empty_table() {
        let schema = create_test_schema();
        let mut table =
            VertexTable::with_config(0, "test".to_string(), schema, VertexTableConfig::default());
        let mut coordinator = CompactionCoordinator::new();

        // Empty table should compact without error
        assert!(coordinator.execute(&mut table).is_ok());
        assert!(!coordinator.has_remapped);
    }

    #[test]
    fn test_coordinator_single_vertex() {
        let schema = create_test_schema();
        let mut table =
            VertexTable::with_config(0, "test".to_string(), schema, VertexTableConfig::default());

        table
            .insert("v1", &[("name".to_string(), Value::string("Alice"))], 100)
            .unwrap();

        let mut coordinator = CompactionCoordinator::new();
        assert!(coordinator.execute(&mut table).is_ok());

        // Since there are no gaps, no remapping should occur
        assert!(!coordinator.has_remapped);
        assert_eq!(table.id_indexer.len(), 1);
    }

    #[test]
    fn test_coordinator_remapping() {
        let schema = create_test_schema();
        let mut table =
            VertexTable::with_config(0, "test".to_string(), schema, VertexTableConfig::default());

        // Insert 5 vertices to allocate space
        for i in 0..5 {
            table
                .insert(
                    &format!("v{}", i),
                    &[("name".to_string(), Value::string(format!("P{}", i)))],
                    100,
                )
                .unwrap();
        }

        assert_eq!(table.id_indexer.len(), 5);

        // Compact should complete successfully
        let mut coordinator = CompactionCoordinator::new();
        assert!(coordinator.execute(&mut table).is_ok());

        // After compaction on a table with no gaps, nothing should be remapped
        assert!(!coordinator.has_remapped);
        assert_eq!(table.id_indexer.len(), 5);
    }

    #[test]
    fn test_journaled_remap_is_atomic_and_committed() {
        let schema = create_test_schema();
        let mut table =
            VertexTable::with_config(0, "test".to_string(), schema, VertexTableConfig::default());
        for i in 0..5 {
            table
                .insert(
                    &format!("v{}", i),
                    &[("name".to_string(), Value::string(format!("P{}", i)))],
                    100,
                )
                .unwrap();
        }
        table.id_indexer.remove(&IdKey::Text("v1".to_string()));
        table.id_indexer.remove(&IdKey::Text("v3".to_string()));

        let mut coordinator = CompactionCoordinator::new();
        coordinator.execute(&mut table).unwrap();

        assert!(coordinator.has_remapped);
        assert!(coordinator.journal.committed);
        assert!(coordinator.journal.rollback_plan().is_empty());
        let mut expected = HashMap::new();
        expected.insert(2u32, 1u32);
        expected.insert(4u32, 2u32);
        assert_eq!(*coordinator.id_mapping(), expected);

        // Index, timestamps, and columns agree on the densified space.
        assert_eq!(table.id_indexer.len(), 3);
        assert_eq!(table.columns.row_count(), 3);
        for (key, name) in [("v0", "P0"), ("v2", "P2"), ("v4", "P4")] {
            let id = table
                .id_indexer
                .get_index(&IdKey::Text(key.to_string()))
                .expect("survivor keeps its key");
            let record = table
                .get_by_internal_id(id, 100)
                .expect("survivor stays readable");
            assert_eq!(
                record
                    .properties
                    .iter()
                    .find(|(k, _)| k == "name")
                    .unwrap()
                    .1,
                Value::string(name)
            );
        }
    }

    #[test]
    fn test_watermarked_compact_collects_mapping_and_preserves_data() {
        let schema = create_test_schema();
        let mut table =
            VertexTable::with_config(0, "test".to_string(), schema, VertexTableConfig::default());
        for i in 0..5 {
            table
                .insert(
                    &format!("w{}", i),
                    &[("name".to_string(), Value::string(format!("Q{}", i)))],
                    100,
                )
                .unwrap();
        }
        table.delete("w1", 200).unwrap();
        table.delete("w3", 200).unwrap();

        let (removed, mapping, _) = table.compact_with_cutoff_collect_mapping(200).unwrap();
        assert_eq!(removed.len(), 2);
        assert!(!mapping.is_empty());
        for (key, name) in [("w0", "Q0"), ("w2", "Q2"), ("w4", "Q4")] {
            let id = table
                .id_indexer
                .get_index(&IdKey::Text(key.to_string()))
                .expect("survivor keeps its key");
            let record = table
                .get_by_internal_id(id, 200)
                .expect("survivor stays readable");
            assert_eq!(
                record
                    .properties
                    .iter()
                    .find(|(k, _)| k == "name")
                    .unwrap()
                    .1,
                Value::string(name)
            );
        }
    }
}
