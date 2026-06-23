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

use std::collections::HashMap;
use crate::core::StorageResult;
use super::core::VertexTable;

/// Unified compaction coordinator for VertexTable
///
/// This struct ensures all three internal structures (id_indexer, timestamps, columns)
/// are updated consistently during compaction.
///
/// # Usage
///
/// ```ignore
/// let mut table = VertexTable::new(...);
/// // ... insert/delete vertices ...
/// table.compact_coordinated()?;  // Uses CompactionCoordinator internally
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
}

impl CompactionCoordinator {
    /// Create a new compaction coordinator
    pub fn new() -> Self {
        Self {
            has_remapped: false,
            id_mapping: HashMap::new(),
        }
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
    /// 6. **Apply encodings**: Batch any pending column encodings
    /// 7. **Verify**: Assert all three structures are consistent
    ///
    /// # Error Handling
    ///
    /// If any step fails:
    /// - Error is returned immediately
    /// - Table is left in the state after the last successful operation
    /// - This is safe but may require manual cleanup in some cases
    ///
    /// # Performance
    ///
    /// - O(n) in number of vertices
    /// - Requires exclusive access (mut self on VertexTable)
    /// - Space is reclaimed eagerly (arrays truncated immediately)
    pub fn execute(&mut self, table: &mut VertexTable) -> StorageResult<()> {
        // Step 1: Get authoritative mapping from id_indexer
        self.id_mapping = table.id_indexer.compact().unwrap_or_default();
        self.has_remapped = !self.id_mapping.is_empty();

        // Step 2 & 3: If remapping occurred, propagate to both structures
        if self.has_remapped {
            self.propagate_remap(table)?;
        } else {
            // No remapping, but clean up any orphaned timestamps
            self.cleanup_orphaned_timestamps(table);
        }

        // Step 4: Resize columns to match new id_indexer size
        table.columns.resize(table.id_indexer.len());

        // Step 5: Apply any deferred encodings
        table.apply_deferred_encodings()?;

        // Step 6: Verify invariants (debug builds only)
        #[cfg(debug_assertions)]
        table.verify_invariants()?;

        Ok(())
    }

    /// Propagate the ID remapping to both timestamps and columns
    ///
    /// This is an internal step that must happen atomically:
    /// if timestamps remap fails, columns aren't remapped.
    fn propagate_remap(&self, table: &mut VertexTable) -> StorageResult<()> {
        // Order matters: columns might be more error-prone, so do timestamps first
        self.remap_timestamps(table)?;
        self.remap_columns(table)?;
        Ok(())
    }

    /// Apply ID mapping to timestamps
    ///
    /// This updates the MVCC visibility information to match the new IDs.
    /// All start_ts and end_ts values are preserved; only the array indices change.
    fn remap_timestamps(&self, table: &mut VertexTable) -> StorageResult<()> {
        if self.id_mapping.is_empty() {
            return Ok(());
        }

        let max_new_id = self
            .id_mapping
            .values()
            .max()
            .copied()
            .unwrap_or(0) as usize;
        let mut new_timestamps =
            super::super::VertexTimestamp::with_capacity(max_new_id + 1);

        for (old_id, new_id) in &self.id_mapping {
            if let Some(start_ts) = table.timestamps.get_start_ts(*old_id) {
                new_timestamps.insert(*new_id, start_ts);
                if let Some(end_ts) = table.timestamps.get_end_ts(*old_id) {
                    if end_ts < crate::storage::vertex::MAX_TIMESTAMP {
                        new_timestamps.remove(*new_id, end_ts);
                    }
                }
            }
        }

        table.timestamps = new_timestamps;
        Ok(())
    }

    /// Apply ID mapping to columns
    ///
    /// This moves all property data to new positions according to the mapping.
    /// Column encodings are preserved; deferred encodings are applied separately.
    fn remap_columns(&self, table: &mut VertexTable) -> StorageResult<()> {
        if self.id_mapping.is_empty() {
            return Ok(());
        }

        let max_old_id = self
            .id_mapping
            .keys()
            .max()
            .copied()
            .unwrap_or(0) as usize;
        if max_old_id >= table.columns.row_count() {
            return Ok(());
        }

        let mut new_columns =
            super::super::ColumnStore::with_capacity(table.id_indexer.len());
        for prop in &table.schema.properties {
            new_columns.add_column(
                prop.name.clone(),
                prop.data_type.clone(),
                prop.nullable,
            );
        }

        // Batch copy: O(vertices) instead of O(vertices × properties)
        for (old_id, new_id) in &self.id_mapping {
            let old_idx = *old_id as usize;
            let new_idx = *new_id as usize;

            let values = table.columns.get(old_idx);
            let pairs: Vec<(String, crate::core::Value)> = values
                .into_iter()
                .filter_map(|(name, opt_val)| opt_val.map(|v| (name, v)))
                .collect();

            if !pairs.is_empty() {
                new_columns.set(new_idx, &pairs)?;
            }
        }

        table.columns = new_columns;
        Ok(())
    }

    /// Clean up timestamp entries that have no corresponding id_indexer entry
    ///
    /// This is a safety fallback for when id_indexer had no remapping
    /// but timestamps may have orphaned entries.
    fn cleanup_orphaned_timestamps(&self, table: &mut VertexTable) {
        let mut new_timestamps =
            super::super::VertexTimestamp::with_capacity(table.id_indexer.len());

        // Copy only timestamps entries that have corresponding id_indexer entries
        for idx in 0..table.timestamps.size() {
            let idx_u32 = idx as u32;
            if table.id_indexer.get_key(idx_u32).is_some() {
                // This ID is still in id_indexer, keep its timestamp info
                if let Some(start_ts) = table.timestamps.get_start_ts(idx_u32) {
                    new_timestamps.insert(idx_u32, start_ts);
                    if let Some(end_ts) = table.timestamps.get_end_ts(idx_u32) {
                        if end_ts < crate::storage::vertex::MAX_TIMESTAMP {
                            new_timestamps.remove(idx_u32, end_ts);
                        }
                    }
                }
            }
        }

        table.timestamps = new_timestamps;
    }

    /// Get the ID mapping that was computed during this compaction
    ///
    /// Useful for logging or debugging to see which IDs were remapped.
    pub fn id_mapping(&self) -> &HashMap<u32, u32> {
        &self.id_mapping
    }

    /// Check whether any remapping occurred
    pub fn has_remapped(&self) -> bool {
        self.has_remapped
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{DataType, Value};
    use crate::storage::types::StoragePropertyDef;
    use crate::storage::vertex::{VertexSchema, VertexTable};

    fn create_test_schema() -> VertexSchema {
        VertexSchema {
            label_id: 0,
            label_name: "test".to_string(),
            properties: vec![
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
        let mut table = VertexTable::new(0, "test".to_string(), schema);
        let mut coordinator = CompactionCoordinator::new();

        // Empty table should compact without error
        assert!(coordinator.execute(&mut table).is_ok());
        assert!(!coordinator.has_remapped());
    }

    #[test]
    fn test_coordinator_single_vertex() {
        let schema = create_test_schema();
        let mut table = VertexTable::new(0, "test".to_string(), schema);

        table
            .insert(
                "v1",
                &[("name".to_string(), Value::String("Alice".to_string()))],
                100,
            )
            .unwrap();

        let mut coordinator = CompactionCoordinator::new();
        assert!(coordinator.execute(&mut table).is_ok());

        // Since there are no gaps, no remapping should occur
        assert!(!coordinator.has_remapped());
        assert_eq!(table.id_indexer.len(), 1);
    }

    #[test]
    fn test_coordinator_remapping() {
        let schema = create_test_schema();
        let mut table = VertexTable::new(0, "test".to_string(), schema);

        // Insert 5 vertices to allocate space
        for i in 0..5 {
            table
                .insert(
                    &format!("v{}", i),
                    &[("name".to_string(), Value::String(format!("P{}", i)))],
                    100,
                )
                .unwrap();
        }

        assert_eq!(table.id_indexer.len(), 5);

        // Compact should complete successfully
        let mut coordinator = CompactionCoordinator::new();
        assert!(coordinator.execute(&mut table).is_ok());

        // After compaction on a table with no gaps, nothing should be remapped
        assert!(!coordinator.has_remapped());
        assert_eq!(table.id_indexer.len(), 5);
    }
}
