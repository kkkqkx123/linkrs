//! MVCC: version-chain management and conflict detection (column-only).

use super::*;

impl PropertyTable {
    pub(super) fn write_versioned_row(
        &mut self,
        offset: u32,
        values: &[(String, Option<Value>)],
        ts: Timestamp,
    ) -> StorageResult<()> {
        let row_idx =
            prop_offset_to_index(offset).ok_or_else(|| StorageError::invalid_offset(offset))?;
        if row_idx >= self.row_create_ts.len() || self.row_create_ts[row_idx] == 0 {
            return Err(StorageError::invalid_offset(offset));
        }

        self.check_write_conflict(row_idx, offset, ts)?;
        // Per-column back-in-time check for each column being written
        for (name, _) in values {
            if self.has_property(name) {
                if let Some(col) = self.column_store.get_column(name) {
                    if let Some(&start) = col.visibility().create_ts().get(row_idx) {
                        if start != 0 && start > ts {
                            return Err(StorageError::write_write_conflict(format!(
                                "property row at offset {} already has a newer version of '{}' at ts={}, attempted write at ts={}",
                                offset, name, start, ts
                            )));
                        }
                    }
                }
            }
        }

        if let Some(old_props) = self.get(offset, None) {
            self.value_index.remove_record(&old_props, offset);
        }

        for (name, value) in values {
            if self.has_property(name) {
                let _ = self
                    .column_store
                    .set_property_versioned(row_idx, name, value.as_ref(), ts);
            }
        }
        if self.version_chain_cap != 0 && !values.is_empty() {
            let names: Vec<String> = values.iter().map(|(n, _)| n.clone()).collect();
            self.fold_oldest_versions_filtered(row_idx, &names);
        }
        self.refresh_zone_map_for_row(row_idx);

        self.value_index.index_record(values, offset);

        Ok(())
    }

    pub(super) fn check_write_conflict(
        &self,
        row_idx: usize,
        offset: u32,
        ts: Timestamp,
    ) -> StorageResult<()> {
        if let Some(Some(del_ts)) = self.row_delete_ts.get(row_idx) {
            if ts > *del_ts {
                return Err(StorageError::write_write_conflict(format!(
                    "property row at offset {} deleted at ts={}, attempted write at ts={}",
                    offset, del_ts, ts
                )));
            }
        } else if let Some(&create_ts) = self.row_create_ts.get(row_idx) {
            if create_ts > ts && create_ts != 0 {
                return Err(StorageError::write_write_conflict(format!(
                    "property row at offset {} already has a newer version at ts={}, attempted write at ts={}",
                    offset, create_ts, ts
                )));
            }
        }
        Ok(())
    }

    pub fn gc_versions(&mut self, min_active_snapshot_ts: Timestamp) -> usize {
        if min_active_snapshot_ts == Timestamp::MAX {
            return 0;
        }
        let removed = self.column_store.gc_versions(min_active_snapshot_ts);
        self.rebuild_zone_maps();
        removed
    }
}
