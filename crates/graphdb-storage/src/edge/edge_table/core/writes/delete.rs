use super::super::EdgeStore;
use crate::edge::edge_table::staging::EdgeStagingBatch;
use crate::edge::EdgePosition;
use crate::edge::MutableCsrTrait;
use graphdb_core::types::{EdgeId, Timestamp};
use graphdb_core::{StorageError, StorageResult};

impl EdgeStore {
    /// Move one staged delete into the committed structures.
    ///
    /// Full-match endpoint semantics: one call deletes every live match for
    /// the endpoint key and reports the deleted count for rollback
    /// reconciliation. Returns the deleted edge id, or `None` when no edge
    /// matched.
    pub(super) fn apply_staged_delete(
        &mut self,
        src: u32,
        dst: u32,
        rank: i64,
        ts: Timestamp,
    ) -> StorageResult<Option<EdgeId>> {
        // Frozen groups reject deletes explicitly instead of reporting a
        // miss: the edge exists, only the layout refuses the write.
        // Single-direction tables check only the stored leg.
        if (self.schema.has_out() && self.out_csr.is_group_frozen_for(src))
            || (self.schema.has_in() && self.in_csr.is_group_frozen_for(dst))
        {
            return Err(StorageError::invalid_operation(format!(
                "frozen group rejects deletes: ({}, {}, {})",
                src, dst, rank
            )));
        }
        let dst_key = Self::edge_endpoint_key(dst, rank);
        let src_key = Self::edge_endpoint_key(src, rank);
        let has_out = self.schema.has_out();
        let has_in = self.schema.has_in();

        let edge_properties = if self.property_index.is_some() {
            self.get_edge(src, dst, rank, ts).map(|e| e.properties)
        } else {
            None
        };

        // Single-direction tables delete only the stored leg; dual tables
        // keep both legs consistent with rollback on partial match.
        if has_out && !has_in {
            return self.apply_staged_delete_single(src, dst_key, dst, rank, ts, true);
        }
        if has_in && !has_out {
            return self.apply_staged_delete_single(dst, src_key, src, rank, ts, false);
        }
        // Single-pass out-direction delete: stamp live matches in place while
        // collecting the first stamped id and its row position, so the merged
        // read above (for the index snapshot) is the only extra locate. A
        // zero count falls through to the miss path below which distinguishes
        // absence from conflict.
        let mut first_out: Option<(EdgeId, Option<EdgePosition>)> = None;
        let out_deleted = self.out_csr.delete_edge_by_dst_reporting_positioned(
            src,
            dst_key,
            ts,
            &mut |edge_id, position| {
                if first_out.is_none() {
                    first_out = Some((edge_id, position));
                }
            },
        );
        if out_deleted > 0 {
            let (edge_id, position) = first_out.expect("reported delete carries an id");
            if out_deleted > 1 {
                log::debug!(
                    "apply_staged_delete multi-match: ({}, {}, {}) out_deleted={}",
                    src,
                    dst,
                    rank,
                    out_deleted
                );
            }
            let mut noop_in = |_edge_id: EdgeId, _position: Option<EdgePosition>| {};
            let in_deleted =
                self.in_csr
                    .delete_edge_by_dst_reporting_positioned(dst, src_key, ts, &mut noop_in);
            if in_deleted == 0 {
                // Roll back the out-direction deletion to keep both sides
                // consistent. Count reconciliation: expected exactly one
                // in-direction match for the out edge just deleted. The
                // position captured by the reporting pass addresses the slot
                // directly; without one the edge-id scan is the fallback.
                let reverted = match position {
                    Some(slot) => self
                        .out_csr
                        .revert_delete_at_position(src, slot, edge_id, ts),
                    None => self.out_csr.revert_delete_by_edge_id(src, edge_id, ts),
                };
                if !reverted {
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
            if nbr.endpoint == dst && nbr.rank == rank {
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

    /// Delete on a single-direction table: stamp the stored leg only.
    ///
    /// `bound` is the row address on the stored leg, `key` the endpoint key,
    /// `peer` the opposite endpoint kept for index bookkeeping. No cross-leg
    /// reconciliation runs because the missing leg stores nothing.
    fn apply_staged_delete_single(
        &mut self,
        bound: u32,
        key: graphdb_core::types::VertexId,
        peer: u32,
        rank: i64,
        ts: Timestamp,
        is_out: bool,
    ) -> StorageResult<Option<EdgeId>> {
        let (src, dst) = if is_out { (bound, peer) } else { (peer, bound) };
        let edge_properties = if self.property_index.is_some() {
            self.get_edge(src, dst, rank, ts).map(|e| e.properties)
        } else {
            None
        };
        let mut first: Option<(EdgeId, Option<EdgePosition>)> = None;
        let deleted = if is_out {
            self.out_csr.delete_edge_by_dst_reporting_positioned(
                bound,
                key,
                ts,
                &mut |edge_id, position| {
                    if first.is_none() {
                        first = Some((edge_id, position));
                    }
                },
            )
        } else {
            self.in_csr.delete_edge_by_dst_reporting_positioned(
                bound,
                key,
                ts,
                &mut |edge_id, position| {
                    if first.is_none() {
                        first = Some((edge_id, position));
                    }
                },
            )
        };
        if deleted == 0 {
            let mut candidates: Vec<EdgeId> = Vec::new();
            let want_endpoint = if is_out { dst } else { src };
            let csr = if is_out { &self.out_csr } else { &self.in_csr };
            csr.visit_physical(bound, |nbr| {
                if nbr.endpoint == want_endpoint && nbr.rank == rank {
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
            return Ok(None);
        }
        let (edge_id, _) = first.expect("reported delete carries an id");
        self.mvcc.record_edge_deletion(edge_id, ts);
        let _ = self.properties.mark_deleted(edge_id, ts);
        self.update_property_index_on_delete(&edge_properties, src, dst, rank, ts);
        self.mark_properties_dirty();
        self.debug_assert_copies_consistent(edge_id);
        Ok(Some(edge_id))
    }

    /// Erase one batch-applied insert during batch rollback.
    ///
    /// Physical removal across all copies; tolerates absence so rollback
    /// stays total even under partial application.
    pub(super) fn erase_applied_insert(
        &mut self,
        src: u32,
        dst: u32,
        rank: i64,
        edge_id: EdgeId,
        ts: Timestamp,
    ) {
        let lookup_src = if self.schema.has_out() { src } else { dst };
        let properties = if self.is_bundled() {
            self.bundled_index_pairs_for_erase(lookup_src, edge_id)
        } else {
            self.properties
                .read_properties_by_edge_id(edge_id)
                .unwrap_or_default()
        };
        if self.schema.has_out() {
            self.out_csr.rollback_insert(src, edge_id);
        }
        if self.schema.has_in() {
            self.in_csr.rollback_insert(dst, edge_id);
        }
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
                let _ = self.note_index_result(&prop_name, result, latency);
            }
        }
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
        let lookup_src = if self.schema.has_out() { src } else { dst };
        let properties = if self.is_bundled() {
            self.bundled_index_pairs_for_erase(lookup_src, edge_id)
        } else {
            self.properties
                .read_properties_by_edge_id(edge_id)
                .unwrap_or_default()
        };
        if self.schema.has_out() {
            self.out_csr.rollback_insert(src, edge_id);
        }
        if self.schema.has_in() {
            self.in_csr.rollback_insert(dst, edge_id);
        }
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
                let _ = self.note_index_result(&prop_name, result, latency);
            }
        }
        self.debug_assert_copies_consistent(edge_id);
        true
    }
}
