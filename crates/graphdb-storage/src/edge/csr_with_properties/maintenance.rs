use super::{CsrWithProperties, RowVisibility};
use crate::edge::property_schema::PropertySchema;
use graphdb_core::types::{EdgeId, Timestamp};
use graphdb_core::DataType;
use std::collections::HashSet;

impl CsrWithProperties {
    pub fn compaction_stats(&self) -> crate::edge::property_schema::PropertyCompactionStats {
        let tombstone_count = self
            .visibility
            .iter()
            .filter(|v| v.delete_ts.is_some())
            .count();
        let live_records = self
            .visibility
            .iter()
            .filter(|v| v.create_ts != 0 && v.delete_ts.is_none())
            .count();
        let mut reclaimable_bytes = 0usize;
        for v in &self.visibility {
            if v.delete_ts.is_some() {
                reclaimable_bytes += 32 * self.property_schema.len();
            }
        }
        crate::edge::property_schema::PropertyCompactionStats {
            tombstone_count,
            total_records: self.visibility.len(),
            live_records,
            reclaimable_bytes,
        }
    }

    /// Garbage-collect property version-chain entries that no active snapshot
    /// can observe. Returns the total number of before-images removed.
    pub fn gc_property_versions(&mut self, min_active_snapshot_ts: Timestamp) -> usize {
        self.property_columns
            .iter_mut()
            .map(|col| col.gc_versions(min_active_snapshot_ts))
            .sum()
    }

    /// Aggregate version-chain statistics across all property columns.
    pub fn property_version_stats(&self) -> crate::vertex::column::mvcc::VersionChainStats {
        let mut total_rows = 0usize;
        let mut total_entries = 0usize;
        let mut max_len = 0usize;
        let mut memory_bytes = 0usize;
        for col in &self.property_columns {
            let stats = col.version_chain_stats();
            total_rows = total_rows.max(stats.total_rows);
            total_entries += stats.total_entries;
            max_len = max_len.max(stats.max_len);
            memory_bytes += stats.memory_bytes;
        }
        let avg_len = if total_rows > 0 {
            total_entries as f64 / total_rows as f64
        } else {
            0.0
        };
        crate::vertex::column::mvcc::VersionChainStats {
            total_rows,
            total_entries,
            max_len,
            avg_len,
            memory_bytes,
        }
    }

    pub fn is_schema_fixed_size(&self) -> bool {
        self.property_schema.iter().all(|s| {
            matches!(
                s.data_type,
                DataType::Bool
                    | DataType::SmallInt
                    | DataType::Int
                    | DataType::BigInt
                    | DataType::Float
                    | DataType::Double
            )
        })
    }

    pub fn used_memory_size(&self) -> usize {
        let mut total = std::mem::size_of::<Self>();
        total += self.visibility.capacity() * std::mem::size_of::<RowVisibility>();
        total += self.edge_to_row.capacity() * std::mem::size_of::<u32>();
        total += self.free_list.capacity() * std::mem::size_of::<u32>();
        total += self.row_to_edge.capacity() * std::mem::size_of::<Option<EdgeId>>();
        total += self.column_index.len()
            * (std::mem::size_of::<String>() + std::mem::size_of::<usize>());
        for col in &self.property_columns {
            total += col.memory_size();
        }
        total += self.property_schema.len() * std::mem::size_of::<PropertySchema>();
        total
    }

    pub fn reclaim_slots(
        &mut self,
        valid_edge_ids: &HashSet<EdgeId>,
        retention_bound: Timestamp,
    ) -> usize {
        if retention_bound == Timestamp::MAX {
            return 0;
        }
        let mut to_reclaim = Vec::new();
        for (idx, vis) in self.visibility.iter().enumerate() {
            if vis.create_ts == 0 {
                continue;
            }
            // O(1) ownership check via the reverse index instead of scanning
            // the full edge map for every row.
            let has_live_edge = self
                .row_to_edge
                .get(idx)
                .and_then(|slot| *slot)
                .is_some_and(|eid| valid_edge_ids.contains(&eid));
            if has_live_edge {
                continue;
            }
            if let Some(del_ts) = vis.delete_ts {
                // Exclusive waterfront: deletable exactly when invisible to
                // every snapshot at or past the cutoff.
                if crate::mvcc_visibility::Visibility::is_gc_eligible(del_ts, retention_bound) {
                    to_reclaim.push(idx);
                }
            }
        }
        if to_reclaim.is_empty() {
            return 0;
        }
        for &idx in &to_reclaim {
            self.visibility[idx].create_ts = 0;
            self.visibility[idx].delete_ts = None;
            if let Some(taken) = self.row_to_edge.get_mut(idx).and_then(|slot| slot.take()) {
                self.map_remove(taken);
            }
            // Reclaimed rows are virgin after the stamp clear, so the free
            // list admits each of them exactly once with no membership set.
            self.free_list.push(idx as u32);
            self.row_count = self.row_count.saturating_sub(1);
        }
        to_reclaim.len()
    }

    /// Recompute persisted per-column statistics from flush buffers.
    ///
    /// Called before the property file is serialized so statistics follow the
    /// checkpoint instead of drifting. Only columns marked dirty are
    /// recomputed; clean columns keep their persisted statistics. Columns
    /// that fail to compute keep their previous statistics.
    pub fn refresh_column_stats(&mut self) {
        if self.dirty_columns.is_empty() {
            for col in &mut self.property_columns {
                if let Ok(stats) = col.compute_stats() {
                    col.set_stats(stats);
                }
            }
            return;
        }
        // Dirt marks stay until the checkpoint clears them: stats refresh
        // must not consume the marks that drive dirty-column persistence.
        let dirty: Vec<usize> = self.dirty_columns.iter().copied().collect();
        for idx in dirty {
            if let Some(col) = self.property_columns.get_mut(idx) {
                if let Ok(stats) = col.compute_stats() {
                    col.set_stats(stats);
                }
            }
        }
    }

    /// Refresh statistics for one column only. Used by column-level
    /// checkpoint follow-up when only a subset changed.
    pub fn refresh_column_stats_for(&mut self, column: &str) {
        if let Some(&idx) = self.column_index.get(column) {
            if let Some(col) = self.property_columns.get_mut(idx) {
                if let Ok(stats) = col.compute_stats() {
                    col.set_stats(stats);
                }
            }
        }
    }
}
