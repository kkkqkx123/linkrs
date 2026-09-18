//! Mutating paths: staging commit, insert, delete and undo.

use super::super::super::MutableCsrTrait;
use super::super::config::UpdateEdgePropertyByKeyParams;
use super::super::staging::EdgeStagingBatch;
use super::EdgeStore;
use crate::types::PropertyId;
use graphdb_core::types::{EdgeId, Timestamp};
use graphdb_core::{StorageError, StorageResult, Value};

impl EdgeStore {
    pub fn insert_edge(
        &mut self,
        src: u32,
        dst: u32,
        rank: i64,
        property_values: &[(String, Value)],
        ts: Timestamp,
    ) -> StorageResult<()> {
        if !self.is_open {
            return Err(StorageError::storage_not_open());
        }

        if self.schema.oe_strategy == super::super::super::EdgeStrategy::None {
            return Err(StorageError::invalid_operation(
                "Cannot insert edge: out-edge strategy is None".to_string(),
            ));
        }

        // Single-entry staging commit: the batch owns the whole multi-step
        // write, so failure handling lives in one place below instead of in
        // per-step compensation branches here.
        let mut batch = EdgeStagingBatch::new();
        batch.stage_insert(src, dst, rank, property_values, ts);
        self.commit_staging_batch(batch).map(|_| ())
    }

    /// Create an empty staging batch for one atomic group of edge writes.
    pub fn staging_batch() -> EdgeStagingBatch {
        EdgeStagingBatch::new()
    }

    /// Commit one staging batch atomically.
    ///
    /// Entries apply in stage order with batch prefix effects: a later entry
    /// observes earlier entries of the same batch. An insert cancelled by a
    /// later delete of the same key leaves no tombstone and no visible edge;
    /// a delete followed by an insert of the same key rebuilds. Staged writes
    /// stay invisible to every read until commit. Prevalidation failures leave
    /// committed state untouched. When a late failure still occurs, only the
    /// entries this batch already applied are rolled back; committed data from
    /// other batches is never touched. Dropping a batch without committing
    /// discards it with no residue. Cancelled inserts consume at most the
    /// monotonic edge-id counter, never visible state.
    ///
    /// Returns the number of net applied entries (inserts plus deletes,
    /// excluding batch-cancelled pairs).
    pub fn commit_staging_batch(&mut self, mut batch: EdgeStagingBatch) -> StorageResult<usize> {
        if !self.is_open {
            return Err(StorageError::storage_not_open());
        }
        if batch.insert_count() > 0
            && self.schema.oe_strategy == super::super::super::EdgeStrategy::None
        {
            return Err(StorageError::invalid_operation(
                "Cannot insert edge: out-edge strategy is None".to_string(),
            ));
        }
        self.prevalidate_staging_batch(&batch)?;
        if let Some(dir) = self.wal_dir.clone() {
            let mut ops = Vec::with_capacity(batch.len());
            for ord in batch.ordered() {
                if ord.is_insert {
                    let ins = &batch.staged_inserts()[ord.slot];
                    ops.push(super::super::wal::EdgeWalOp::Insert {
                        src: ins.src,
                        dst: ins.dst,
                        rank: ins.rank,
                        properties: ins.properties.clone(),
                        create_ts: ins.create_ts,
                    });
                } else {
                    let del = &batch.staged_deletes()[ord.slot];
                    ops.push(super::super::wal::EdgeWalOp::Delete {
                        src: del.src,
                        dst: del.dst,
                        rank: del.rank,
                        delete_ts: del.delete_ts,
                    });
                }
            }
            super::super::wal::append_ops(&dir, &ops)?;
        }
        let max_ts = batch.max_timestamp();
        let inserts = batch.take_inserts();
        let deletes = batch.take_deletes();
        let order = batch.take_order();
        if order.is_empty() {
            return Ok(0);
        }

        // Recycled working buffers: cleared, not reallocated, and handed
        // back on every exit path below so the next commit reuses them.
        let mut scratch = self.commit_scratch.take();
        scratch.reset(inserts.len(), deletes.len());
        let applied_inserts = &mut scratch.applied_inserts;
        let insert_by_key = &mut scratch.insert_by_key;
        let applied_deletes = &mut scratch.applied_deletes;
        for ord in &order {
            if ord.is_insert {
                let ins = &inserts[ord.slot];
                match self.apply_staged_insert(
                    ins.src,
                    ins.dst,
                    ins.rank,
                    &ins.properties,
                    ins.create_ts,
                ) {
                    Ok(edge_id) => {
                        let entry = (ins.src, ins.dst, ins.rank, edge_id, ins.create_ts);
                        applied_inserts.push(entry);
                        insert_by_key.insert((ins.src, ins.dst, ins.rank), entry);
                    }
                    Err(e) => {
                        for (src, dst, _rank, edge_id, ts) in applied_deletes.drain(..) {
                            self.revert_applied_delete(src, dst, edge_id, ts);
                        }
                        for (src, dst, rank, edge_id, ts) in applied_inserts.drain(..) {
                            self.erase_applied_insert(src, dst, rank, edge_id, ts);
                        }
                        self.commit_scratch = scratch;
                        return Err(e);
                    }
                }
            } else {
                let del = &deletes[ord.slot];
                let key = (del.src, del.dst, del.rank);
                if let Some((src, dst, rank, edge_id, ts)) = insert_by_key.remove(&key) {
                    self.erase_applied_insert(src, dst, rank, edge_id, ts);
                    applied_inserts.retain(|(_, _, _, eid, _)| *eid != edge_id);
                    continue;
                }
                match self.apply_staged_delete(del.src, del.dst, del.rank, del.delete_ts) {
                    Ok(Some(edge_id)) => {
                        applied_deletes.push((del.src, del.dst, del.rank, edge_id, del.delete_ts));
                    }
                    Ok(None) => {}
                    Err(e) => {
                        for (src, dst, _rank, edge_id, ts) in applied_deletes.drain(..) {
                            self.revert_applied_delete(src, dst, edge_id, ts);
                        }
                        for (src, dst, rank, edge_id, ts) in applied_inserts.drain(..) {
                            self.erase_applied_insert(src, dst, rank, edge_id, ts);
                        }
                        self.commit_scratch = scratch;
                        return Err(e);
                    }
                }
            }
        }

        let applied = applied_inserts.len() + applied_deletes.len();
        if applied > 0 {
            // Backpressure is observed, not dropped: an over-limit commit
            // warns and the maintenance pass below runs synchronously.
            let mut pressured = false;
            if let Some(ts) = max_ts {
                pressured = self.check_and_apply_write_backpressure(ts);
            }
            self.maybe_run_auto_maintenance();
            if pressured {
                log::warn!(
                    "edge table '{}' over mutable CSR budget, synchronous maintenance ran",
                    self.label_name
                );
            }
            for (_, _, _, edge_id, _) in applied_inserts.iter().chain(applied_deletes.iter()) {
                self.debug_assert_copies_consistent(*edge_id);
            }
        }
        self.commit_scratch = scratch;
        Ok(applied)
    }

    fn convert_property_values(
        &self,
        property_values: &[(String, Value)],
    ) -> StorageResult<Vec<(String, Value)>> {
        let mut converted_values: Vec<(String, Value)> = Vec::with_capacity(property_values.len());
        for (name, value) in property_values {
            let prop_idx = self
                .property_index_cache
                .get(name)
                .ok_or_else(|| StorageError::column_not_found(name.clone()))?;
            let prop_def = &self.schema.properties[*prop_idx];

            if value.data_type() != prop_def.data_type {
                let converted = value.try_cast_to(&prop_def.data_type)?;
                converted_values.push((name.clone(), converted));
            } else {
                converted_values.push((name.clone(), value.clone()));
            }
        }
        Ok(converted_values)
    }

    fn prevalidate_staging_batch(&self, batch: &EdgeStagingBatch) -> StorageResult<()> {
        use std::collections::HashSet;
        let mut seen_inserts: HashSet<(u32, u32, i64)> = HashSet::new();
        let mut seen_deletes: HashSet<(u32, u32, i64)> = HashSet::new();
        let mut seen_single_src: HashSet<u32> = HashSet::new();
        let mut seen_single_dst: HashSet<u32> = HashSet::new();
        let single_out = self.schema.oe_strategy == super::super::super::EdgeStrategy::Single;
        let single_in = self.schema.ie_strategy == super::super::super::EdgeStrategy::Single;
        for ord in batch.ordered() {
            if ord.is_insert {
                let ins = &batch.staged_inserts()[ord.slot];
                for (name, _) in &ins.properties {
                    if !self.property_index_cache.contains_key(name) {
                        return Err(StorageError::column_not_found(name.clone()));
                    }
                }
                let key = (ins.src, ins.dst, ins.rank);
                if seen_inserts.contains(&key) {
                    return Err(StorageError::edge_already_exists(format!(
                        "{} -> {}@{}",
                        ins.src, ins.dst, ins.rank
                    )));
                }
                if single_out && seen_single_src.contains(&ins.src) {
                    return Err(StorageError::conflict(format!(
                        "Single out-edge strategy already holds a live edge for src={}",
                        ins.src
                    )));
                }
                if single_in && seen_single_dst.contains(&ins.dst) {
                    return Err(StorageError::conflict(format!(
                        "Single in-edge strategy already holds a live edge for dst={}",
                        ins.dst
                    )));
                }
                if single_out {
                    let live = self.merged_edges_of(&self.out_csr, ins.src, ins.create_ts);
                    let covered = !live.is_empty()
                        && live
                            .iter()
                            .all(|nbr| seen_deletes.contains(&(ins.src, nbr.endpoint, nbr.rank)));
                    if !live.is_empty() && !covered {
                        return Err(StorageError::conflict(format!(
                            "Single out-edge strategy already holds a live edge for src={}",
                            ins.src
                        )));
                    }
                }
                if single_in {
                    let live = self.merged_edges_of(&self.in_csr, ins.dst, ins.create_ts);
                    let covered = !live.is_empty()
                        && live
                            .iter()
                            .all(|nbr| seen_deletes.contains(&(nbr.endpoint, ins.dst, nbr.rank)));
                    if !live.is_empty() && !covered {
                        return Err(StorageError::conflict(format!(
                            "Single in-edge strategy already holds a live edge for dst={}",
                            ins.dst
                        )));
                    }
                }
                if !seen_deletes.contains(&key)
                    && self.has_edge(ins.src, ins.dst, ins.rank, ins.create_ts)
                {
                    return Err(StorageError::edge_already_exists(format!(
                        "{} -> {}@{}",
                        ins.src, ins.dst, ins.rank
                    )));
                }
                if seen_deletes.contains(&key) {
                    seen_deletes.remove(&key);
                }
                seen_inserts.insert(key);
                if single_out {
                    seen_single_src.insert(ins.src);
                }
                if single_in {
                    seen_single_dst.insert(ins.dst);
                }
            } else {
                let del = &batch.staged_deletes()[ord.slot];
                let key = (del.src, del.dst, del.rank);
                if seen_inserts.remove(&key) {
                    if single_out && !seen_inserts.iter().any(|(s, _, _)| *s == del.src) {
                        seen_single_src.remove(&del.src);
                    }
                    if single_in && !seen_inserts.iter().any(|(_, d, _)| *d == del.dst) {
                        seen_single_dst.remove(&del.dst);
                    }
                } else {
                    seen_deletes.insert(key);
                }
            }
        }
        Ok(())
    }

    /// Move one staged insert into the committed structures.
    ///
    /// Self-contained: a failure cleans up only this entry, so the batch
    /// rollback above only handles entries that fully applied.
    fn apply_staged_insert(
        &mut self,
        src: u32,
        dst: u32,
        rank: i64,
        property_values: &[(String, Value)],
        ts: Timestamp,
    ) -> StorageResult<EdgeId> {
        let converted_values = self.convert_property_values(property_values)?;
        let edge_id = self.next_edge_id.fetch_add();

        // No existence re-check here by design. The batch prevalidation is
        // the single main existence check: it already rejected duplicate keys
        // and occupied Single slots for this batch. The topology insert
        // below is the light recheck: its live-set/slot guard rejects the
        // same conflicts in O(1) without another row scan, and the failure
        // path underneath cleans up the record staged above.

        self.mvcc.record_creation(edge_id, ts);

        if let Err(e) = self
            .properties
            .insert_for_edge(edge_id, &converted_values, ts)
        {
            self.mvcc.remove_edge_timestamps(edge_id);
            return Err(e);
        }

        let dst_key = Self::edge_endpoint_key(dst, rank);
        let src_key = Self::edge_endpoint_key(src, rank);
        if let Err(e) = self.out_csr.insert_edge(src, dst_key, edge_id, ts) {
            if let Some(row) = self.properties.remove_edge_mapping(edge_id) {
                self.properties.release_row(row);
            }
            self.mvcc.remove_edge_timestamps(edge_id);
            self.mark_properties_dirty();
            self.debug_assert_copies_consistent(edge_id);
            return Err(e);
        }

        if let Err(e) = self.in_csr.insert_edge(dst, src_key, edge_id, ts) {
            // Roll back the out-direction insertion physically so no
            // tombstone residue remains; fall back to logical deletion if
            // the entry cannot be located.
            if !self.out_csr.remove_edge(src, edge_id) {
                let _ = self.out_csr.delete_edge(src, edge_id, ts);
            }
            if let Some(row) = self.properties.remove_edge_mapping(edge_id) {
                self.properties.release_row(row);
            }
            let _ = self.properties.mark_deleted(edge_id, ts);
            self.mvcc.remove_edge_timestamps(edge_id);
            self.mark_properties_dirty();
            self.debug_assert_copies_consistent(edge_id);
            return Err(e);
        }

        if self.property_index.is_some() {
            let label = self.label;
            let outcomes: Vec<(String, StorageResult<()>, u64)> = if let Some(ref mut index) =
                self.property_index
            {
                converted_values
                    .iter()
                    .map(|(prop_name, prop_value)| {
                        let started = std::time::Instant::now();
                        let result = index.insert(prop_name, prop_value, src, dst, rank, label, ts);
                        let latency = started.elapsed().as_millis() as u64;
                        (prop_name.clone(), result, latency)
                    })
                    .collect()
            } else {
                Vec::new()
            };
            for (prop_name, result, latency) in outcomes {
                self.note_index_result(&prop_name, result, latency);
            }
        }

        self.mark_properties_dirty();
        self.edge_owner
            .insert(edge_id, self.owner_gid_for(src, dst));
        self.debug_assert_copies_consistent(edge_id);
        Ok(edge_id)
    }

    /// Move one staged delete into the committed structures.
    ///
    /// Full-match endpoint semantics: one call deletes every live match for
    /// the endpoint key and reports the deleted count for rollback
    /// reconciliation. Returns the deleted edge id, or `None` when no edge
    /// matched.
    fn apply_staged_delete(
        &mut self,
        src: u32,
        dst: u32,
        rank: i64,
        ts: Timestamp,
    ) -> StorageResult<Option<EdgeId>> {
        let dst_key = Self::edge_endpoint_key(dst, rank);
        let src_key = Self::edge_endpoint_key(src, rank);

        let edge_properties = if self.property_index.is_some() {
            self.get_edge(src, dst, rank, ts).map(|e| e.properties)
        } else {
            None
        };

        if let Some(nbr) = self.merged_get_edge(&self.out_csr, src, dst_key, ts) {
            let edge_id = nbr.edge_id;

            if !self.out_csr.delete_edge(src, edge_id, ts)? {
                return Ok(None);
            }
            let in_deleted = self.in_csr.delete_edge_by_dst(dst, src_key, ts);
            if in_deleted == 0 {
                // Roll back the out-direction deletion to keep both sides
                // consistent. Count reconciliation: expected exactly one
                // in-direction match for the out edge just deleted.
                if !self.out_csr.revert_delete_by_edge_id(src, edge_id, ts) {
                    return Err(StorageError::invalid_operation(format!(
                        "delete rollback failed for edge {:?}: out-direction revert missed",
                        edge_id
                    )));
                }
                return Ok(None);
            }
            if in_deleted > 1 {
                log::debug!(
                    "apply_staged_delete multi-match: ({}, {}, {}) in_deleted={}",
                    src,
                    dst,
                    rank,
                    in_deleted
                );
            }

            self.mvcc.record_edge_deletion(edge_id, ts);
            let _ = self.properties.mark_deleted(edge_id, ts);
            self.update_property_index_on_delete(&edge_properties, src, dst, rank, ts);
            self.mark_properties_dirty();
            self.debug_assert_copies_consistent(edge_id);
            return Ok(Some(edge_id));
        }

        // Missed the merged read: distinguish absent edges from conflicting
        // re-deletes. Scan every physical generation sharing the endpoint
        // key and check each against the authority, so a stale tombstone
        // generation never masks the visible generation.
        let mut candidates: Vec<EdgeId> = Vec::new();
        self.out_csr.visit_physical(src, |nbr| {
            if nbr.to_vertex_id() == dst_key {
                candidates.push(nbr.edge_id);
            }
            true
        });
        for candidate in candidates {
            if let Some(info) = self.mvcc.edge_timestamps.get(&candidate) {
                if info.delete_ts != Timestamp::MAX && info.delete_ts != ts {
                    return Err(StorageError::write_write_conflict(format!(
                        "edge {:?} already deleted at ts={}, attempted delete at ts={}",
                        candidate, info.delete_ts, ts
                    )));
                }
            }
        }

        Ok(None)
    }

    /// Erase one batch-applied insert during batch rollback.
    ///
    /// Physical removal across all copies; tolerates absence so rollback
    /// stays total even under partial application.
    fn erase_applied_insert(
        &mut self,
        src: u32,
        dst: u32,
        rank: i64,
        edge_id: EdgeId,
        ts: Timestamp,
    ) {
        let properties = self
            .properties
            .read_properties_by_edge_id(edge_id)
            .unwrap_or_default();
        self.out_csr.remove_edge(src, edge_id);
        self.in_csr.remove_edge(dst, edge_id);
        if let Some(row) = self.properties.remove_edge_mapping(edge_id) {
            self.properties.release_row(row);
        }
        self.mvcc.remove_edge_timestamps(edge_id);
        self.edge_owner.remove(&edge_id);
        if self.property_index.is_some() {
            let outcomes: Vec<(String, StorageResult<()>, u64)> =
                if let Some(ref mut index) = self.property_index {
                    properties
                        .iter()
                        .map(|(prop_name, prop_value)| {
                            let started = std::time::Instant::now();
                            let result = index.delete(prop_name, prop_value, src, dst, rank, ts);
                            let latency = started.elapsed().as_millis() as u64;
                            (prop_name.clone(), result, latency)
                        })
                        .collect()
                } else {
                    Vec::new()
                };
            for (prop_name, result, latency) in outcomes {
                self.note_index_result(&prop_name, result, latency);
            }
        }
        self.mark_properties_dirty();
        self.debug_assert_copies_consistent(edge_id);
    }

    /// Shared gate for authority revival on delete rollback.
    ///
    /// Authority may only return to live when both directions physically
    /// reverted. A partial revert keeps the authority deletion mark so a
    /// future timestamp tombstone can never coexist with a live authority
    /// record.
    #[inline]
    fn fully_reverted(out_ok: bool, in_ok: bool) -> bool {
        out_ok && in_ok
    }

    /// Revert one batch-applied delete during batch rollback.
    fn revert_applied_delete(&mut self, src: u32, dst: u32, edge_id: EdgeId, ts: Timestamp) {
        let out_ok = self.out_csr.revert_delete_by_edge_id(src, edge_id, ts);
        let in_ok = self.in_csr.revert_delete_by_edge_id(dst, edge_id, ts);
        if !Self::fully_reverted(out_ok, in_ok) {
            return;
        }
        if let Some(ts_info) = self.mvcc.edge_timestamps.get_mut(&edge_id) {
            ts_info.delete_ts = Timestamp::MAX;
        }
        let _ = self.properties.revert_deletion_for_edge(edge_id);
        self.mark_properties_dirty();
        self.debug_assert_copies_consistent(edge_id);
    }

    pub fn delete_edge(
        &mut self,
        src: u32,
        dst: u32,
        rank: i64,
        ts: Timestamp,
    ) -> StorageResult<bool> {
        if !self.is_open {
            return Err(StorageError::storage_not_open());
        }

        // Single-entry staging commit: the batch owns the two-direction
        // write, so the out/in rollback lives in one place.
        let mut batch = EdgeStagingBatch::new();
        batch.stage_delete(src, dst, rank, ts);
        Ok(self.commit_staging_batch(batch)? > 0)
    }

    /// Physically erase an edge inserted by an uncommitted transaction.
    ///
    /// Insert-undo path: unlike a user delete (logical deletion through
    /// `delete_edge`), aborting an insert must leave no trace in any of the
    /// three copies — otherwise the aborted edge would stay visible inside
    /// its snapshot window and leave a permanent tombstone. Every step
    /// tolerates absence, so replaying the undo (abort re-drive, WAL
    /// recovery re-application) is idempotent.
    pub fn erase_edge(&mut self, src: u32, dst: u32, rank: i64, ts: Timestamp) -> bool {
        let Some(edge_id) = self.edge_id_of(src, dst, rank, ts) else {
            return false;
        };
        let properties = self
            .properties
            .read_properties_by_edge_id(edge_id)
            .unwrap_or_default();
        self.out_csr.remove_edge(src, edge_id);
        self.in_csr.remove_edge(dst, edge_id);
        if let Some(row) = self.properties.remove_edge_mapping(edge_id) {
            self.properties.release_row(row);
        }
        self.mvcc.remove_edge_timestamps(edge_id);
        self.edge_owner.remove(&edge_id);
        if self.property_index.is_some() {
            let outcomes: Vec<(String, StorageResult<()>, u64)> =
                if let Some(ref mut index) = self.property_index {
                    properties
                        .iter()
                        .map(|(prop_name, prop_value)| {
                            let started = std::time::Instant::now();
                            let result = index.delete(prop_name, prop_value, src, dst, rank, ts);
                            let latency = started.elapsed().as_millis() as u64;
                            (prop_name.clone(), result, latency)
                        })
                        .collect()
                } else {
                    Vec::new()
                };
            for (prop_name, result, latency) in outcomes {
                self.note_index_result(&prop_name, result, latency);
            }
        }
        self.debug_assert_copies_consistent(edge_id);
        true
    }

    /// Debug-only cross-copy consistency check for one edge.
    ///
    /// Release builds skip the whole body (zero overhead): every arm is a
    /// `debug_assert`. A property row mapping must never outlive its
    /// authority entry (orphan row). Called on insert success, delete
    /// success and delete-rollback success.
    fn debug_assert_copies_consistent(&self, edge_id: EdgeId) {
        debug_assert!(
            self.properties.get_row_for_edge(edge_id).is_none()
                || self.mvcc.edge_timestamps.contains_key(&edge_id),
            "property row mapping without authority entry"
        );
    }

    /// Revert a deletion by edge key without offsets.
    ///
    /// Undo path for transaction rollback: scans every physical generation
    /// sharing the endpoint key, verifies this undo owns the deletion
    /// through the authority, then reverts both directions by edge id.
    /// Authority revival requires both directions to revert; a partial
    /// revert keeps the deletion mark and reports an error.
    pub fn revert_delete_edge(
        &mut self,
        src: u32,
        dst: u32,
        rank: i64,
        ts: Timestamp,
    ) -> StorageResult<bool> {
        if !self.is_open {
            return Err(StorageError::storage_not_open());
        }
        let dst_key = Self::edge_endpoint_key(dst, rank);
        let mut candidates: Vec<EdgeId> = Vec::new();
        self.out_csr.visit_physical(src, |nbr| {
            if nbr.to_vertex_id() == dst_key {
                candidates.push(nbr.edge_id);
            }
            true
        });
        let mut edge_id = None;
        for candidate in candidates {
            match self.mvcc.edge_timestamps.get(&candidate) {
                Some(info) if info.delete_ts != Timestamp::MAX && info.delete_ts <= ts => {
                    edge_id = Some(candidate);
                    break;
                }
                _ => continue,
            }
        }
        let Some(edge_id) = edge_id else {
            return Ok(false);
        };
        if !self.out_csr.revert_delete_by_edge_id(src, edge_id, ts) {
            return Ok(false);
        }
        if !self.in_csr.revert_delete_by_edge_id(dst, edge_id, ts) {
            return Err(StorageError::invalid_operation(format!(
                "delete rollback failed for edge {:?}: in-direction revert missed",
                edge_id
            )));
        }
        if let Some(ts_info) = self.mvcc.edge_timestamps.get_mut(&edge_id) {
            ts_info.delete_ts = Timestamp::MAX;
        }
        let _ = self.properties.revert_deletion_for_edge(edge_id);
        let restored = self.properties_for_edge(edge_id, ts);
        if self.property_index.is_some() {
            let label = self.label;
            let outcomes: Vec<(String, StorageResult<()>, u64)> =
                if let Some(ref mut index) = self.property_index {
                    restored
                        .into_iter()
                        .map(|(prop_name, prop_value)| {
                            let started = std::time::Instant::now();
                            let result =
                                index.insert(&prop_name, &prop_value, src, dst, rank, label, ts);
                            let latency = started.elapsed().as_millis() as u64;
                            (prop_name, result, latency)
                        })
                        .collect()
                } else {
                    Vec::new()
                };
            for (prop_name, result, latency) in outcomes {
                self.note_index_result(&prop_name, result, latency);
            }
        }
        self.mark_properties_dirty();
        self.debug_assert_copies_consistent(edge_id);
        Ok(true)
    }

    pub fn update_edge_property(
        &mut self,
        src: u32,
        dst: u32,
        rank: i64,
        prop_name: &str,
        value: &Value,
        ts: Timestamp,
    ) -> StorageResult<bool> {
        if !self.is_open {
            return Err(StorageError::storage_not_open());
        }

        // Validate property exists via cache
        let _ = self
            .property_index_cache
            .get(prop_name)
            .ok_or_else(|| StorageError::column_not_found(prop_name.to_string()))?;

        let dst_key = Self::edge_endpoint_key(dst, rank);
        if let Some(nbr) = self.merged_get_edge(&self.out_csr, src, dst_key, ts) {
            if let Some(dir) = self.wal_dir.clone() {
                super::super::wal::append_ops(
                    &dir,
                    &[super::super::wal::EdgeWalOp::PropertyUpdate {
                        src,
                        dst,
                        rank,
                        prop_name: prop_name.to_string(),
                        value: value.clone(),
                        ts,
                    }],
                )?;
            }
            self.properties
                .set_property_for_edge(nbr.edge_id, prop_name, Some(value.clone()), ts)
                .map_err(|_| StorageError::column_not_found(prop_name.to_string()))?;
            self.mark_properties_dirty_for_edge(src, dst);
            self.maybe_run_auto_maintenance();
            return Ok(true);
        }

        Ok(false)
    }

    pub fn update_edge_property_by_key(
        &mut self,
        params: UpdateEdgePropertyByKeyParams,
    ) -> StorageResult<bool> {
        if !self.is_open {
            return Err(StorageError::storage_not_open());
        }

        let dst_key = Self::edge_endpoint_key(params.dst, params.rank);
        if let Some(nbr) = self.merged_get_edge(&self.out_csr, params.src, dst_key, params.ts) {
            if let Some(dir) = self.wal_dir.clone() {
                let prop_name = self
                    .properties
                    .property_schema()
                    .iter()
                    .find(|schema| schema.prop_id as u16 == params.prop_id)
                    .map(|schema| schema.name.clone())
                    .unwrap_or_else(|| format!("prop_id={}", params.prop_id));
                super::super::wal::append_ops(
                    &dir,
                    &[super::super::wal::EdgeWalOp::PropertyUpdate {
                        src: params.src,
                        dst: params.dst,
                        rank: params.rank,
                        prop_name,
                        value: params.value.clone(),
                        ts: params.ts,
                    }],
                )?;
            }
            self.properties
                .set_property_by_id_for_edge(
                    nbr.edge_id,
                    PropertyId(params.prop_id),
                    Some(params.value.clone()),
                    params.ts,
                )
                .map_err(|_| {
                    StorageError::column_not_found(format!("prop_id={}", params.prop_id))
                })?;
            self.mark_properties_dirty_for_edge(params.src, params.dst);

            let src_key = Self::edge_endpoint_key(params.src, params.rank);
            if let Some(ie_nbr) = self.merged_get_edge(&self.in_csr, params.dst, src_key, params.ts)
            {
                if nbr.edge_id != ie_nbr.edge_id {
                    return Err(StorageError::data_corruption(format!(
                        "edge_id mismatch: out_csr={}, in_csr={} at edge ({}, {})",
                        nbr.edge_id.0, ie_nbr.edge_id.0, params.src, params.dst
                    )));
                }
            }
            self.maybe_run_auto_maintenance();
            return Ok(true);
        }

        Ok(false)
    }
}
