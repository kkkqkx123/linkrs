use super::super::EdgeStore;
use crate::edge::MutableCsrTrait;
use graphdb_core::types::{EdgeId, Timestamp};
use graphdb_core::{StorageError, StorageResult};

impl EdgeStore {
    /// Revive the authority record after both directions reverted.
    ///
    /// Shared by the batch rollback and the single-key undo so the two paths
    /// cannot drift apart. Returns true when the record was revived and false
    /// when neither direction held a tombstone to revert. A partial revert
    /// keeps the deletion mark and reports an error.
    fn revive_authority_after_revert(
        &mut self,
        edge_id: EdgeId,
        out_ok: bool,
        in_ok: bool,
    ) -> StorageResult<bool> {
        let has_out = self.schema.has_out();
        let has_in = self.schema.has_in();
        let out_done = !has_out || out_ok;
        let in_done = !has_in || in_ok;
        if out_done && in_done && (out_ok || in_ok || (!has_out && !has_in)) {
            if let Some(ts_info) = self.mvcc.edge_timestamps.get_mut(&edge_id) {
                ts_info.delete_ts = Timestamp::MAX;
            }
            let _ = self.properties.revert_deletion_for_edge(edge_id);
            // Trace the owning group so the revived row reaches its shard
            // even when no other write marked that group.
            if let Some(owner) = self.edge_owner.get(&edge_id) {
                self.mark_properties_dirty_for_owner(owner);
            } else {
                self.mark_properties_dirty();
            }
            self.debug_assert_copies_consistent(edge_id);
            return Ok(true);
        }
        if !out_done && !in_done {
            return Ok(false);
        }
        if (!has_out || out_ok) && (!has_in || in_ok) {
            if let Some(ts_info) = self.mvcc.edge_timestamps.get_mut(&edge_id) {
                ts_info.delete_ts = Timestamp::MAX;
            }
            let _ = self.properties.revert_deletion_for_edge(edge_id);
            if let Some(owner) = self.edge_owner.get(&edge_id) {
                self.mark_properties_dirty_for_owner(owner);
            } else {
                self.mark_properties_dirty();
            }
            self.debug_assert_copies_consistent(edge_id);
            return Ok(true);
        }
        if !out_ok && !in_ok {
            return Ok(false);
        }
        Err(Self::partial_revert_error(edge_id))
    }

    /// Revert one batch-applied delete during batch rollback.
    ///
    /// Positional fast path first with edge-id fallback: the locate shares
    /// one scan, the positional write then addresses the slot directly and
    /// refuses stale positions instead of touching the wrong edge.
    pub(super) fn revert_applied_delete(
        &mut self,
        src: u32,
        dst: u32,
        edge_id: EdgeId,
        ts: Timestamp,
    ) -> StorageResult<()> {
        let has_out = self.schema.has_out();
        let has_in = self.schema.has_in();
        let out_ok = if has_out {
            match self.out_csr.locate_edge(src, edge_id) {
                Some((position, _)) => {
                    self.out_csr
                        .revert_delete_at_position(src, position, edge_id, ts)
                        || self.out_csr.revert_delete_by_edge_id(src, edge_id, ts)
                }
                None => self.out_csr.revert_delete_by_edge_id(src, edge_id, ts),
            }
        } else {
            false
        };
        let in_ok = if has_in {
            match self.in_csr.locate_edge(dst, edge_id) {
                Some((position, _)) => {
                    self.in_csr
                        .revert_delete_at_position(dst, position, edge_id, ts)
                        || self.in_csr.revert_delete_by_edge_id(dst, edge_id, ts)
                }
                None => self.in_csr.revert_delete_by_edge_id(dst, edge_id, ts),
            }
        } else {
            false
        };
        self.revive_authority_after_revert(edge_id, out_ok, in_ok)?;
        Ok(())
    }

    /// Roll back everything this batch applied, then report the failure.
    ///
    /// Deletes revert first through the shared partial-revert gate, inserts
    /// erase afterwards, and every entry is drained on both paths so a batch
    /// reported as failed never leaves visible residue. A failed revert keeps
    /// its authority deletion mark; the first revert error is returned with
    /// priority over the original error so the caller cannot mistake the
    /// half-reverted state for a clean commit failure.
    pub(super) fn rollback_applied_batch(
        &mut self,
        applied_deletes: &mut Vec<(u32, u32, i64, EdgeId, Timestamp)>,
        applied_inserts: &mut Vec<(u32, u32, i64, EdgeId, Timestamp)>,
        original: StorageError,
    ) -> StorageError {
        let mut revert_error: Option<StorageError> = None;
        for (src, dst, _rank, edge_id, ts) in applied_deletes.drain(..) {
            if let Err(revert_err) = self.revert_applied_delete(src, dst, edge_id, ts) {
                if revert_error.is_none() {
                    revert_error = Some(revert_err);
                }
            }
        }
        for (src, dst, rank, edge_id, ts) in applied_inserts.drain(..) {
            self.erase_applied_insert(src, dst, rank, edge_id, ts);
        }
        revert_error.unwrap_or(original)
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
        let has_out = self.schema.has_out();
        let has_in = self.schema.has_in();
        let mut candidates: Vec<EdgeId> = Vec::new();
        if has_out {
            self.out_csr.visit_physical(src, |nbr| {
                if nbr.endpoint == dst && nbr.rank == rank {
                    candidates.push(nbr.edge_id);
                }
                true
            });
        }
        if candidates.is_empty() && has_in {
            self.in_csr.visit_physical(dst, |nbr| {
                if nbr.endpoint == src && nbr.rank == rank {
                    candidates.push(nbr.edge_id);
                }
                true
            });
        }
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
        // Erase forms drop the edge id on delete, so an id-keyed locate
        // after delete cannot find the sentinel slot. A positional revert
        // holding the pre-delete slot revives the retained word; without a
        // position the undo reports not found and leaves authority deleted.
        // Missing legs count as reverted because they store nothing.
        let (out_ok, in_ok) = if self.is_bundled() {
            let out_ok = if has_out {
                match self.out_csr.locate_edge(src, edge_id) {
                    Some((position, _)) => self
                        .out_csr
                        .revert_delete_at_position(src, position, edge_id, ts),
                    None => false,
                }
            } else {
                true
            };
            let in_ok = if has_in {
                match self.in_csr.locate_edge(dst, edge_id) {
                    Some((position, _)) => self
                        .in_csr
                        .revert_delete_at_position(dst, position, edge_id, ts),
                    None => false,
                }
            } else {
                true
            };
            (out_ok, in_ok)
        } else {
            (
                if has_out {
                    self.out_csr.revert_delete_by_edge_id(src, edge_id, ts)
                } else {
                    true
                },
                if has_in {
                    self.in_csr.revert_delete_by_edge_id(dst, edge_id, ts)
                } else {
                    true
                },
            )
        };
        if !self.revive_authority_after_revert(edge_id, out_ok, in_ok)? {
            return Ok(false);
        }
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
                let _ = self.note_index_result(&prop_name, result, latency);
            }
        }
        self.mark_properties_dirty();
        self.debug_assert_copies_consistent(edge_id);
        Ok(true)
    }
}
