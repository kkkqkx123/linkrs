use super::super::EdgeStore;
use crate::edge::edge_table::staging::{EdgeStagingBatch, StagedInsert};
use crate::edge::edge_table::wal;
use crate::edge::EdgeStrategy;
use crate::edge::MutableCsrTrait;
use graphdb_core::types::EdgeId;
use graphdb_core::{StorageError, StorageResult};

impl EdgeStore {
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
    /// Crash-atomic boundary: the batch is the atomic unit. Logical redo is
    /// appended to the write-ahead log before any topology, timestamp or
    /// property mutation, and replay is idempotent (inserts skip on
    /// `EdgeAlreadyExists`, deletes treat missing edges as done), so a crash
    /// at any point replays to either the whole batch visible or the whole
    /// batch invisible. A torn apply can never leave single-direction
    /// topology or ownerless timestamps behind: the load-time copy audit
    /// counts such residue and rejects the load fail-closed. The manifest
    /// publish order (group shards before metadata, manifest file last with
    /// its tail embedded in `meta.bin`) is unchanged; only this contract and
    /// its tests are new.
    ///
    /// Returns the number of net applied entries (inserts plus deletes,
    /// excluding batch-cancelled pairs).
    pub fn commit_staging_batch(&mut self, mut batch: EdgeStagingBatch) -> StorageResult<usize> {
        if !self.is_open {
            return Err(StorageError::storage_not_open());
        }
        if self.migration_pending_checkpoint {
            return Err(StorageError::invalid_operation(
                "table requires a checkpoint after record-form switch before further writes"
                    .to_string(),
            ));
        }
        if batch.insert_count() > 0 && self.schema.oe_strategy == EdgeStrategy::None {
            return Err(StorageError::invalid_operation(
                "Cannot insert edge: out-edge strategy is None".to_string(),
            ));
        }
        self.prevalidate_staging_batch(&batch)?;
        let max_ts = batch.max_timestamp();
        let inserts = batch.take_inserts();
        let deletes = batch.take_deletes();
        let order = batch.take_order();
        if let Some(dir) = self.wal_dir.clone() {
            let mut ops = Vec::with_capacity(order.len());
            for ord in &order {
                if ord.is_insert {
                    let ins = &inserts[ord.slot];
                    // The redo owns a clone: taking the staged values here
                    // would drain the insert before the apply loop below
                    // reads them, silently storing defaults on WAL tables.
                    ops.push(wal::EdgeWalOp::Insert {
                        src: ins.src,
                        dst: ins.dst,
                        rank: ins.rank,
                        properties: ins.properties.clone(),
                        create_ts: ins.create_ts,
                    });
                } else {
                    let del = &deletes[ord.slot];
                    ops.push(wal::EdgeWalOp::Delete {
                        src: del.src,
                        dst: del.dst,
                        rank: del.rank,
                        delete_ts: del.delete_ts,
                    });
                }
            }
            wal::append_ops(&dir, &ops)?;
        }
        if order.is_empty() {
            return Ok(0);
        }
        self.reserve_topology_for_inserts(&inserts);

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
                        let err = self.rollback_applied_batch(applied_deletes, applied_inserts, e);
                        self.commit_scratch = scratch;
                        return Err(err);
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
                        let err = self.rollback_applied_batch(applied_deletes, applied_inserts, e);
                        self.commit_scratch = scratch;
                        return Err(err);
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
                self.ensure_copies_consistent(*edge_id)?;
            }
        }
        self.commit_scratch = scratch;
        Ok(applied)
    }

    /// Pre-size touched topology rows once for the staged inserts.
    ///
    /// Counts inserts per bound endpoint per direction and sizes each row a
    /// single time, so the apply loop below lands in reserved gaps instead
    /// of growing overflow chunk by chunk. Sizing only; inserts still flow
    /// through the regular apply path so authority, properties, dirt and
    /// append logs stay exact. Failures leave at most empty groups behind.
    fn reserve_topology_for_inserts(&mut self, inserts: &[StagedInsert]) {
        if inserts.is_empty() {
            return;
        }
        let mut by_src: Vec<u32> = inserts.iter().map(|ins| ins.src).collect();
        by_src.sort_unstable();
        let mut out_counts: Vec<(u32, usize)> = Vec::new();
        for src in by_src {
            if let Some(last) = out_counts.last_mut() {
                if last.0 == src {
                    last.1 += 1;
                    continue;
                }
            }
            out_counts.push((src, 1));
        }
        let mut by_dst: Vec<u32> = inserts.iter().map(|ins| ins.dst).collect();
        by_dst.sort_unstable();
        let mut in_counts: Vec<(u32, usize)> = Vec::new();
        for dst in by_dst {
            if let Some(last) = in_counts.last_mut() {
                if last.0 == dst {
                    last.1 += 1;
                    continue;
                }
            }
            in_counts.push((dst, 1));
        }
        // Best-effort sizing only: the apply loop below reserves through the
        // routed insert path and reports real failures there. A reservation
        // miss here only leaves sizing undone, never corrupt state, so it is
        // observed and ignored rather than aborting the batch.
        if let Err(e) = self.out_csr.reserve_for_batch(&out_counts) {
            log::debug!("reserve out topology skipped: {}", e);
        }
        if let Err(e) = self.in_csr.reserve_for_batch(&in_counts) {
            log::debug!("reserve in topology skipped: {}", e);
        }
    }

    /// Shared gate for authority revival on delete rollback.
    ///
    /// Authority may only return to live when both directions physically
    /// reverted. A partial revert keeps the authority deletion mark so a
    /// future timestamp tombstone can never coexist with a live authority
    /// record.
    #[inline]
    pub(super) fn fully_reverted(out_ok: bool, in_ok: bool) -> bool {
        out_ok && in_ok
    }

    #[inline]
    pub(super) fn partial_revert_error(edge_id: EdgeId) -> StorageError {
        StorageError::invalid_operation(format!(
            "delete rollback failed for edge {:?}: partial direction revert",
            edge_id
        ))
    }
}
