//! Tombstones: deletion marks, tombstone GC, and slot reclamation (column-only).

use super::*;

impl PropertyTable {
    pub fn mark_deleted(&mut self, offset: u32, delete_ts: Timestamp) -> StorageResult<()> {
        let row_idx =
            prop_offset_to_index(offset).ok_or_else(|| StorageError::invalid_offset(offset))?;
        if row_idx >= self.row_create_ts.len() || self.row_create_ts[row_idx] == 0 {
            return Ok(());
        }
        if self.row_delete_ts.get(row_idx).and_then(|v| *v).is_some() {
            return Err(StorageError::invalid_operation(
                "record already marked deleted",
            ));
        }
        if let Some(props) = self.get(offset, None) {
            self.value_index.remove_record(&props, offset);
        }
        if let Some(slot) = self.row_delete_ts.get_mut(row_idx) {
            *slot = Some(delete_ts);
        } else {
            self.row_delete_ts.resize(row_idx + 1, None);
            self.row_delete_ts[row_idx] = Some(delete_ts);
        }
        self.tombstones_manager.add_tombstone(offset, delete_ts);
        self.refresh_zone_map_for_row(row_idx);
        Ok(())
    }

    pub fn revert_deletion(&mut self, offset: u32) -> bool {
        let row_idx = match prop_offset_to_index(offset) {
            Some(idx) => idx,
            None => return false,
        };
        if row_idx >= self.row_create_ts.len() || self.row_create_ts[row_idx] == 0 {
            return false;
        }
        let cur = self.row_delete_ts.get(row_idx).and_then(|v| *v);
        if cur.is_none() {
            return false;
        }
        if let Some(slot) = self.row_delete_ts.get_mut(row_idx) {
            *slot = None;
        }
        self.tombstones_manager.remove(offset);
        if let Some(props) = self.get(offset, None) {
            self.value_index.index_record(&props, offset);
        }
        self.refresh_zone_map_for_row(row_idx);
        true
    }

    pub fn is_deleted(&self, offset: u32) -> bool {
        let Some(row_idx) = prop_offset_to_index(offset) else {
            return false;
        };
        self.row_delete_ts
            .get(row_idx)
            .and_then(|v| *v)
            .is_some()
    }

    pub fn gc_tombstones(&mut self, min_active_snapshot_ts: Timestamp) -> u32 {
        let batch_size = 10_000usize;
        self.tombstones_manager
            .gc_batch(min_active_snapshot_ts, batch_size);
        self.tombstones_manager.gc(min_active_snapshot_ts);

        let mut reclaimed = 0u32;
        let mut indices_to_clear = Vec::new();
        for (idx, del_opt) in self.row_delete_ts.iter().enumerate() {
            if let Some(delete_ts) = del_opt {
                if *delete_ts < min_active_snapshot_ts && self.row_create_ts[idx] != 0 {
                    let offset = prop_index_to_offset(idx);
                    indices_to_clear.push((idx, offset));
                    reclaimed += 1;
                }
            }
        }
        for (idx, offset) in &indices_to_clear {
            let live_props = self.column_store.get(*idx);
            let opt_props: Vec<(String, Option<Value>)> = self
                .schema
                .iter()
                .map(|s| {
                    let v = live_props
                        .iter()
                        .find(|(n, _)| n == &s.name)
                        .and_then(|(_, v)| v.clone());
                    (s.name.clone(), v)
                })
                .collect();
            self.value_index.remove_record(&opt_props, *offset);
        }
        let has_cleared = !indices_to_clear.is_empty();
        for (idx, offset) in indices_to_clear {
            self.row_create_ts[idx] = 0;
            self.row_delete_ts[idx] = None;
            self.column_store.clear_row_version_chains(idx);
            self.free_list.push(offset);
        }
        if has_cleared {
            self.rebuild_zone_maps();
        }
        reclaimed
    }

    pub fn delete(&mut self, offset: u32) -> bool {
        let row_idx = match prop_offset_to_index(offset) {
            Some(idx) => idx,
            None => return false,
        };
        if row_idx >= self.row_create_ts.len() || self.row_create_ts[row_idx] == 0 {
            return false;
        }
        if let Some(props) = self.get(offset, None) {
            self.value_index.remove_record(&props, offset);
        }
        self.tombstones_manager.remove(offset);
        self.row_create_ts[row_idx] = 0;
        self.row_delete_ts[row_idx] = None;
        self.column_store.clear_row_version_chains(row_idx);
        self.free_list.push(offset);
        self.refresh_zone_map_for_row(row_idx);
        true
    }

    pub fn reclaim_slots(
        &mut self,
        valid_offsets: &HashSet<u32>,
        retention_bound: Timestamp,
    ) -> usize {
        if retention_bound == Timestamp::MAX {
            return 0;
        }
        let mut reclaimed = 0usize;
        let snapshot: Vec<(usize, u32, Option<Timestamp>)> = (0..self.row_create_ts.len())
            .map(|idx| {
                let offset = prop_index_to_offset(idx);
                let del = self.row_delete_ts.get(idx).and_then(|v| *v);
                (idx, offset, del)
            })
            .collect();
        let mut to_free = Vec::new();
        for (idx, offset, del_opt) in snapshot {
            if valid_offsets.contains(&offset) {
                continue;
            }
            if self.row_create_ts[idx] == 0 {
                continue;
            }
            let Some(delete_ts) = del_opt else {
                continue;
            };
            if delete_ts > retention_bound {
                continue;
            }
            to_free.push((idx, offset));
        }
        for (idx, offset) in to_free {
            let live_props = self.column_store.get(idx);
            let opt_props: Vec<(String, Option<Value>)> = self
                .schema
                .iter()
                .map(|s| {
                    let v = live_props
                        .iter()
                        .find(|(n, _)| n == &s.name)
                        .and_then(|(_, v)| v.clone());
                    (s.name.clone(), v)
                })
                .collect();
            self.value_index.remove_record(&opt_props, offset);
            self.tombstones_manager.remove(offset);
            self.row_create_ts[idx] = 0;
            self.row_delete_ts[idx] = None;
            self.column_store.clear_row_version_chains(idx);
            self.free_list.push(offset);
            self.row_count = self.row_count.saturating_sub(1);
            reclaimed += 1;
        }
        if reclaimed > 0 {
            self.rebuild_zone_maps();
        }
        reclaimed
    }
}
