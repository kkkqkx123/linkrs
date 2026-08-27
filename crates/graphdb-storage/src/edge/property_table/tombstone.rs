//! Tombstones: deletion marks, tombstone GC, and slot reclamation.

use super::*;

impl PropertyTable {
    /// Mark a property record as deleted for MVCC tracking
    pub fn mark_deleted(&mut self, offset: u32, delete_ts: Timestamp) -> StorageResult<()> {
        let row_idx =
            prop_offset_to_index(offset).ok_or_else(|| StorageError::invalid_offset(offset))?;
        if row_idx >= self.records.len() {
            return Ok(()); // Already deleted or doesn't exist
        }

        // Check deletion state and get old props BEFORE mutable borrow
        let can_delete = self.records[row_idx]
            .as_ref()
            .is_some_and(|r| r.delete_ts.is_none());

        if can_delete {
            // Remove from index before marking as deleted
            if let Some(props) = self.get(offset, None) {
                self.value_index.remove_record(&props, offset);
            }

            if let Some(record) = &mut self.records[row_idx] {
                record.delete_ts = Some(delete_ts);
                self.tombstones_manager.add_tombstone(offset, delete_ts);
            }
            // Columnar: zone map needs refresh; column values stay for time-travel
            // but are excluded from live zone stats.
            self.refresh_zone_map_for_row(row_idx);
            Ok(())
        } else if self.records[row_idx].is_some() {
            Err(StorageError::invalid_operation(
                "record already marked deleted",
            ))
        } else {
            Ok(()) // Idempotent: already deleted
        }
    }

    /// Revert a [`PropertyTable::mark_deleted`]: clear the deletion mark and
    /// drop the tombstone entry so the record is visible again. Used by the
    /// edge delete undo path to restore properties alongside the adjacency.
    ///
    /// Returns true if a deletion mark was actually cleared.
    pub fn revert_deletion(&mut self, offset: u32) -> bool {
        let row_idx = match prop_offset_to_index(offset) {
            Some(idx) => idx,
            None => return false,
        };
        let Some(record) = self.records[row_idx].as_mut() else {
            return false;
        };
        if record.delete_ts.is_none() {
            return false;
        }
        record.delete_ts = None;
        self.tombstones_manager.remove(offset);
        if let Some(props) = self.get(offset, None) {
            self.value_index.index_record(&props, offset);
        }
        self.refresh_zone_map_for_row(row_idx);
        true
    }

    /// Check whether the record at `offset` is currently marked deleted.
    pub fn is_deleted(&self, offset: u32) -> bool {
        let Some(row_idx) = prop_offset_to_index(offset) else {
            return false;
        };
        self.records
            .get(row_idx)
            .and_then(|r| r.as_ref())
            .is_some_and(|r| r.delete_ts.is_some())
    }

    /// Garbage collect tombstones older than min_active_snapshot_ts
    pub fn gc_tombstones(&mut self, min_active_snapshot_ts: Timestamp) -> u32 {
        // Incremental batch GC first, then a full GC pass to clean remaining.
        let batch_size = 10_000usize;
        self.tombstones_manager
            .gc_batch(min_active_snapshot_ts, batch_size);
        self.tombstones_manager.gc(min_active_snapshot_ts);

        // Remove records that are fully tombstoned and older than min_active_snapshot_ts
        let mut reclaimed = 0u32;
        let mut indices_to_clear = Vec::new();

        for (idx, record_opt) in self.records.iter().enumerate() {
            if let Some(record) = record_opt {
                if let Some(delete_ts) = record.delete_ts {
                    if delete_ts < min_active_snapshot_ts {
                        let offset = prop_index_to_offset(idx);
                        indices_to_clear.push((idx, offset));
                        reclaimed += 1;
                    }
                }
            }
        }

        for (idx, offset) in &indices_to_clear {
            // Remove from index if still indexed
            if let Some(ref record) = self.records[*idx] {
                let props = deserialize_row_raw(&self.schema, &record.data);
                self.value_index.remove_record(&props, *offset);
            }
        }

        let has_cleared = !indices_to_clear.is_empty();
        for (idx, offset) in indices_to_clear {
            if let Some(record) = &self.records[idx] {
                self.used_data_bytes = self.used_data_bytes.saturating_sub(record.data.len());
            }
            // The row's full version history (before-images included) is no
            // longer visible to any active snapshot.
            if let Some(chain) = self.chain_records.get_mut(idx) {
                for entry in chain.drain(..) {
                    self.used_data_bytes = self.used_data_bytes.saturating_sub(entry.data.len());
                }
            }
            self.records[idx] = None;
            self.free_list.push(offset);
            // Columnar: keep column values for time-travel but exclude from zone maps.
            // Zone maps are refreshed below.
        }

        if has_cleared {
            self.rebuild_zone_maps();
        }

        reclaimed
    }

    /// Legacy delete method for backward compatibility (physical delete)
    pub fn delete(&mut self, offset: u32) -> bool {
        let row_idx = match prop_offset_to_index(offset) {
            Some(idx) => idx,
            None => return false,
        };
        if row_idx >= self.records.len() {
            return false;
        }

        // Remove from index before deleting
        if let Some(props) = self.get(offset, None) {
            self.value_index.remove_record(&props, offset);
        }

        if let Some(record) = &self.records[row_idx] {
            self.used_data_bytes = self.used_data_bytes.saturating_sub(record.data.len());
        }
        // The row is removed wholesale; its version history dies with it.
        if let Some(chain) = self.chain_records.get_mut(row_idx) {
            for entry in chain.drain(..) {
                self.used_data_bytes = self.used_data_bytes.saturating_sub(entry.data.len());
            }
        }
        self.records[row_idx] = None;
        self.free_list.push(offset);
        // Columnar: keep column slot but mark zone map dirty.
        self.refresh_zone_map_for_row(row_idx);
        true
    }

    /// Reclaim property slots whose rows are physically dead: tombstoned,
    /// no longer referenced by any live edge (`offset ∉ valid_offsets`), and
    /// deleted at or before the retention bound. Cleared slots return to the
    /// free list for reuse by future inserts.
    ///
    /// Live rows never move: offsets are stable, so external references
    /// (CSR `prop_offset` pointers) stay valid without any relocation
    /// mapping. An unbounded retention bound ([`Timestamp::MAX`]) is not a
    /// real timestamp — nothing is reclaimable, preserving time-travel
    /// history.
    ///
    /// Returns the number of reclaimed rows.
    pub fn reclaim_slots(
        &mut self,
        valid_offsets: &HashSet<u32>,
        retention_bound: Timestamp,
    ) -> usize {
        if retention_bound == Timestamp::MAX {
            return 0;
        }
        let mut reclaimed = 0usize;
        for idx in 0..self.records.len() {
            let offset = prop_index_to_offset(idx);
            if valid_offsets.contains(&offset) {
                continue;
            }
            let Some(record) = self.records[idx].as_ref() else {
                continue;
            };
            // Live rows (no deletion mark) are never reclaimed here: they may
            // still be referenced by edges outside the collected set of valid
            // offsets.
            let Some(delete_ts) = record.delete_ts else {
                continue;
            };
            if delete_ts > retention_bound {
                continue;
            }

            let props = deserialize_row_raw(&self.schema, &record.data);
            self.value_index.remove_record(&props, offset);
            self.tombstones_manager.remove(offset);
            self.used_data_bytes = self.used_data_bytes.saturating_sub(record.data.len());
            // The row's full version history dies with its slot; this is safe
            // because `delete_ts <= retention_bound` means no active snapshot
            // can observe any version of the row.
            if let Some(chain) = self.chain_records.get_mut(idx) {
                for entry in chain.drain(..) {
                    self.used_data_bytes = self.used_data_bytes.saturating_sub(entry.data.len());
                }
            }
            self.records[idx] = None;
            self.free_list.push(offset);
            self.row_count = self.row_count.saturating_sub(1);
            reclaimed += 1;
        }

        if reclaimed > 0 {
            // Zone maps must exclude the cleared rows; the columnar store
            // keeps cell values until the slot is reused (same policy as
            // physical delete).
            self.rebuild_zone_maps();
        }
        reclaimed
    }
}
