use super::ShardedVertexTable;
use crate::vertex::{IdKey, PkLookup, WriteScope};
use graphdb_core::types::Timestamp;
use graphdb_core::{StorageError, StorageResult, Value};

type ShardStagedInsert = (IdKey, u32, Vec<(String, Value)>);

impl ShardedVertexTable {
    pub fn insert(
        &self,
        external_id: &str,
        properties: &[(String, Value)],
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
        properties: &[(String, Value)],
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

    pub fn delete_by_internal_id(&self, global_id: u32, ts: Timestamp) -> StorageResult<()> {
        let (idx, local_id) = self.decode_id(global_id);
        let table = self.shards[idx].read();
        table.delete_by_internal_id(local_id, ts)
    }

    pub fn batch_delete(&self, external_ids: &[&str], ts: Timestamp) -> StorageResult<usize> {
        // Route ids to their owning shard and delete each shard's batch under
        // one shared guard: each row's tombstone runs the identity-then-
        // segment point path, so no shard write lock is needed.
        let mut by_shard: Vec<Vec<&str>> = vec![Vec::new(); self.layout.num_shards];
        for id in external_ids {
            by_shard[self.shard_index_by_str(id)].push(id);
        }
        let mut total = 0;
        for (idx, ids) in by_shard.iter().enumerate() {
            if !ids.is_empty() {
                total += self.shards[idx].read().batch_delete(ids, ts)?;
            }
        }
        Ok(total)
    }

    pub fn batch_delete_i64(&self, external_ids: &[i64], ts: Timestamp) -> StorageResult<usize> {
        let mut by_shard: Vec<Vec<i64>> = vec![Vec::new(); self.layout.num_shards];
        for id in external_ids {
            by_shard[self.shard_index_by_i64(*id)].push(*id);
        }
        let mut total = 0;
        for (idx, ids) in by_shard.iter().enumerate() {
            if !ids.is_empty() {
                total += self.shards[idx].read().batch_delete_i64(ids, ts)?;
            }
        }
        Ok(total)
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
        rows: &[(&str, &[(String, Value)])],
        ts: Timestamp,
        sorted: bool,
    ) -> StorageResult<usize> {
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
            let _ = self.batch_delete(&applied, ts);
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
        rows: &[(i64, &[(String, Value)])],
        ts: Timestamp,
        sorted: bool,
    ) -> StorageResult<usize> {
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
            let _ = self.batch_delete_i64(&applied, ts);
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
        rows: &[(&str, &[(String, Value)])],
        ts: Timestamp,
    ) -> Vec<StorageResult<u32>> {
        let mut by_shard: Vec<Vec<usize>> = vec![Vec::new(); self.layout.num_shards];
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
        rows: &[(i64, &[(String, Value)])],
        ts: Timestamp,
    ) -> Vec<StorageResult<u32>> {
        let mut by_shard: Vec<Vec<usize>> = vec![Vec::new(); self.layout.num_shards];
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
        columns: &[(String, Value)],
    ) -> StorageResult<Vec<(String, Value)>> {
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
        properties: &[(String, Value)],
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
        properties: &[(String, Value)],
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
        rows: &[(&str, &[(String, Value)])],
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
        rows: &[(i64, &[(String, Value)])],
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
            vec![(col_name.to_string(), value.clone())],
        )?;
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

    /// Scoped batch delete staging. Every resolvable id is buffered;
    /// unresolvable ids fail per row and are skipped by the caller, matching
    /// the direct batch contract. The commit hook applies the staged set.
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

    /// Commit hook for one label table: applies every staged row of the
    /// label, then drops the label's staging records.
    ///
    /// Application order is fixed: inserts grouped by shard in shard-index
    /// order, then updates, then deletes; one shard guard is held at a time
    /// and never alongside another shard's. Inserts carry their conflict
    /// recheck inside the table apply, so a concurrent commit of the same
    /// key fails exactly one side. A row failure undoes the rows this call
    /// already applied and rejects the whole commit with nothing partially
    /// applied left behind; the failed label's staging records are dropped
    /// together with the undo, so the scope stays consistent for the
    /// caller's rollback hook. Called before the timestamp commit; the WAL
    /// commit entries stay the durability point. Returns the staged key to
    /// allocated global id mapping for the label.
    pub fn commit_write_scope(
        &self,
        scope: &mut WriteScope,
        ts: Timestamp,
    ) -> StorageResult<Vec<(IdKey, u32)>> {
        scope.ensure_same_write_ts(ts)?;
        let mut applied: Vec<(usize, u32)> = Vec::new();
        let mut mapping: Vec<(IdKey, u32)> = Vec::new();
        let result = self.apply_staged_inserts(scope, ts, &mut applied, &mut mapping);
        if let Err(error) = result {
            self.undo_applied_inserts(&applied);
            self.rollback_write_scope(scope, ts);
            return Err(error);
        }
        if let Err(error) = self.apply_staged_updates(scope, ts) {
            self.undo_applied_inserts(&applied);
            self.rollback_write_scope(scope, ts);
            return Err(error);
        }
        if let Err(error) = self.apply_staged_deletes(scope, ts) {
            self.undo_applied_inserts(&applied);
            self.rollback_write_scope(scope, ts);
            return Err(error);
        }
        scope.commit_label(self.label);
        Ok(mapping)
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
    fn apply_staged_updates(&self, scope: &mut WriteScope, ts: Timestamp) -> StorageResult<()> {
        let staged = scope.take_updates_for_label(self.label);
        for (global_id, props) in staged {
            let (idx, local_id) = self.decode_id(global_id);
            let table = self.shards[idx].read();
            for (col_name, value) in &props {
                table.update_property(local_id, col_name, value, ts)?;
            }
        }
        Ok(())
    }

    /// Apply the label's staged deletes after inserts and updates.
    fn apply_staged_deletes(&self, scope: &mut WriteScope, ts: Timestamp) -> StorageResult<()> {
        let staged = scope.take_deletes_for_label(self.label);
        for global_id in staged {
            let (idx, local_id) = self.decode_id(global_id);
            let table = self.shards[idx].read();
            table.apply_delete(local_id, ts)?;
        }
        Ok(())
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
    /// [`Self::commit_write_scope`] apply, addressed by their allocated
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
mod scoped_tests {
    use super::*;
    use crate::types::StoragePropertyDef;
    use crate::vertex::VertexSchema;
    use graphdb_core::DataType;

    fn test_schema() -> VertexSchema {
        VertexSchema {
            label_id: 7,
            label_name: "scoped".to_string(),
            properties: vec![StoragePropertyDef::new(
                "name".to_string(),
                DataType::String,
            )],
            primary_key_index: 0,
            schema_version: 1,
        }
    }

    fn props(name: &str) -> Vec<(String, Value)> {
        vec![("name".to_string(), Value::from(name))]
    }

    #[test]
    fn scoped_insert_is_self_consistent_and_older_snapshots_miss() {
        let table = ShardedVertexTable::with_config(7, "scoped".to_string(), test_schema(), 4);
        let write_ts: Timestamp = 100;
        let mut scope = WriteScope::new(write_ts);
        table
            .insert_with_scope("k1", &props("k1"), write_ts, &mut scope)
            .unwrap();
        // Staged but unapplied: the global area still misses the key.
        assert_eq!(table.lookup_pk("k1", write_ts), PkLookup::Missing);
        let applied = table.commit_write_scope(&mut scope, write_ts).unwrap();
        assert_eq!(applied.len(), 1);
        let global = applied[0].1;
        assert!(table.get_by_internal_id(global, write_ts).is_some());
        assert_eq!(table.lookup_pk("absent", write_ts), PkLookup::Missing);
        assert_eq!(
            table.lookup_pk("k1", write_ts - 1),
            PkLookup::Missing,
            "older snapshots miss committed keys by timestamp ordering"
        );
        assert!(scope.is_empty());
        assert!(table.get_by_internal_id(global, write_ts).is_some());
    }

    #[test]
    fn same_scope_duplicate_fails_without_extra_allocation() {
        let table = ShardedVertexTable::with_config(7, "scoped".to_string(), test_schema(), 4);
        let ts: Timestamp = 100;
        let mut scope = WriteScope::new(ts);
        table
            .insert_with_scope("dup", &props("dup"), ts, &mut scope)
            .unwrap();
        assert!(table
            .insert_with_scope("dup", &props("dup"), ts, &mut scope)
            .is_err());
        table.commit_write_scope(&mut scope, ts).unwrap();
        assert_eq!(table.approximate_total_count(), 1);
    }

    #[test]
    fn cross_scope_conflict_fails_at_commit_and_allocates_once() {
        let table = ShardedVertexTable::with_config(7, "scoped".to_string(), test_schema(), 4);
        let ts: Timestamp = 100;
        let mut first = WriteScope::new(ts);
        let mut second = WriteScope::new(ts);
        table
            .insert_with_scope("hot", &props("hot"), ts, &mut first)
            .unwrap();
        table
            .insert_with_scope("hot", &props("hot"), ts, &mut second)
            .unwrap();
        assert!(table.commit_write_scope(&mut first, ts).is_ok());
        assert!(table.commit_write_scope(&mut second, ts).is_err());
        assert_eq!(table.approximate_total_count(), 1);
        assert!(second.is_empty());
    }

    #[test]
    fn rollback_discards_staged_rows_without_global_writes() {
        let table = ShardedVertexTable::with_config(7, "scoped".to_string(), test_schema(), 4);
        let ts: Timestamp = 100;
        let mut scope = WriteScope::new(ts);
        table
            .insert_with_scope("tmp", &props("tmp"), ts, &mut scope)
            .unwrap();
        table.rollback_write_scope(&mut scope, ts);
        assert!(scope.is_empty());
        assert_eq!(table.approximate_total_count(), 0);
        assert_eq!(table.lookup_pk("tmp", ts), PkLookup::Missing);
    }

    #[test]
    fn reserved_ids_recycle_through_release_and_bind_at_commit() {
        let table = ShardedVertexTable::with_config(7, "scoped".to_string(), test_schema(), 4);
        let ts: Timestamp = 100;
        let key = IdKey::Text("x".to_string());
        let first = table.reserve_vertex_id(&key).unwrap();
        table.release_reserved_vertex(first);
        let second = table.reserve_vertex_id(&key).unwrap();
        assert_eq!(second, first, "released reservation is reused");
        let mut scope = WriteScope::new(ts);
        scope
            .stage_insert(7, key.clone(), second, props("x"))
            .unwrap();
        let applied = table.commit_write_scope(&mut scope, ts).unwrap();
        assert_eq!(applied, vec![(key.clone(), second)]);
        assert_eq!(table.get_internal_id("x", ts), Some(second));
        // Releasing a bound id is a no-op: the live row keeps its slot and
        // re-reserving a bound key reports the existing id.
        table.release_reserved_vertex(second);
        assert_eq!(table.reserve_vertex_id(&key).unwrap(), second);
    }

    #[test]
    fn over_limit_scoped_write_is_rejected_before_global_mutation() {
        let table = ShardedVertexTable::with_config(7, "scoped".to_string(), test_schema(), 4);
        let ts: Timestamp = 100;
        let mut scope = WriteScope::new(ts);
        for index in 0..crate::vertex::MAX_WRITE_SCOPE_KEYS {
            scope
                .stage_insert(7, IdKey::Int(index as i64), index as u32, props("x"))
                .expect("prefill within capacity");
        }
        assert!(table
            .insert_with_scope("overflow", &props("overflow"), ts, &mut scope)
            .is_err());
        assert_eq!(table.approximate_total_count(), 0);
    }

    #[test]
    fn concurrent_scoped_same_key_inserts_allocate_once() {
        use std::sync::Arc;
        let table = Arc::new(ShardedVertexTable::with_config(
            7,
            "scoped".to_string(),
            test_schema(),
            8,
        ));
        let ts: Timestamp = 100;
        let mut handles = Vec::new();
        for _ in 0..8 {
            let table = Arc::clone(&table);
            handles.push(std::thread::spawn(move || {
                let mut scope = WriteScope::new(ts);
                table
                    .insert_with_scope("race", &props("race"), ts, &mut scope)
                    .expect("staging never touches global state");
                table.commit_write_scope(&mut scope, ts).map(|_| ())
            }));
        }
        let mut oks = 0usize;
        for handle in handles {
            if handle.join().unwrap().is_ok() {
                oks += 1;
            }
        }
        assert_eq!(oks, 1);
        assert_eq!(table.approximate_total_count(), 1);
    }

    #[test]
    fn scoped_batch_same_key_duplicate_fails_without_extra_allocation() {
        let table = ShardedVertexTable::with_config(7, "scoped".to_string(), test_schema(), 4);
        let ts: Timestamp = 100;
        let mut scope = WriteScope::new(ts);
        let holder = props("dup");
        let rows: Vec<(&str, &[(String, Value)])> =
            vec![("dup", holder.as_slice()), ("dup", holder.as_slice())];
        let results = table.insert_batch_str_with_scope(&rows, ts, &mut scope);
        assert_eq!(results.len(), 2);
        let oks = results.iter().filter(|r| r.is_ok()).count();
        assert_eq!(oks, 1);
        let applied = table.commit_write_scope(&mut scope, ts).unwrap();
        assert_eq!(applied.len(), 1);
        assert_eq!(table.approximate_total_count(), 1);
        assert!(scope.is_empty());
    }

    #[test]
    fn scoped_batch_over_limit_rejected_before_global_mutation() {
        let table = ShardedVertexTable::with_config(7, "scoped".to_string(), test_schema(), 4);
        let ts: Timestamp = 100;
        let mut scope = WriteScope::new(ts);
        for index in 0..crate::vertex::MAX_WRITE_SCOPE_KEYS {
            scope
                .stage_insert(7, IdKey::Int(index as i64), index as u32, props("x"))
                .expect("prefill within capacity");
        }
        let holder = props("overflow");
        let rows: Vec<(&str, &[(String, Value)])> = vec![("overflow", holder.as_slice())];
        let results = table.insert_batch_str_with_scope(&rows, ts, &mut scope);
        assert!(results[0].is_err());
        assert_eq!(table.approximate_total_count(), 0);
    }

    #[test]
    fn scoped_batch_i64_same_key_duplicate_applies_once() {
        let table = ShardedVertexTable::with_config(7, "scoped".to_string(), test_schema(), 4);
        let ts: Timestamp = 100;
        let mut scope = WriteScope::new(ts);
        let holder = props("42");
        let rows: Vec<(i64, &[(String, Value)])> =
            vec![(42, holder.as_slice()), (42, holder.as_slice())];
        let results = table.insert_batch_i64_with_scope(&rows, ts, &mut scope);
        assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
        let applied = table.commit_write_scope(&mut scope, ts).unwrap();
        assert_eq!(applied.len(), 1);
        assert_eq!(table.approximate_total_count(), 1);
        assert!(scope.is_empty());
    }

    #[test]
    fn scoped_update_and_delete_apply_at_commit() {
        use crate::types::StoragePropertyDef;
        use graphdb_core::DataType;
        let schema = VertexSchema {
            label_id: 7,
            label_name: "scoped".to_string(),
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
        };
        let table = ShardedVertexTable::with_config(7, "scoped".to_string(), schema, 4);
        let ts: Timestamp = 100;
        let mut scope = WriteScope::new(ts);
        table
            .insert_with_scope(
                "row",
                &[("age".to_string(), Value::from(1))],
                ts,
                &mut scope,
            )
            .unwrap();
        table.commit_write_scope(&mut scope, ts).unwrap();
        let global = table.get_internal_id("row", ts).expect("applied");

        let mut scope = WriteScope::new(ts + 1);
        table
            .update_property_with_scope(global, "age", &Value::from(2), ts + 1, &mut scope)
            .unwrap();
        // Staged update is invisible until the commit apply.
        assert_eq!(
            table
                .get_by_internal_id(global, ts + 1)
                .expect("row")
                .properties,
            vec![
                ("name".to_string(), Value::from("row")),
                ("age".to_string(), Value::from(1)),
            ]
        );
        table.commit_write_scope(&mut scope, ts + 1).unwrap();
        assert_eq!(
            table
                .get_by_internal_id(global, ts + 1)
                .expect("row")
                .properties,
            vec![
                ("name".to_string(), Value::from("row")),
                ("age".to_string(), Value::from(2)),
            ]
        );

        let mut scope = WriteScope::new(ts + 2);
        table.delete_with_scope("row", ts + 2, &mut scope).unwrap();
        assert!(table.get_by_internal_id(global, ts + 2).is_some());
        table.commit_write_scope(&mut scope, ts + 2).unwrap();
        assert!(table.get_by_internal_id(global, ts + 2).is_none());
    }

    #[test]
    fn failed_commit_leaves_no_partial_application() {
        let table = ShardedVertexTable::with_config(7, "scoped".to_string(), test_schema(), 4);
        let ts: Timestamp = 100;
        // Seed a conflicting row directly.
        table.insert("taken", &props("taken"), ts).unwrap();
        let mut scope = WriteScope::new(ts);
        table
            .insert_with_scope("fresh", &props("fresh"), ts, &mut scope)
            .unwrap();
        table
            .insert_with_scope("taken", &props("taken"), ts, &mut scope)
            .unwrap();
        // Staging accepts both; the commit recheck rejects the taken key and
        // undoes the fresh row applied ahead of it in shard order.
        assert!(table.commit_write_scope(&mut scope, ts).is_err());
        assert_eq!(table.lookup_pk("fresh", ts), PkLookup::Missing);
        assert_eq!(table.approximate_total_count(), 1);
    }

    #[test]
    fn scoped_insert_rejects_cross_timestamp_reuse() {
        let table = ShardedVertexTable::with_config(7, "scoped".to_string(), test_schema(), 4);
        let mut scope = WriteScope::new(100);
        assert!(table
            .insert_with_scope("k1", &props("k1"), 101, &mut scope)
            .is_err());
        assert_eq!(table.approximate_total_count(), 0);
        assert!(scope.is_empty());
    }

    #[test]
    fn bulk_import_empty_sorted_matches_batched_rows() {
        let table = ShardedVertexTable::with_config(7, "scoped".to_string(), test_schema(), 4);
        let ts: Timestamp = 100;
        let names: Vec<String> = (0..10).map(|i| format!("s_{:02}", i)).collect();
        let holders: Vec<Vec<(String, Value)>> = names.iter().map(|n| props(n)).collect();
        let rows: Vec<(&str, &[(String, Value)])> = names
            .iter()
            .zip(holders.iter())
            .map(|(n, p)| (n.as_str(), p.as_slice()))
            .collect();
        let count = table
            .bulk_import_str(&rows, ts, true)
            .expect("sorted import");
        assert_eq!(count, 10);
        assert_eq!(table.approximate_total_count(), 10);
        for name in &names {
            assert!(table.get_internal_id(name, ts).is_some());
        }
    }

    #[test]
    fn bulk_import_rejects_nonempty_and_false_sorted_declaration() {
        let table = ShardedVertexTable::with_config(7, "scoped".to_string(), test_schema(), 4);
        let ts: Timestamp = 100;
        let holder = props("b");
        let rows: Vec<(&str, &[(String, Value)])> = vec![("b", holder.as_slice())];
        assert!(table.bulk_import_str(&rows, ts, true).is_ok());
        let holder_b = props("c");
        let rows_b: Vec<(&str, &[(String, Value)])> = vec![("c", holder_b.as_slice())];
        assert!(table.bulk_import_str(&rows_b, ts, false).is_err());
        let unsorted_table =
            ShardedVertexTable::with_config(7, "scoped".to_string(), test_schema(), 4);
        let ha = props("z");
        let hb = props("a");
        let unsorted: Vec<(&str, &[(String, Value)])> =
            vec![("z", ha.as_slice()), ("a", hb.as_slice())];
        assert!(unsorted_table.bulk_import_str(&unsorted, ts, true).is_err());
        assert_eq!(unsorted_table.approximate_total_count(), 0);
    }
}
