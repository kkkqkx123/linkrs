use super::ShardedVertexTable;
use crate::vertex::{IdKey, PkLookup, WriteScope};
use graphdb_core::types::Timestamp;
use graphdb_core::{StorageError, StorageResult, Value};
use std::sync::Arc;

type ShardStagedInsert = (IdKey, u32, Vec<(Arc<str>, Value)>);

/// Every mutation a tracked commit installed, addressed by global id so a
/// later durability failure can compensate the whole set.
#[derive(Debug, Default)]
pub struct CommitApplied {
    pub mapping: Vec<(IdKey, u32)>,
    pub inserts: Vec<u32>,
    pub updates: Vec<(u32, Arc<str>)>,
    pub deletes: Vec<u32>,
    pub property_deletes: Vec<(u32, Arc<str>)>,
}

impl CommitApplied {
    fn from_shard_parts(
        table: &ShardedVertexTable,
        mapping: Vec<(IdKey, u32)>,
        applied_updates: Vec<(usize, u32, Arc<str>)>,
        applied_deletes: Vec<(usize, u32)>,
        applied_property_deletes: Vec<(usize, u32, Arc<str>)>,
    ) -> Self {
        Self {
            inserts: mapping.iter().map(|(_, id)| *id).collect(),
            mapping,
            updates: applied_updates
                .into_iter()
                .map(|(shard_idx, local_id, col)| (table.encode_id(shard_idx, local_id), col))
                .collect(),
            deletes: applied_deletes
                .into_iter()
                .map(|(shard_idx, local_id)| table.encode_id(shard_idx, local_id))
                .collect(),
            property_deletes: applied_property_deletes
                .into_iter()
                .map(|(shard_idx, local_id, col)| (table.encode_id(shard_idx, local_id), col))
                .collect(),
        }
    }
}

impl ShardedVertexTable {
    pub fn insert(
        &self,
        external_id: &str,
        properties: &[(Arc<str>, Value)],
        ts: Timestamp,
    ) -> StorageResult<u32> {
        let idx = self.shard_index_by_str(external_id);
        // Point inserts run under the shard read guard: the identity step
        // inside the table serializes duplicate check plus allocation on
        // the identity latch, and the data step uses segment latches, so
        // concurrent inserts to different rows of one shard proceed
        // together while the same key still allocates exactly once.
        let table = self.shards[idx].read();
        let local_id = table.insert(external_id, properties, ts)?;
        Ok(self.record_allocation(idx, local_id))
    }

    pub fn insert_by_i64(
        &self,
        external_id: i64,
        properties: &[(Arc<str>, Value)],
        ts: Timestamp,
    ) -> StorageResult<u32> {
        let idx = self.shard_index_by_i64(external_id);
        let table = self.shards[idx].read();
        let local_id = table.insert_by_i64(external_id, properties, ts)?;
        Ok(self.record_allocation(idx, local_id))
    }

    pub fn update_property(
        &self,
        global_id: u32,
        col_name: &str,
        value: &Value,
        ts: Timestamp,
    ) -> StorageResult<()> {
        let (idx, local_id) = self.decode_id(global_id);
        let table = self.shards[idx].read();
        table.update_property(local_id, col_name, value, ts)
    }

    pub fn delete_property_by_global_id(
        &self,
        global_id: u32,
        col_name: &str,
        ts: Timestamp,
    ) -> StorageResult<()> {
        let (idx, local_id) = self.decode_id(global_id);
        let table = self.shards[idx].read();
        table.delete_property(local_id, col_name, ts)
    }

    pub fn delete_by_internal_id(&self, global_id: u32, ts: Timestamp) -> StorageResult<()> {
        let (idx, local_id) = self.decode_id(global_id);
        let table = self.shards[idx].read();
        table.delete_by_internal_id(local_id, ts)
    }

    pub fn reserve_id_capacity(&self, additional: usize) {
        // Every component serializes internally (id indexer mutex, identity
        // latch for timestamps, tail-only growth under the segment channel),
        // so the shared guard is enough and the reservation stays
        // concurrent with point paths on the same shard.
        for shard in &self.shards {
            shard.read().reserve_id_capacity(additional);
        }
    }

    /// Empty-table fast path for initial loads.
    ///
    /// Rejects non-empty tables with a guidance error; callers then use the
    /// incremental batch entries. When `sorted` is true the input keys must
    /// arrive ordered and unique, verified upfront without per-row hash
    /// probes; a false declaration fails instead of silently falling back.
    /// Capacity is reserved once before any global mutation, rows apply under
    /// one lock hold per shard, and any row failure rolls back the applied
    /// prefix so the table stays empty.
    pub fn bulk_import_str(
        &self,
        rows: &[(&str, &[(Arc<str>, Value)])],
        ts: Timestamp,
        sorted: bool,
    ) -> StorageResult<usize> {
        // Empty-table gate runs quiescently (no concurrent writers during
        // bulk import), so the approximate total — which includes tombstones —
        // is exact here: non-zero means definitely non-empty, zero means
        // definitely empty.
        if self.approximate_total_count() != 0 {
            return Err(StorageError::invalid_operation(
                "bulk import requires an empty table: use insert_batch for incremental writes"
                    .to_string(),
            ));
        }
        if sorted {
            for pair in rows.windows(2) {
                if pair[0].0 >= pair[1].0 {
                    return Err(StorageError::invalid_input(
                        "sorted bulk import input must be strictly ordered and unique".to_string(),
                    ));
                }
            }
        }
        self.reserve_id_capacity(rows.len());
        let results = self.insert_batch_str(rows, ts);
        let mut applied: Vec<&str> = Vec::new();
        let mut first_error: Option<StorageError> = None;
        for ((external_id, _), result) in rows.iter().zip(results) {
            match result {
                Ok(_) => applied.push(external_id),
                Err(e) => {
                    if first_error.is_none() {
                        first_error = Some(e);
                    }
                }
            }
        }
        if let Some(e) = first_error {
            let applied_ids: Vec<u32> = applied
                .iter()
                .filter_map(|name| self.get_internal_id(name, ts))
                .collect();
            self.undo_applied_ids(&applied_ids);
            return Err(e);
        }
        Ok(applied.len())
    }

    /// Integer-keyed empty-table fast path. Same contract as
    /// [`Self::bulk_import_str`].
    ///
    /// [`Self::bulk_import_str`]: Self::bulk_import_str
    pub fn bulk_import_i64(
        &self,
        rows: &[(i64, &[(Arc<str>, Value)])],
        ts: Timestamp,
        sorted: bool,
    ) -> StorageResult<usize> {
        // Same quiescent empty-table gate as `bulk_import_str`.
        if self.approximate_total_count() != 0 {
            return Err(StorageError::invalid_operation(
                "bulk import requires an empty table: use insert_batch for incremental writes"
                    .to_string(),
            ));
        }
        if sorted {
            for pair in rows.windows(2) {
                if pair[0].0 >= pair[1].0 {
                    return Err(StorageError::invalid_input(
                        "sorted bulk import input must be strictly ordered and unique".to_string(),
                    ));
                }
            }
        }
        self.reserve_id_capacity(rows.len());
        let results = self.insert_batch_i64(rows, ts);
        let mut applied: Vec<i64> = Vec::new();
        let mut first_error: Option<StorageError> = None;
        for ((external_id, _), result) in rows.iter().zip(results) {
            match result {
                Ok(_) => applied.push(*external_id),
                Err(e) => {
                    if first_error.is_none() {
                        first_error = Some(e);
                    }
                }
            }
        }
        if let Some(e) = first_error {
            let applied_ids: Vec<u32> = applied
                .iter()
                .filter_map(|name| self.get_internal_id_by_i64(*name, ts))
                .collect();
            self.undo_applied_ids(&applied_ids);
            return Err(e);
        }
        Ok(applied.len())
    }

    /// Shard-grouped batch apply of a string-keyed insert batch.
    ///
    /// Rows are first routed to their owning shards without taking any lock
    /// (the routing step), then each non-empty shard group is applied under
    /// a single shared-guard hold in input order (the merge step), each row
    /// running its own identity-then-segment point path. Results are
    /// aligned with the input: every row is attempted even after earlier
    /// failures, so the caller rolls back every `Ok` entry when any `Err`
    /// is present. Duplicate keys fail per row exactly as [`insert`] does.
    ///
    /// [`insert`]: Self::insert
    pub fn insert_batch_str(
        &self,
        rows: &[(&str, &[(Arc<str>, Value)])],
        ts: Timestamp,
    ) -> Vec<StorageResult<u32>> {
        let mut by_shard: Vec<Vec<usize>> = vec![Vec::new(); self.layout.num_shards];
        let estimate = rows.len().div_ceil(self.layout.num_shards.max(1)).max(1);
        for group in by_shard.iter_mut() {
            group.reserve(estimate);
        }
        for (pos, (external_id, _)) in rows.iter().enumerate() {
            by_shard[self.shard_index_by_str(external_id)].push(pos);
        }
        let mut out: Vec<Option<StorageResult<u32>>> = (0..rows.len()).map(|_| None).collect();
        for (shard_idx, positions) in by_shard.iter().enumerate() {
            if positions.is_empty() {
                continue;
            }
            let table = self.shards[shard_idx].read();
            for &pos in positions {
                let (external_id, properties) = &rows[pos];
                let result = table
                    .insert(external_id, properties, ts)
                    .map(|local_id| self.record_allocation(shard_idx, local_id));
                out[pos] = Some(result);
            }
        }
        out.into_iter()
            .map(|slot| slot.expect("every input row is routed once"))
            .collect()
    }

    /// Shard-grouped staged apply of an integer-keyed insert batch. Same
    /// staging/merge contract as [`insert_batch_str`].
    ///
    /// [`insert_batch_str`]: Self::insert_batch_str
    pub fn insert_batch_i64(
        &self,
        rows: &[(i64, &[(Arc<str>, Value)])],
        ts: Timestamp,
    ) -> Vec<StorageResult<u32>> {
        let mut by_shard: Vec<Vec<usize>> = vec![Vec::new(); self.layout.num_shards];
        let estimate = rows.len().div_ceil(self.layout.num_shards.max(1)).max(1);
        for group in by_shard.iter_mut() {
            group.reserve(estimate);
        }
        for (pos, (external_id, _)) in rows.iter().enumerate() {
            by_shard[self.shard_index_by_i64(*external_id)].push(pos);
        }
        let mut out: Vec<Option<StorageResult<u32>>> = (0..rows.len()).map(|_| None).collect();
        for (shard_idx, positions) in by_shard.iter().enumerate() {
            if positions.is_empty() {
                continue;
            }
            let table = self.shards[shard_idx].read();
            for &pos in positions {
                let (external_id, properties) = &rows[pos];
                let result = table
                    .insert_by_i64(*external_id, properties, ts)
                    .map(|local_id| self.record_allocation(shard_idx, local_id));
                out[pos] = Some(result);
            }
        }
        out.into_iter()
            .map(|slot| slot.expect("every input row is routed once"))
            .collect()
    }

    /// Read-only validation and normalization for a staged update column
    /// set: primary-key rejection, column existence and type casts. Shard 0
    /// is the schema authority, so any shard answers identically.
    pub fn prepare_vertex_update(
        &self,
        columns: &[(Arc<str>, Value)],
    ) -> StorageResult<Vec<(std::sync::Arc<str>, Value)>> {
        let table = self.shards[0].read();
        columns
            .iter()
            .map(|(name, value)| Ok((name.clone(), table.prepare_update(name, value)?)))
            .collect()
    }

    /// Reserve the global vertex id one staged row will commit under.
    ///
    /// A key already bound in its shard (live or tombstoned awaiting GC)
    /// reports the existing id, mirroring the re-insert reuse of
    /// [`VertexTable::apply_insert`]; otherwise the owning shard takes an
    /// unbound local slot and nothing is visible to lookups, scans or the
    /// delta log until the commit apply binds the key to the returned id.
    /// Callers stage inserts before commit and must hand an unused
    /// reservation back through [`Self::release_reserved_vertex`].
    ///
    /// [`VertexTable::apply_insert`]: crate::vertex::VertexTable::apply_insert
    pub fn reserve_vertex_id(&self, key: &IdKey) -> StorageResult<u32> {
        let idx = match key {
            IdKey::Text(name) => self.shard_index_by_str(name),
            IdKey::Int(n) => self.shard_index_by_i64(*n),
        };
        let local_id = self.shards[idx].read().reserve_identity(key)?;
        Ok(self.record_allocation(idx, local_id))
    }

    /// Return a reserved global vertex id to its shard's free stack. A
    /// no-op when the id was already bound or already recycled, so release
    /// paths only need single ownership of the reservation.
    pub fn release_reserved_vertex(&self, global_id: u32) {
        let (idx, local_id) = self.decode_id(global_id);
        self.shards[idx].read().release_reserved_identity(local_id);
    }

    /// Cancel a previous release of a reserved global vertex id, pulling it
    /// back out of the owning shard's free stack. False means the id is no
    /// longer a reclaimable release and the caller must reserve a fresh one.
    pub fn try_reclaim_reserved_vertex(&self, global_id: u32) -> bool {
        let (idx, local_id) = self.decode_id(global_id);
        self.shards[idx]
            .read()
            .try_reclaim_reserved_identity(local_id)
    }

    /// Scoped insert staging the caller-owned row.
    ///
    /// Validates and normalizes the row under the shard read guard,
    /// reserves the row's global id and buffers it in the scope; the id
    /// stays unbound until the commit hook applies it. Same-scope
    /// duplicates and over-capacity rows fail here without leaving a
    /// reservation behind. The scope is a single-request staging area bound
    /// to `ts`, not a transaction write set; timestamp mismatches are
    /// rejected.
    pub fn insert_with_scope(
        &self,
        external_id: &str,
        properties: &[(std::sync::Arc<str>, Value)],
        ts: Timestamp,
        scope: &mut WriteScope,
    ) -> StorageResult<()> {
        scope.ensure_same_write_ts(ts)?;
        let key = IdKey::Text(external_id.to_string());
        let idx = self.shard_index_by_str(external_id);
        let table = self.shards[idx].read();
        let prepared = table.prepare_insert(&key, properties)?;
        let reserved = table.reserve_identity(&key)?;
        let reserved_global = self.record_allocation(idx, reserved);
        if let Err(error) = scope.stage_insert(self.label, key, reserved_global, prepared) {
            table.release_reserved_identity(reserved);
            return Err(error);
        }
        Ok(())
    }

    /// Integer-keyed scoped insert staging. Same contract as
    /// [`Self::insert_with_scope`].
    ///
    /// [`Self::insert_with_scope`]: Self::insert_with_scope
    pub fn insert_by_i64_with_scope(
        &self,
        external_id: i64,
        properties: &[(Arc<str>, Value)],
        ts: Timestamp,
        scope: &mut WriteScope,
    ) -> StorageResult<()> {
        scope.ensure_same_write_ts(ts)?;
        let key = IdKey::Int(external_id);
        let idx = self.shard_index_by_i64(external_id);
        let table = self.shards[idx].read();
        let prepared = table.prepare_insert(&key, properties)?;
        let reserved = table.reserve_identity(&key)?;
        let reserved_global = self.record_allocation(idx, reserved);
        if let Err(error) = scope.stage_insert(self.label, key, reserved_global, prepared) {
            table.release_reserved_identity(reserved);
            return Err(error);
        }
        Ok(())
    }

    /// Scoped batch insert staging: every row is validated and buffered
    /// without touching global state. Results stay aligned with the input;
    /// same-scope duplicates, validation failures and over-capacity rows
    /// fail per row. The commit hook applies the staged rows.
    pub fn insert_batch_str_with_scope(
        &self,
        rows: &[(&str, &[(Arc<str>, Value)])],
        ts: Timestamp,
        scope: &mut WriteScope,
    ) -> Vec<StorageResult<()>> {
        if scope.ensure_same_write_ts(ts).is_err() {
            return rows
                .iter()
                .map(|_| {
                    Err(StorageError::invalid_operation(
                        "write scope timestamp mismatch: scopes are single-request only"
                            .to_string(),
                    ))
                })
                .collect();
        }
        rows.iter()
            .map(|(external_id, properties)| {
                self.insert_with_scope(external_id, properties, ts, scope)
            })
            .collect()
    }

    /// Integer-keyed scoped batch insert staging. Same contract as
    /// [`Self::insert_batch_str_with_scope`].
    ///
    /// [`Self::insert_batch_str_with_scope`]: Self::insert_batch_str_with_scope
    pub fn insert_batch_i64_with_scope(
        &self,
        rows: &[(i64, &[(Arc<str>, Value)])],
        ts: Timestamp,
        scope: &mut WriteScope,
    ) -> Vec<StorageResult<()>> {
        if scope.ensure_same_write_ts(ts).is_err() {
            return rows
                .iter()
                .map(|_| {
                    Err(StorageError::invalid_operation(
                        "write scope timestamp mismatch: scopes are single-request only"
                            .to_string(),
                    ))
                })
                .collect();
        }
        rows.iter()
            .map(|(external_id, properties)| {
                self.insert_by_i64_with_scope(*external_id, properties, ts, scope)
            })
            .collect()
    }

    /// Scoped property update staging against a resolved global id.
    ///
    /// Existence is checked read-only; the converted value is buffered and
    /// applied by the commit hook. Primary-key columns stay rejected at
    /// apply time through the shared table path.
    pub fn update_property_with_scope(
        &self,
        global_id: u32,
        col_name: &str,
        value: &Value,
        ts: Timestamp,
        scope: &mut WriteScope,
    ) -> StorageResult<()> {
        scope.ensure_same_write_ts(ts)?;
        let (idx, local_id) = self.decode_id(global_id);
        let table = self.shards[idx].read();
        table
            .get_external_id(local_id, ts)
            .ok_or(StorageError::vertex_not_found())?;
        scope.stage_update(
            self.label,
            global_id,
            vec![(Arc::from(col_name), value.clone())],
        )?;
        Ok(())
    }

    /// Scoped column-deletion staging against a resolved global id.
    ///
    /// Only row liveness is prechecked here; column values are never
    /// checked. The tombstone lands at commit time.
    pub fn delete_property_with_scope(
        &self,
        global_id: u32,
        col_name: &str,
        ts: Timestamp,
        scope: &mut WriteScope,
    ) -> StorageResult<()> {
        scope.ensure_same_write_ts(ts)?;
        let (idx, local_id) = self.decode_id(global_id);
        let table = self.shards[idx].read();
        table
            .get_external_id(local_id, ts)
            .ok_or(StorageError::vertex_not_found())?;
        if table.is_pk_column(col_name) {
            return Err(StorageError::invalid_operation(format!(
                "Primary key column '{}' cannot be deleted by row replacement",
                col_name
            )));
        }
        scope.stage_property_deletes(self.label, global_id, vec![Arc::from(col_name)])?;
        Ok(())
    }

    /// Scoped delete staging by external id. Existence is resolved read-only;
    /// the tombstone is applied by the commit hook.
    pub fn delete_with_scope(
        &self,
        external_id: &str,
        ts: Timestamp,
        scope: &mut WriteScope,
    ) -> StorageResult<()> {
        scope.ensure_same_write_ts(ts)?;
        let idx = self.shard_index_by_str(external_id);
        let table = self.shards[idx].read();
        let local_id = match table.lookup_internal_id(&IdKey::Text(external_id.to_string()), ts) {
            PkLookup::Visible(local_id) => local_id,
            PkLookup::Missing => return Err(StorageError::vertex_not_found()),
        };
        scope.stage_delete(self.label, self.encode_id(idx, local_id))?;
        Ok(())
    }

    /// Integer-keyed scoped delete staging. Same contract as
    /// [`Self::delete_with_scope`].
    ///
    /// [`Self::delete_with_scope`]: Self::delete_with_scope
    pub fn delete_by_i64_with_scope(
        &self,
        external_id: i64,
        ts: Timestamp,
        scope: &mut WriteScope,
    ) -> StorageResult<()> {
        scope.ensure_same_write_ts(ts)?;
        let idx = self.shard_index_by_i64(external_id);
        let table = self.shards[idx].read();
        let local_id = match table.lookup_internal_id(&IdKey::Int(external_id), ts) {
            PkLookup::Visible(local_id) => local_id,
            PkLookup::Missing => return Err(StorageError::vertex_not_found()),
        };
        scope.stage_delete(self.label, self.encode_id(idx, local_id))?;
        Ok(())
    }

    /// Scoped batch delete staging. Returns per-row staging results;
    /// unresolvable ids fail per row and stage nothing, while resolvable
    /// ids are buffered. The commit hook applies the staged set.
    pub fn batch_delete_with_scope(
        &self,
        external_ids: &[&str],
        ts: Timestamp,
        scope: &mut WriteScope,
    ) -> Vec<StorageResult<()>> {
        if scope.ensure_same_write_ts(ts).is_err() {
            return external_ids
                .iter()
                .map(|_| {
                    Err(StorageError::invalid_operation(
                        "write scope timestamp mismatch: scopes are single-request only"
                            .to_string(),
                    ))
                })
                .collect();
        }
        external_ids
            .iter()
            .map(|external_id| self.delete_with_scope(external_id, ts, scope))
            .collect()
    }

    /// Integer-keyed scoped batch delete staging. Same contract as
    /// [`Self::batch_delete_with_scope`].
    ///
    /// [`Self::batch_delete_with_scope`]: Self::batch_delete_with_scope
    pub fn batch_delete_i64_with_scope(
        &self,
        external_ids: &[i64],
        ts: Timestamp,
        scope: &mut WriteScope,
    ) -> Vec<StorageResult<()>> {
        if scope.ensure_same_write_ts(ts).is_err() {
            return external_ids
                .iter()
                .map(|_| {
                    Err(StorageError::invalid_operation(
                        "write scope timestamp mismatch: scopes are single-request only"
                            .to_string(),
                    ))
                })
                .collect();
        }
        external_ids
            .iter()
            .map(|external_id| self.delete_by_i64_with_scope(*external_id, ts, scope))
            .collect()
    }

    /// Tracked commit hook reporting every applied mutation for later
    /// compensation: a durability failure after this apply (WAL append,
    /// index replay) undoes the whole set through
    /// [`Self::undo_applied_commit`] instead of inserts only.
    pub fn commit_write_scope_tracked(
        &self,
        scope: &mut WriteScope,
        ts: Timestamp,
    ) -> StorageResult<CommitApplied> {
        scope.ensure_same_write_ts(ts)?;
        if let Err(error) = self.prevalidate_staged(scope, ts) {
            self.rollback_write_scope(scope, ts);
            return Err(error);
        }
        let mut applied: Vec<(usize, u32)> = Vec::new();
        let mut applied_updates: Vec<(usize, u32, Arc<str>)> = Vec::new();
        let mut applied_property_deletes: Vec<(usize, u32, Arc<str>)> = Vec::new();
        let mut applied_deletes: Vec<(usize, u32)> = Vec::new();
        let mut mapping: Vec<(IdKey, u32)> = Vec::new();
        let result = self.apply_staged_inserts(scope, ts, &mut applied, &mut mapping);
        if let Err(error) = result {
            self.undo_applied_inserts(&applied);
            self.rollback_write_scope(scope, ts);
            return Err(error);
        }
        if let Err(error) = self.apply_staged_updates(scope, ts, &mut applied_updates) {
            self.undo_applied_updates(&applied_updates, ts);
            self.undo_applied_inserts(&applied);
            self.rollback_write_scope(scope, ts);
            return Err(error);
        }
        if let Err(error) =
            self.apply_staged_property_deletes(scope, ts, &mut applied_property_deletes)
        {
            self.undo_applied_updates(&applied_property_deletes, ts);
            self.undo_applied_updates(&applied_updates, ts);
            self.undo_applied_inserts(&applied);
            self.rollback_write_scope(scope, ts);
            return Err(error);
        }
        if let Err(error) = self.apply_staged_deletes(scope, ts, &mut applied_deletes) {
            self.undo_applied_deletes(&applied_deletes);
            self.undo_applied_updates(&applied_property_deletes, ts);
            self.undo_applied_updates(&applied_updates, ts);
            self.undo_applied_inserts(&applied);
            self.rollback_write_scope(scope, ts);
            return Err(error);
        }
        scope.commit_label(self.label);
        Ok(CommitApplied::from_shard_parts(
            self,
            mapping,
            applied_updates,
            applied_deletes,
            applied_property_deletes,
        ))
    }

    /// Compensate a previous tracked apply in reverse order: deletes are
    /// revived, updates and column tombstones pop their version entry,
    /// inserts drop their keys.
    pub fn undo_applied_commit(&self, applied: &CommitApplied, ts: Timestamp) {
        for global_id in applied.deletes.iter().rev() {
            let (idx, local_id) = self.decode_id(*global_id);
            self.shards[idx].read().revert_delete(local_id);
        }
        for (global_id, col_name) in applied.property_deletes.iter().rev() {
            let (idx, local_id) = self.decode_id(*global_id);
            let _ = self.shards[idx].read().undo_update(local_id, col_name, ts);
        }
        for (global_id, col_name) in applied.updates.iter().rev() {
            let (idx, local_id) = self.decode_id(*global_id);
            let _ = self.shards[idx].read().undo_update(local_id, col_name, ts);
        }
        self.undo_applied_ids(&applied.inserts);
    }

    fn prevalidate_staged(&self, scope: &mut WriteScope, ts: Timestamp) -> StorageResult<()> {
        // Rows created by this commit's own insert stage are exempt from
        // the liveness check; their values were validated at staging.
        let created: std::collections::HashSet<u32> = scope
            .staged_insert_ids_for_label(self.label)
            .into_iter()
            .collect();
        let staged_updates = scope.take_updates_for_label(self.label);
        let mut first_error: Option<StorageError> = None;
        for (global_id, props) in &staged_updates {
            if first_error.is_some() {
                break;
            }
            let (idx, local_id) = self.decode_id(*global_id);
            let table = self.shards[idx].read();
            if created.contains(global_id) {
                for (col_name, value) in props {
                    if table.is_pk_column(col_name) {
                        continue;
                    }
                    if let Err(error) = table.prepare_update(col_name, value) {
                        first_error = Some(error);
                        break;
                    }
                }
                continue;
            }
            if !table.is_row_live_at(local_id, ts) {
                first_error = Some(StorageError::vertex_not_found());
                break;
            }
            for (col_name, value) in props {
                if let Err(error) = table.validate_update_value(local_id, col_name, value) {
                    first_error = Some(error);
                    break;
                }
            }
        }
        for (global_id, props) in staged_updates {
            let _ = scope.stage_update(self.label, global_id, props);
        }
        if let Some(error) = first_error {
            return Err(error);
        }
        let staged_property_deletes = scope.take_property_deletes_for_label(self.label);
        let mut property_delete_error: Option<StorageError> = None;
        for (global_id, cols) in &staged_property_deletes {
            if property_delete_error.is_some() {
                break;
            }
            if created.contains(global_id) {
                property_delete_error = Some(StorageError::invalid_operation(format!(
                    "row {} created by this commit cannot lose columns to row replacement",
                    global_id
                )));
                break;
            }
            let (idx, local_id) = self.decode_id(*global_id);
            let table = self.shards[idx].read();
            if !table.is_row_live_at(local_id, ts) {
                property_delete_error = Some(StorageError::vertex_not_found());
                break;
            }
            for col_name in cols {
                if table.is_pk_column(col_name) {
                    property_delete_error = Some(StorageError::invalid_operation(format!(
                        "Primary key column '{}' cannot be deleted by row replacement",
                        col_name
                    )));
                    break;
                }
                if let Err(error) = table.columns.check_column_available(col_name) {
                    property_delete_error = Some(error);
                    break;
                }
            }
        }
        for (global_id, cols) in staged_property_deletes {
            let _ = scope.stage_property_deletes(self.label, global_id, cols);
        }
        if let Some(error) = property_delete_error {
            return Err(error);
        }
        let staged_deletes = scope.take_deletes_for_label(self.label);
        let mut delete_error: Option<StorageError> = None;
        for global_id in &staged_deletes {
            if created.contains(global_id) {
                continue;
            }
            let (idx, local_id) = self.decode_id(*global_id);
            let table = self.shards[idx].read();
            if !table.is_row_live_at(local_id, ts) {
                delete_error = Some(StorageError::vertex_not_found());
                break;
            }
        }
        for global_id in staged_deletes {
            let _ = scope.stage_delete(self.label, global_id);
        }
        if let Some(error) = delete_error {
            return Err(error);
        }
        Ok(())
    }

    /// Apply the label's staged inserts grouped by shard. Every row commits
    /// under the global id reserved at staging time, so the reported
    /// mapping echoes the declared ids. Rows applied before a failure are
    /// collected in `applied` for the caller to undo; the failed row and
    /// every later row have their unbound reservations released here.
    fn apply_staged_inserts(
        &self,
        scope: &mut WriteScope,
        ts: Timestamp,
        applied: &mut Vec<(usize, u32)>,
        mapping: &mut Vec<(IdKey, u32)>,
    ) -> StorageResult<()> {
        let staged = scope.take_inserts_for_label(self.label);
        let mut by_shard: Vec<Vec<ShardStagedInsert>> = vec![Vec::new(); self.layout.num_shards];
        let estimate = staged.len().div_ceil(self.layout.num_shards.max(1)).max(1);
        for group in by_shard.iter_mut() {
            group.reserve(estimate);
        }
        for (key, reserved, props) in staged {
            let idx = match &key {
                IdKey::Text(name) => self.shard_index_by_str(name),
                IdKey::Int(n) => self.shard_index_by_i64(*n),
            };
            by_shard[idx].push((key, reserved, props));
        }
        for (shard_idx, group) in by_shard.iter().enumerate() {
            if group.is_empty() {
                continue;
            }
            let table = self.shards[shard_idx].read();
            let mut pos = 0usize;
            let mut failure: Option<StorageError> = None;
            while pos < group.len() {
                let (key, reserved, props) = &group[pos];
                let (_, local_id) = self.decode_id(*reserved);
                if let Err(error) = table.apply_insert(key.clone(), props, ts, Some(local_id)) {
                    failure = Some(error);
                    break;
                }
                applied.push((shard_idx, local_id));
                mapping.push((key.clone(), *reserved));
                pos += 1;
            }
            drop(table);
            if let Some(error) = failure {
                // The failed row's reservation is already settled inside
                // `apply_insert` (released or recycled through its own
                // undo); only rows that never entered the table remain.
                for (_, reserved, _) in &group[pos + 1..] {
                    self.release_reserved_vertex(*reserved);
                }
                for later in &by_shard[shard_idx + 1..] {
                    for (_, reserved, _) in later {
                        self.release_reserved_vertex(*reserved);
                    }
                }
                return Err(error);
            }
        }
        Ok(())
    }

    /// Apply the label's staged updates after the inserts.
    fn apply_staged_updates(
        &self,
        scope: &mut WriteScope,
        ts: Timestamp,
        applied: &mut Vec<(usize, u32, Arc<str>)>,
    ) -> StorageResult<()> {
        let staged = scope.take_updates_for_label(self.label);
        for (global_id, props) in staged {
            let (idx, local_id) = self.decode_id(global_id);
            let table = self.shards[idx].read();
            for (col_name, value) in &props {
                table.update_property(local_id, col_name, value, ts)?;
                applied.push((idx, local_id, col_name.clone()));
            }
        }
        Ok(())
    }

    /// Apply the label's staged column deletions after the updates.
    fn apply_staged_property_deletes(
        &self,
        scope: &mut WriteScope,
        ts: Timestamp,
        applied: &mut Vec<(usize, u32, Arc<str>)>,
    ) -> StorageResult<()> {
        let staged = scope.take_property_deletes_for_label(self.label);
        for (global_id, cols) in staged {
            let (idx, local_id) = self.decode_id(global_id);
            let table = self.shards[idx].read();
            for col_name in &cols {
                table.delete_property(local_id, col_name, ts)?;
                applied.push((idx, local_id, col_name.clone()));
            }
        }
        Ok(())
    }

    /// Apply the label's staged deletes after inserts, updates and column deletions.
    fn apply_staged_deletes(
        &self,
        scope: &mut WriteScope,
        ts: Timestamp,
        applied: &mut Vec<(usize, u32)>,
    ) -> StorageResult<()> {
        let staged = scope.take_deletes_for_label(self.label);
        for global_id in staged {
            let (idx, local_id) = self.decode_id(global_id);
            let table = self.shards[idx].read();
            table.apply_delete(local_id, ts)?;
            applied.push((idx, local_id));
        }
        Ok(())
    }

    fn undo_applied_updates(&self, applied: &[(usize, u32, Arc<str>)], ts: Timestamp) {
        for &(shard_idx, local_id, ref col_name) in applied.iter().rev() {
            let _ = self.shards[shard_idx]
                .read()
                .undo_update(local_id, col_name, ts);
        }
    }

    fn undo_applied_deletes(&self, applied: &[(usize, u32)]) {
        for &(shard_idx, local_id) in applied.iter().rev() {
            self.shards[shard_idx].read().revert_delete(local_id);
        }
    }

    /// Undo rows applied by a failed commit: drop each key and invalidate
    /// its timestamp slot. Column version entries of never-visible rows stay
    /// unreachable until the recycled id reuses the slot.
    fn undo_applied_inserts(&self, applied: &[(usize, u32)]) {
        for &(shard_idx, local_id) in applied {
            self.shards[shard_idx].read().undo_apply_insert(local_id);
        }
    }

    /// Rollback hook for one label table: discard the label's staged rows
    /// and release every reservation they carried. Nothing was applied for
    /// entries still staged, so no global write happens here; rows a
    /// committed apply already installed are removed by the caller through
    /// [`Self::undo_applied_ids`]-equivalent table undo before this call.
    pub fn rollback_write_scope(&self, scope: &mut WriteScope, _ts: Timestamp) {
        for reserved in scope.rollback_label(self.label) {
            self.release_reserved_vertex(reserved);
        }
    }

    /// Undo rows this caller already committed through a previous
    /// [`Self::commit_write_scope_tracked`] apply, addressed by their allocated
    /// global ids. Used by write entries that must fail a whole request
    /// after the table apply succeeded (secondary index maintenance,
    /// mutation recording): each key is dropped and its timestamp slot
    /// invalidated, matching the in-apply failure undo.
    pub fn undo_applied_ids(&self, global_ids: &[u32]) {
        for &global_id in global_ids {
            let (idx, local_id) = self.decode_id(global_id);
            self.shards[idx].read().undo_apply_insert(local_id);
        }
    }
}

#[cfg(test)]
mod tests;
