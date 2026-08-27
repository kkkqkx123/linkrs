//! MVCC: version-chain management for before-images and conflict detection.

use super::*;

impl PropertyTable {
    /// Ensure `chain_records` has one entry per record row.
    pub(super) fn ensure_chain_len(&mut self) {
        self.chain_records.resize(self.records.len(), Vec::new());
    }

    /// Shared slow path for in-place versioned writes: reject conflicting
    /// writes, supersede the current record into the before-image chain, and
    /// install a new record built from `values` at the same offset. Keeps the
    /// value index, columnar store, and zone maps consistent with the new
    /// version.
    pub(super) fn write_versioned_row(
        &mut self,
        offset: u32,
        values: &[(String, Option<Value>)],
        ts: Timestamp,
    ) -> StorageResult<()> {
        let row_idx =
            prop_offset_to_index(offset).ok_or_else(|| StorageError::invalid_offset(offset))?;
        if row_idx >= self.records.len() {
            return Err(StorageError::invalid_offset(offset));
        }

        // Storage-layer write-write conflict detection: reject a write whose
        // timestamp would overlap a newer existing version or a tombstoned
        // row, before any side effect on indexes or records.
        self.check_write_conflict(row_idx, offset, ts)?;

        // Remove old values from the index before overwriting the record.
        if let Some(old_props) = self.get(offset, None) {
            self.value_index.remove_record(&old_props, offset);
        }

        let new_record = self.serialize_row_with_nulls(values)?;

        // MVCC: supersede the current version. The old row becomes a
        // before-image (visible on `[create_ts, ts)`) and the new row takes
        // over from `ts` onward, preserving historical snapshots.
        self.supersede_current(row_idx, offset, ts);

        let new_record_obj = PropertyRecord::new(new_record, ts);
        self.used_data_bytes += new_record_obj.data.len();
        self.records[row_idx] = Some(new_record_obj);

        // Columnar sync: version every column named in `values`.
        for (name, value) in values {
            if self.has_property(name) {
                let _ = self
                    .column_store
                    .set_property_versioned(row_idx, name, value.as_ref(), ts);
            }
        }
        self.refresh_zone_map_for_row(row_idx);

        // Re-index with new values
        self.value_index.index_record(values, offset);

        Ok(())
    }

    /// Resolve the property row visible at `query_ts` (snapshot read).
    ///
    /// Cheap inspection that mirrors [`PropertyTable::get`]'s visibility
    /// rules without paying for deserialization.
    pub(super) fn resolve_version(
        &self,
        row_idx: usize,
        query_ts: Option<Timestamp>,
    ) -> Option<&PropertyRecord> {
        let record = match query_ts {
            None => {
                let rec = self.records[row_idx].as_ref()?;
                if rec.delete_ts.is_some() {
                    return None;
                }
                rec
            }
            Some(ts) => {
                if let Some(rec) = self.records[row_idx].as_ref() {
                    if rec.is_visible_at(ts) {
                        return Some(rec);
                    }
                }
                self.chain_records
                    .get(row_idx)?
                    .iter()
                    .find(|record| record.is_visible_at(ts))?
            }
        };
        Some(record)
    }

    /// Supersede the current version of a row in favor of a newer one.
    ///
    /// Marks the current record as tombstoned at `ts` and pushes it into the
    /// before-image chain (visible on `[create_ts, ts)`), mirroring the
    /// vertex `Column::set_versioned` guard: a before-image is only useful
    /// when the current version genuinely predates the write
    /// (`create_ts < ts`). Same-timestamp re-writes (rollback / WAL redo that
    /// reuses the transaction timestamp) and already-deleted rows produce no
    /// observable intermediate state, so they skip the chain entry.
    pub(super) fn supersede_current(&mut self, row_idx: usize, offset: u32, ts: Timestamp) {
        let should_version = self.records[row_idx]
            .as_ref()
            .is_some_and(|r| r.delete_ts.is_none() && r.create_ts < ts);

        if let Some(record) = &mut self.records[row_idx] {
            if record.delete_ts.is_none() {
                record.delete_ts = Some(ts);
                self.tombstones_manager.add_tombstone(offset, ts);
            }
        }

        if should_version {
            if let Some(record) = self.records[row_idx].as_ref() {
                if self.chain_records.len() <= row_idx {
                    self.chain_records.resize(row_idx + 1, Vec::new());
                }
                self.chain_records[row_idx].push(record.clone());
                // Bound the chain length: fold the oldest before-images once
                // the cap is exceeded so memory stays bounded.
                self.fold_oldest_versions(row_idx);
            }
        }
    }

    /// Bound the before-image chain length for `row_idx` by folding the oldest
    /// entries when the chain exceeds `version_chain_cap`.
    ///
    /// Folding merges the two oldest before-images: the older entry's data is
    /// kept as the representative and its visibility interval `[create_ts,
    /// delete_ts)` is extended to cover the second entry's interval, which is
    /// then dropped. This preserves the original oldest value and the newest
    /// current value while coarsening intermediate history, so the most recent
    /// updates remain exact.
    ///
    /// A cap of `0` disables the bound (unbounded history).
    fn fold_oldest_versions(&mut self, row_idx: usize) {
        let cap = self.version_chain_cap;
        if cap == 0 {
            return;
        }
        let horizon = self.retention_horizon;
        let chain = &mut self.chain_records[row_idx];
        while chain.len() > cap {
            if chain.len() < 2 {
                break;
            }
            // Never fold an entry that an active snapshot may still observe:
            // its visibility interval must end before the retention horizon.
            let can_fold = chain[1]
                .delete_ts
                .is_none_or(|delete_ts| delete_ts <= horizon);
            if !can_fold {
                break;
            }
            // Merge the two oldest entries into one: keep the older data,
            // extend its interval to cover the younger entry.
            let second = chain.remove(1);
            if let Some(end) = second.delete_ts {
                chain[0].delete_ts = Some(end);
            }
            self.used_data_bytes = self.used_data_bytes.saturating_sub(second.data.len());
        }
    }

    /// Reject a write whose timestamp would contradict the row's current
    /// version. This is the storage-layer write-write conflict detection:
    ///
    /// - Writing at `ts` strictly **before** the current version's creation
    ///   time would clobber a newer version without preserving it as history
    ///   (a "back-in-time" write overlapping an existing interval).
    /// - Writing at `ts` strictly **after** the row was marked deleted writes
    ///   to a tombstoned entity.
    ///
    /// Same-timestamp re-writes (rollback / WAL redo that reuse the original
    /// transaction timestamp) and strictly forward writes (the normal
    /// time-travel version chain) pass through unchanged, preserving the
    /// distinction between "concurrent transaction conflict" and "historical
    /// version write".
    pub(super) fn check_write_conflict(
        &self,
        row_idx: usize,
        offset: u32,
        ts: Timestamp,
    ) -> StorageResult<()> {
        let Some(record) = self.records[row_idx].as_ref() else {
            return Ok(());
        };
        if let Some(del_ts) = record.delete_ts {
            if ts > del_ts {
                return Err(StorageError::write_write_conflict(format!(
                    "property row at offset {} deleted at ts={}, attempted write at ts={}",
                    offset, del_ts, ts
                )));
            }
        } else if record.create_ts > ts {
            return Err(StorageError::write_write_conflict(format!(
                "property row at offset {} already has a newer version at ts={}, attempted write at ts={}",
                offset, record.create_ts, ts
            )));
        }
        Ok(())
    }

    /// Garbage collect before-image chain entries no longer visible to any
    /// active snapshot at `min_active_snapshot_ts`. Returns the number of
    /// entries removed.
    ///
    /// An entry is obsolete when its whole visibility interval `[create_ts,
    /// delete_ts)` precedes the oldest active snapshot, i.e. `delete_ts <=
    /// min_active_snapshot_ts`. The current record is never reclaimed here
    /// (it still owns the row); fully-deleted rows are reclaimed through
    /// [`PropertyTable::gc_tombstones`].
    pub fn gc_versions(&mut self, min_active_snapshot_ts: Timestamp) -> usize {
        // An unbounded horizon (`MAX`) means nothing pins history — but it is
        // not a real timestamp: treating it as one would reclaim every
        // before-image, including history arbitrary-ts time-travel reads may
        // still request. Without a bound (no active snapshot and no retention
        // floor) nothing is reclaimable.
        if min_active_snapshot_ts == Timestamp::MAX {
            return 0;
        }
        let mut removed = 0usize;
        let mut reclaimed_bytes = 0usize;
        for chain in &mut self.chain_records {
            let before_len = chain.len();
            let before_bytes: usize = chain.iter().map(|e| e.data.len()).sum();
            chain.retain(|entry| entry.delete_ts.is_none_or(|d| d > min_active_snapshot_ts));
            let after_bytes: usize = chain.iter().map(|e| e.data.len()).sum();
            removed += before_len - chain.len();
            reclaimed_bytes += before_bytes - after_bytes;
        }
        self.used_data_bytes = self.used_data_bytes.saturating_sub(reclaimed_bytes);
        // Columnar GC: reclaim per-cell version chains as well (side effect only;
        // return value stays row-chain-centric for backward compatibility with
        // existing tests that assert exact counts).
        let _ = self.column_store.gc_versions(min_active_snapshot_ts);
        // Rebuild zone maps for chunks that may have had history reclaimed.
        self.rebuild_zone_maps();
        removed
    }
}
