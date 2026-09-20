use super::super::csr_shared::{
    can_revert_delete, decide_slot_delete, decode_endpoint_pair, is_reclaimable_cold,
    DeleteSlotOutcome,
};
use super::super::{ColdStamps, EdgeId, HotNbr, Nbr, Timestamp, VertexId};
use super::core::REUSE_HINT_UNKNOWN;
use super::overflow::OVERFLOW_REPACK_CHUNKS_PER_VERTEX;
use super::MutableCsr;
use graphdb_core::{StorageError, StorageResult};
use std::collections::HashSet;

/// Hot-path bound for the tombstone-reuse scan. Rows wider than this leave
/// leftover reclaimable slots to the maintenance pass instead of scanning
/// the whole row on every insert. The per-vertex reuse hint usually avoids
/// the scan entirely; the bound only caps the fallback walk when the hint
/// is unknown or stale. Tune against `benches/csr_perf_bench.rs` before
/// changing it.
const TOMBSTONE_REUSE_SCAN_BOUND: usize = 64;

/// Physical position of one stored edge inside its row.
///
/// Primary positions address the primary block by slot index; overflow
/// positions address a chunk and a slot inside it. Positions are valid only
/// while the row is untouched by compaction, rebalance, repack or removal:
/// every positional write revalidates the expected edge id first and
/// refuses stale positions instead of touching the wrong edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgePosition {
    Primary { slot: u32 },
    Overflow { chunk: u32, slot: u32 },
}

impl MutableCsr {
    fn append_overflow(&mut self, src_vid: u32, nbr: Nbr, live_hint: usize) {
        let chunk_edges = self.effective_chunk_edges(live_hint);
        // Single table routing per call: the tail-chunk check, the push and
        // the new-chunk capacity report all happen inside one lookup.
        let (chunk_count, added) = self.overflow_chunks.push_to_row(src_vid, nbr, chunk_edges);
        if let Some(new_cap) = added {
            self.add_capacity(new_cap);
            self.overflow_chunk_allocs += 1;
        }
        if nbr.delete_ts == Timestamp::MAX {
            // Tail slot just pushed: the row-relative position feeds the
            // wide-row location index directly, so no rescan is needed to
            // keep it exact. Rows still narrow stay set-free through the
            // width gate inside the tracker.
            let position = self
                .overflow_chunks
                .get(src_vid)
                .map(|chunks| {
                    let chunk = chunks.len().saturating_sub(1);
                    let slot = chunks
                        .last()
                        .map(|tail| tail.len().saturating_sub(1))
                        .unwrap_or(0);
                    EdgePosition::Overflow {
                        chunk: chunk as u32,
                        slot: slot as u32,
                    }
                })
                .unwrap_or(EdgePosition::Overflow {
                    chunk: chunk_count.saturating_sub(1) as u32,
                    slot: 0,
                });
            self.track_live_insert(src_vid, nbr.endpoint, nbr.rank, position);
        }
        // Per-vertex overflow bound: past the limit the row is consolidated
        // into one contiguous chunk whether or not it holds dead entries,
        // so skewed rows cannot grow unbounded pointer chains. The repack
        // preserves every entry (no watermark here); watermark-confirmed
        // reclaim runs through the vertex-level reporting passes. Later
        // appends grow fresh graded tail chunks until the next breach, so
        // the consolidation cost stays amortized.
        if chunk_count > OVERFLOW_REPACK_CHUNKS_PER_VERTEX {
            let mut noop = |_id: EdgeId, _ts: Timestamp| {};
            self.compact_overflow_for_vertex(src_vid, Timestamp::MAX, &mut noop);
            self.repack_count += 1;
        }
    }

    /// Insert an edge with automatic capacity expansion
    pub fn insert_edge(
        &mut self,
        src_vid: u32,
        dst: VertexId,
        edge_id: EdgeId,
        ts: Timestamp,
    ) -> StorageResult<()> {
        let (decoded_endpoint, decoded_rank) = decode_endpoint_pair(dst);

        let src_idx = src_vid as usize;

        if src_idx >= self.vertex_capacity() {
            self.ensure_vertex_capacity(src_idx + 1);
        }

        // Lazy primary block allocation on first edge
        if self.rows.primary_capacities[src_idx] == 0 {
            self.allocate_primary_block(src_idx);
        }

        // Duplicate check via the single live set covering primary and
        // overflow. The set is authoritative: it is rebuilt on load and
        // compact and updated on every write. Indexed rows answer from the
        // set; narrow rows pay one combined scan that also counts live
        // entries, so the width probe below costs no second walk.
        let live = if let Some(set) = self.live_sets.get(&src_vid) {
            if set.contains(&(decoded_endpoint, decoded_rank)) {
                return Err(StorageError::edge_already_exists(format!(
                    "{} -> {:?}",
                    src_vid, dst
                )));
            }
            set.len()
        } else {
            let (present, live) = self.row_live_scan(src_vid, decoded_endpoint, decoded_rank);
            if present {
                return Err(StorageError::edge_already_exists(format!(
                    "{} -> {:?}",
                    src_vid, dst
                )));
            }
            live
        };

        let nbr_with_ts = Nbr::with_create_ts(decoded_endpoint, decoded_rank, edge_id, ts);

        // Steady-state gap fill: primary trailing gaps are filled first
        // even when overflow exists, so everyday writes land in row gaps.
        // Row rebalances that pull overflow back into freed gaps run on the
        // maintenance path, never on this hot path, keeping inserts O(1).
        let degree = self.rows.degrees[src_idx] as usize;
        if degree < self.rows.primary_capacities[src_idx] as usize {
            let base = self.rows.adj_offsets[src_idx] as usize;
            self.set_slot(base + degree, nbr_with_ts);
            self.rows.degrees[src_idx] += 1;
            self.live_counts[src_idx] += 1;
            self.track_live_insert(
                src_vid,
                decoded_endpoint,
                decoded_rank,
                EdgePosition::Primary {
                    slot: degree as u32,
                },
            );
            self.edge_count += 1;
            return Ok(());
        }

        // Full row: with a watermark-derived reuse cutoff, reclaim one
        // GC-eligible primary tombstone in place instead of growing
        // overflow. The eligibility predicate matches the maintenance
        // compaction exactly, so the hot path never drops a tombstone the
        // reclaim pass would keep. Overflow tombstones stay for the region
        // compaction. The per-vertex hint gives O(1) reuse when a recent
        // delete recorded its slot; the bounded prefix scan below is only
        // the fallback when the hint is unknown or stale.
        if self.tombstone_reuse_cutoff != Timestamp::MAX {
            let base = self.rows.adj_offsets[src_idx] as usize;
            let cutoff = self.tombstone_reuse_cutoff;
            if let Some(&hint) = self.reuse_hint.get(src_idx) {
                if hint != REUSE_HINT_UNKNOWN {
                    let slot = hint as usize;
                    if slot < degree {
                        let reclaimable = self
                            .cold_at(base + slot)
                            .is_some_and(|cold| is_reclaimable_cold(&cold, cutoff));
                        if reclaimable {
                            self.set_slot(base + slot, nbr_with_ts);
                            self.invalidate_reuse_hint(src_idx);
                            self.live_counts[src_idx] += 1;
                            self.tombstone_counts[src_idx] =
                                self.tombstone_counts[src_idx].saturating_sub(1);
                            self.track_live_insert(
                                src_vid,
                                decoded_endpoint,
                                decoded_rank,
                                EdgePosition::Primary { slot: slot as u32 },
                            );
                            self.edge_count += 1;
                            self.tombstone_reuse_count += 1;
                            return Ok(());
                        }
                    }
                    self.invalidate_reuse_hint(src_idx);
                }
            }
            let bound = degree.min(TOMBSTONE_REUSE_SCAN_BOUND);
            for i in 0..bound {
                let reclaimable = self
                    .cold_at(base + i)
                    .is_some_and(|cold| is_reclaimable_cold(&cold, cutoff));
                if reclaimable {
                    self.set_slot(base + i, nbr_with_ts);
                    self.invalidate_reuse_hint(src_idx);
                    self.live_counts[src_idx] += 1;
                    self.tombstone_counts[src_idx] =
                        self.tombstone_counts[src_idx].saturating_sub(1);
                    self.track_live_insert(
                        src_vid,
                        decoded_endpoint,
                        decoded_rank,
                        EdgePosition::Primary { slot: i as u32 },
                    );
                    self.edge_count += 1;
                    self.tombstone_reuse_count += 1;
                    return Ok(());
                }
            }
        }

        self.append_overflow(src_vid, nbr_with_ts, live.saturating_add(1));
        self.live_counts[src_idx] += 1;
        self.edge_count += 1;
        Ok(())
    }

    /// Locate one overflow entry by edge id, stopping at the first match.
    ///
    /// Point-lookup fast path: never builds a match vector, so hot single
    /// deletes pay no heap allocation. Callers needing every match use the
    /// in-place stamping passes instead.
    fn scan_overflow_for_edge_id(&self, src_vid: u32, edge_id: EdgeId) -> Option<(usize, usize)> {
        let chunks = self.overflow_chunks.get(src_vid)?;
        for (chunk_idx, chunk) in chunks.iter().enumerate() {
            for (edge_idx, hot) in chunk.hot_slice().iter().enumerate() {
                if hot.edge_id == edge_id {
                    return Some((chunk_idx, edge_idx));
                }
            }
        }
        None
    }

    /// Delete an edge by edge_id.
    ///
    /// Returns `Ok(true)` when deleted, `Ok(false)` when the edge does not
    /// exist or is not deletable at `ts`, and
    /// `Err(StorageError::write_write_conflict)` when the edge was already
    /// deleted at a different timestamp (write-write conflict at the storage
    /// layer, surfaced immediately instead of a silent `false`).
    pub fn delete_edge(
        &mut self,
        src_vid: u32,
        edge_id: EdgeId,
        ts: Timestamp,
    ) -> StorageResult<bool> {
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return Ok(false);
        }

        // Scan primary: locate by edge id on shared slices first, then
        // stamp through one indexed write.
        let found = {
            let (hot, cold) = self.primary_pair(src_idx);
            hot.iter()
                .zip(cold.iter())
                .enumerate()
                .find_map(|(i, (h, c))| {
                    (h.edge_id == edge_id).then(|| (i, Nbr::from_parts(*h, *c)))
                })
        };
        if let Some((i, probe)) = found {
            match decide_slot_delete(&probe, edge_id, ts)? {
                DeleteSlotOutcome::AlreadyStamped => {
                    return Ok(false);
                }
                DeleteSlotOutcome::Stamped => {}
            }
            let (start, _) = self.primary_window(src_idx);
            self.cold_list[start + i].delete_ts = ts;
            self.edge_count -= 1;
            self.live_counts[src_idx] = self.live_counts[src_idx].saturating_sub(1);
            self.tombstone_counts[src_idx] += 1;
            self.note_primary_tombstone(src_idx, i);
            self.track_live_remove(src_vid, probe.endpoint, probe.rank);
            return Ok(true);
        }

        // Scan overflow
        if let Some((chunk_idx, edge_idx)) = self.scan_overflow_for_edge_id(src_vid, edge_id) {
            // Capture the assembled slot before mutable borrow ends for live set update.
            let probe = {
                let chunks = self.overflow_chunks.get(src_vid).unwrap();
                chunks[chunk_idx].slot_at(edge_idx).unwrap()
            };
            if let Some(chunks) = self.overflow_chunks.get_mut(src_vid) {
                match decide_slot_delete(&probe, edge_id, ts)? {
                    DeleteSlotOutcome::AlreadyStamped => {
                        return Ok(false);
                    }
                    DeleteSlotOutcome::Stamped => {}
                }
                chunks[chunk_idx].cold_at_mut(edge_idx).unwrap().delete_ts = ts;
                self.edge_count -= 1;
                self.live_counts[src_idx] = self.live_counts[src_idx].saturating_sub(1);
                self.tombstone_counts[src_idx] += 1;
                self.track_live_remove(src_vid, probe.endpoint, probe.rank);
                return Ok(true);
            }
        }

        Ok(false)
    }

    /// Delete edges by destination vertex with full-match semantics.
    ///
    /// Deletes every live match and returns the deleted count so table
    /// rollback can reconcile by count. One call deletes the whole match.
    /// Single pass: primary and overflow rows are stamped in place while
    /// walking, so no match vector is materialized and no second scan runs.
    /// Every stamped id is reported through `on_deleted` for append-log and
    /// audit callers that previously paid a separate collection scan.
    pub fn delete_edge_by_dst_reporting(
        &mut self,
        src_vid: u32,
        dst: VertexId,
        ts: Timestamp,
        on_deleted: &mut dyn FnMut(EdgeId),
    ) -> usize {
        let mut noop_pos = |edge_id: EdgeId, _position: EdgePosition| {
            on_deleted(edge_id);
        };
        self.delete_edge_by_dst_reporting_positioned(src_vid, dst, ts, &mut noop_pos)
    }

    /// Position-reporting form of [`Self::delete_edge_by_dst_reporting`].
    ///
    /// Same single stamping pass, but each stamped id arrives with its row
    /// position so the caller can revert or re-delete the exact slot without
    /// rescanning. Positions are row-local and expire on the next compaction,
    /// rebalance, repack or removal of the row.
    pub fn delete_edge_by_dst_reporting_positioned(
        &mut self,
        src_vid: u32,
        dst: VertexId,
        ts: Timestamp,
        on_deleted: &mut dyn FnMut(EdgeId, EdgePosition),
    ) -> usize {
        let (decoded_endpoint, decoded_rank) = decode_endpoint_pair(dst);
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return 0;
        }

        let mut deleted = 0usize;

        // Stamp primary matches in place over disjoint hot/cold slices, so
        // the walk pays one bounds check per row while stamping inline.
        // Only direct field accesses run inside the loop: no `&self` method
        // calls, so the shared hot borrow and the exclusive cold borrow
        // coexist with the edge-count ledger update.
        let (start, end) = self.primary_window(src_idx);
        let hot = &self.hot_list[start..end];
        let cold = &mut self.cold_list[start..end];
        let mut first_primary_tombstone: Option<usize> = None;
        for (i, (h, c)) in hot.iter().zip(cold.iter_mut()).enumerate() {
            if h.endpoint == decoded_endpoint && h.rank == decoded_rank && c.is_live() {
                c.delete_ts = ts;
                self.edge_count -= 1;
                self.live_counts[src_idx] = self.live_counts[src_idx].saturating_sub(1);
                self.tombstone_counts[src_idx] += 1;
                on_deleted(h.edge_id, EdgePosition::Primary { slot: i as u32 });
                if first_primary_tombstone.is_none_or(|first| i < first) {
                    first_primary_tombstone = Some(i);
                }
                deleted += 1;
            }
        }

        // Stamp overflow matches in the same pass.
        if let Some(chunks) = self.overflow_chunks.get_mut(src_vid) {
            for (chunk_idx, chunk) in chunks.iter_mut().enumerate() {
                for slot_idx in 0..chunk.len() {
                    let hot = chunk.hot_at(slot_idx).unwrap();
                    let cold = chunk.cold_at(slot_idx).unwrap();
                    if hot.endpoint == decoded_endpoint
                        && hot.rank == decoded_rank
                        && cold.is_live()
                    {
                        chunk.cold_at_mut(slot_idx).unwrap().delete_ts = ts;
                        self.edge_count -= 1;
                        self.live_counts[src_idx] = self.live_counts[src_idx].saturating_sub(1);
                        self.tombstone_counts[src_idx] += 1;
                        on_deleted(
                            hot.edge_id,
                            EdgePosition::Overflow {
                                chunk: chunk_idx as u32,
                                slot: slot_idx as u32,
                            },
                        );
                        deleted += 1;
                    }
                }
            }
        }
        if deleted > 0 {
            if let Some(slot) = first_primary_tombstone {
                self.note_primary_tombstone(src_idx, slot);
            }
            self.track_live_remove(src_vid, decoded_endpoint, decoded_rank);
        }

        deleted
    }

    /// Delete edges by destination vertex with full-match semantics.
    ///
    /// Deletes every live match and returns the deleted count so table
    /// rollback can reconcile by count. One call deletes the whole match.
    pub fn delete_edge_by_dst(&mut self, src_vid: u32, dst: VertexId, ts: Timestamp) -> usize {
        let mut noop = |_: EdgeId| {};
        self.delete_edge_by_dst_reporting(src_vid, dst, ts, &mut noop)
    }

    pub fn delete_edge_by_offset(
        &mut self,
        src_vid: u32,
        offset: i32,
        ts: Timestamp,
    ) -> StorageResult<bool> {
        if offset < 0 {
            return Ok(false);
        }
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() || self.rows.primary_capacities[src_idx] == 0 {
            return Ok(false);
        }
        if offset as usize >= self.rows.degrees[src_idx] as usize {
            return Ok(false);
        }
        let idx = self.rows.adj_offsets[src_idx] as usize + offset as usize;
        if idx >= self.hot_list.len() {
            return Ok(false);
        }
        let probe = Nbr::from_parts(self.hot_list[idx], self.cold_list[idx]);
        // Shared delete state machine: a tombstone at another timestamp
        // surfaces as a write-write conflict, an identical re-delete or a
        // not-yet-created edge reports `false`, and only a live deletable
        // slot falls through to the stamp below.
        match decide_slot_delete(&probe, probe.edge_id, ts)? {
            DeleteSlotOutcome::AlreadyStamped => {
                return Ok(false);
            }
            DeleteSlotOutcome::Stamped => {}
        }
        self.cold_list[idx].delete_ts = ts;
        self.edge_count -= 1;
        self.live_counts[src_idx] = self.live_counts[src_idx].saturating_sub(1);
        self.tombstone_counts[src_idx] += 1;
        self.note_primary_tombstone(src_idx, offset as usize);
        self.track_live_remove(src_vid, probe.endpoint, probe.rank);
        Ok(true)
    }

    /// Revert a deleted edge by offset position in the primary block.
    ///
    /// Only reverts deletions that occurred at or before the given timestamp.
    /// This maintains MVCC semantics during transaction rollback: we can only
    /// undo deletions that happened before the rollback point.
    pub fn revert_delete_by_offset(&mut self, src_vid: u32, offset: i32, ts: Timestamp) -> bool {
        if offset < 0 {
            return false;
        }
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() || self.rows.primary_capacities[src_idx] == 0 {
            return false;
        }
        if offset as usize >= self.rows.degrees[src_idx] as usize {
            return false;
        }

        let base_offset = self.rows.adj_offsets[src_idx] as usize;
        let idx = base_offset + offset as usize;

        if idx >= self.hot_list.len() {
            return false;
        }

        let probe = Nbr::from_parts(self.hot_list[idx], self.cold_list[idx]);
        // Only revert deletions that happened at or before rollback time.
        // Prevents rolling back deletions that occur after the rollback point.
        if can_revert_delete(&probe, ts) {
            self.cold_list[idx].delete_ts = Timestamp::MAX;
            self.edge_count += 1;
            self.live_counts[src_idx] += 1;
            self.tombstone_counts[src_idx] =
                self.tombstone_counts[src_idx].saturating_sub(1);
            self.invalidate_reuse_hint(src_idx);
            self.track_live_insert(
                src_vid,
                probe.endpoint,
                probe.rank,
                EdgePosition::Primary {
                    slot: offset as u32,
                },
            );
            return true;
        }
        false
    }

    /// Erase one just-inserted edge for insert rollback.
    ///
    /// Rollback-only path: erases the slot and updates degree/edge count,
    /// leaving no tombstone trace, so a failed double-write can pretend the
    /// edge never existed. This is deliberately distinct from MVCC deletes
    /// (`delete_edge` family), which stamp `delete_ts` and stay visible to
    /// the reclaim machinery (`reclaimable_count`, `vertex_reclaim_probe`,
    /// `fragmentation_ratio`). Physical erasures free their slots inline
    /// and therefore never appear in tombstone statistics; mixing the two
    /// models on one row is expected (erasure for insert rollback,
    /// tombstones for MVCC) and both keep the capacity ledger and the live
    /// index exact.
    pub fn rollback_insert(&mut self, src_vid: u32, edge_id: EdgeId) -> bool {
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return false;
        }

        // Scan primary on the hot slice, then close the gap with one
        // memmove per half instead of a per-slot copy loop.
        let found = {
            let (hot, cold) = self.primary_pair(src_idx);
            hot.iter()
                .zip(cold.iter())
                .enumerate()
                .find_map(|(i, (h, c))| {
                    (h.edge_id == edge_id).then(|| (i, Nbr::from_parts(*h, *c)))
                })
        };
        if let Some((i, probe)) = found {
            let (start, _) = self.primary_window(src_idx);
            let degree = self.rows.degrees[src_idx] as usize;
            let was_live = probe.delete_ts == Timestamp::MAX;
            // Shift left to close the gap, then decrement the degree.
            self.hot_list
                .copy_within(start + i + 1..start + degree, start + i);
            self.cold_list
                .copy_within(start + i + 1..start + degree, start + i);
            self.rows.degrees[src_idx] -= 1;
            self.invalidate_reuse_hint(src_idx);
            if was_live {
                self.live_counts[src_idx] = self.live_counts[src_idx].saturating_sub(1);
                // Gap-closing memmove shifts every later primary slot left
                // by one, so stored row positions go stale: rebuild the
                // indexed row when one exists instead of dropping one key.
                if self.live_sets.get(&src_vid).is_some() {
                    self.rebuild_live_set_for_vertex(src_vid);
                }
                self.edge_count -= 1;
            } else {
                self.tombstone_counts[src_idx] =
                    self.tombstone_counts[src_idx].saturating_sub(1);
            }
            return true;
        }

        // Scan overflow
        if let Some((chunk_idx, edge_idx)) = self.scan_overflow_for_edge_id(src_vid, edge_id) {
            // Capture live status before removal for set maintenance.
            let probe = {
                let chunks = self.overflow_chunks.get(src_vid).unwrap();
                chunks[chunk_idx].slot_at(edge_idx).unwrap()
            };
            let was_live = probe.delete_ts == Timestamp::MAX;
            // Detach the entry, capturing the ledger delta before the shard
            // borrow ends so the release routes through the ledger primitive.
            // `Some((freed, emptied))` when an empty chunk detached.
            let detached = if let Some(chunks) = self.overflow_chunks.get_mut(src_vid) {
                chunks[chunk_idx].remove(edge_idx);
                // Clean up empty chunk vectors to keep per-vertex chunk count bounded.
                if chunks[chunk_idx].is_empty() {
                    let removed = chunks.remove(chunk_idx);
                    Some((removed.capacity(), chunks.is_empty()))
                } else {
                    None
                }
            } else {
                return false;
            };
            if let Some((freed, emptied)) = detached {
                self.sub_capacity(freed);
                if emptied {
                    if was_live {
                        self.edge_count -= 1;
                        self.live_counts[src_idx] =
                            self.live_counts[src_idx].saturating_sub(1);
                    } else {
                        self.tombstone_counts[src_idx] =
                            self.tombstone_counts[src_idx].saturating_sub(1);
                    }
                    self.overflow_chunks.remove(src_vid);
                    self.rebuild_live_set_for_vertex(src_vid);
                    return true;
                }
            }
            if was_live {
                // In-chunk removal shifts later slots of the same chunk, so
                // stored overflow positions go stale: rebuild the indexed
                // row when one exists instead of dropping one key.
                if self.live_sets.get(&src_vid).is_some() {
                    self.rebuild_live_set_for_vertex(src_vid);
                }
                self.edge_count -= 1;
                self.live_counts[src_idx] =
                    self.live_counts[src_idx].saturating_sub(1);
            } else {
                self.tombstone_counts[src_idx] =
                    self.tombstone_counts[src_idx].saturating_sub(1);
            }
            return true;
        }

        false
    }

    /// Revert a deletion of an edge by edge id.
    ///
    /// Restores `delete_ts` to MAX when the entry was deleted at or before the
    /// given timestamp. Used to roll back the out-direction when the
    /// in-direction deletion fails.
    pub fn revert_delete_by_edge_id(
        &mut self,
        src_vid: u32,
        edge_id: EdgeId,
        ts: Timestamp,
    ) -> bool {
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return false;
        }

        // Scan primary on shared slices first, then revert one slot.
        let found = {
            let (hot, cold) = self.primary_pair(src_idx);
            hot.iter()
                .zip(cold.iter())
                .enumerate()
                .find_map(|(i, (h, c))| {
                    let probe = Nbr::from_parts(*h, *c);
                    (probe.edge_id == edge_id && can_revert_delete(&probe, ts)).then(|| (i, probe))
                })
        };
        if let Some((i, probe)) = found {
            let (start, _) = self.primary_window(src_idx);
            self.cold_list[start + i].delete_ts = Timestamp::MAX;
            self.edge_count += 1;
            self.invalidate_reuse_hint(src_idx);
            self.track_live_insert(
                src_vid,
                probe.endpoint,
                probe.rank,
                EdgePosition::Primary { slot: i as u32 },
            );
            return true;
        }

        // Scan overflow
        if let Some((chunk_idx, edge_idx)) = self.scan_overflow_for_edge_id(src_vid, edge_id) {
            let probe = {
                let chunks = self.overflow_chunks.get(src_vid).unwrap();
                chunks[chunk_idx].slot_at(edge_idx).unwrap()
            };
            if let Some(chunks) = self.overflow_chunks.get_mut(src_vid) {
                if can_revert_delete(&probe, ts) {
                    chunks[chunk_idx].cold_at_mut(edge_idx).unwrap().delete_ts = Timestamp::MAX;
                    self.edge_count += 1;
                    self.track_live_insert(
                        src_vid,
                        probe.endpoint,
                        probe.rank,
                        EdgePosition::Overflow {
                            chunk: chunk_idx as u32,
                            slot: edge_idx as u32,
                        },
                    );
                    return true;
                }
            }
        }

        false
    }

    /// Reserve primary capacity for a batch of `(src, count)` pairs.
    ///
    /// Each touched row is sized once at the packed density target for its
    /// current live width plus the incoming count, so the following inserts
    /// land in reserved gaps instead of allocating chunk by chunk. Rows not
    /// listed are untouched. Live sets are left alone; per-edge inserts keep
    /// maintaining them incrementally.
    ///
    /// Single-pass rebuild: all target capacities are computed first, then
    /// the primary list is rebuilt once front to back with fresh offsets.
    /// The previous per-row splice shifted the whole tail and re-fixed every
    /// offset per touched row (O(R x V)); this pass moves each reserved byte
    /// exactly once (O(total)). Untouched rows keep byte-identical content:
    /// reserved blocks are copied verbatim, only the trailing gap grows.
    pub fn reserve_for_batch(&mut self, counts: &[(u32, usize)]) {
        if counts.is_empty() {
            return;
        }
        // Aggregate duplicate entries per row so one row is sized once.
        // Ordered bulk writes arrive src-ordered, so the aggregation
        // re-sort is skipped when the input already arrives ordered.
        let mut aggregated: Vec<(u32, usize)> = Vec::with_capacity(counts.len());
        {
            let mut push = |src: u32, incoming: usize| {
                if let Some(last) = aggregated.last_mut() {
                    if last.0 == src {
                        last.1 = last.1.saturating_add(incoming);
                        return;
                    }
                }
                aggregated.push((src, incoming));
            };
            if counts.windows(2).all(|w| w[0].0 <= w[1].0) {
                for &(src, incoming) in counts {
                    push(src, incoming);
                }
            } else {
                let mut sorted = counts.to_vec();
                sorted.sort_unstable_by_key(|(src, _)| *src);
                for (src, incoming) in sorted {
                    push(src, incoming);
                }
            }
        }
        // Make every touched row addressable with a primary block first.
        for (src_vid, _) in &aggregated {
            let src_idx = *src_vid as usize;
            if src_idx >= self.vertex_capacity() {
                self.ensure_vertex_capacity(src_idx + 1);
            }
            if self.rows.primary_capacities[src_idx] == 0 {
                self.allocate_primary_block(src_idx);
            }
        }
        // Compute target capacities; track whether anything must grow.
        let mut new_caps = self.rows.primary_capacities.clone();
        let mut total_extra = 0usize;
        for (src_vid, incoming) in &aggregated {
            let src_idx = *src_vid as usize;
            let live = self.live_key_count(*src_vid);
            let want = Self::sized_row_capacity(live.saturating_add(*incoming));
            let have = new_caps[src_idx] as usize;
            if want > have {
                total_extra += want - have;
                new_caps[src_idx] = want as u32;
            }
        }
        if total_extra == 0 {
            return;
        }
        let rows = self.vertex_capacity();
        let total: usize = new_caps.iter().map(|&c| c as usize).sum();
        let mut new_hot = Vec::with_capacity(total);
        let mut new_cold = Vec::with_capacity(total);
        let mut new_offsets = vec![0u32; rows];
        for vid in 0..rows {
            new_offsets[vid] = new_hot.len() as u32;
            let base = self.rows.adj_offsets[vid] as usize;
            let cap = self.rows.primary_capacities[vid] as usize;
            let want = new_caps[vid] as usize;
            if cap > 0 {
                new_hot.extend_from_slice(&self.hot_list[base..base + cap]);
                new_cold.extend_from_slice(&self.cold_list[base..base + cap]);
            }
            if want > cap {
                new_hot.resize(new_hot.len() + (want - cap), HotNbr::dead_gap());
                new_cold.resize(new_cold.len() + (want - cap), ColdStamps::dead_gap());
            }
        }
        self.hot_list = new_hot;
        self.cold_list = new_cold;
        self.rows.adj_offsets = new_offsets;
        self.rows.primary_capacities = new_caps;
        self.add_capacity(total_extra);
        self.reset_reuse_hints();
    }

    /// Bulk insert pre-grouped edges: `groups` holds one `(src, batch)` per
    /// touched row with decoded `(endpoint, rank, edge_id, ts)` tuples.
    ///
    /// Each row is reserved once at the packed density target, written in a
    /// single pass, and has its live set rebuilt once instead of per edge.
    /// Duplicate keys inside or against the row are rejected before any write
    /// when `check_duplicates` is set; otherwise the caller guarantees
    /// uniqueness. Returns the inserted edge count.
    pub fn batch_put_edges(
        &mut self,
        groups: &[(u32, Vec<(u32, i64, EdgeId, Timestamp)>)],
        check_duplicates: bool,
    ) -> StorageResult<usize> {
        if check_duplicates {
            // Scratch buffers live for the whole batch: each row clears and
            // refills them instead of allocating per row, so duplicate
            // checking over many rows keeps peak allocation to one key
            // buffer plus one probe set.
            let mut keys: Vec<(u32, i64)> = Vec::new();
            let mut seen: HashSet<(u32, i64)> = HashSet::new();
            for (src_vid, batch) in groups {
                keys.clear();
                keys.extend(
                    batch
                        .iter()
                        .map(|(endpoint, rank, _, _)| (*endpoint, *rank)),
                );
                // Ordered batches arrive key-ordered; skip the re-sort when
                // the keys already arrive ordered.
                if !keys.windows(2).all(|w| w[0] <= w[1]) {
                    keys.sort_unstable();
                }
                for window in keys.windows(2) {
                    if window[0] == window[1] {
                        return Err(StorageError::edge_already_exists(format!(
                            "duplicate key in bulk batch for vertex {}",
                            src_vid
                        )));
                    }
                }
                // One row scan into the reused set: per-edge
                // `live_key_present` would rescan the row for every batch
                // edge on set-free rows, degrading bulk loads to
                // O(batch x degree). The transient set also absorbs batch
                // keys as they are checked, so intra-row conflicts against
                // both stored and staged edges surface in one pass.
                seen.clear();
                seen.reserve(batch.len());
                self.visit_physical(*src_vid, |nbr| {
                    if nbr.delete_ts == Timestamp::MAX {
                        seen.insert((nbr.endpoint, nbr.rank));
                    }
                    true
                });
                for (endpoint, rank, _, _) in batch {
                    if !seen.insert((*endpoint, *rank)) {
                        return Err(StorageError::edge_already_exists(format!(
                            "{} -> ({}, {})",
                            src_vid, endpoint, rank
                        )));
                    }
                }
            }
        }
        let counts: Vec<(u32, usize)> = groups
            .iter()
            .map(|(src, batch)| (*src, batch.len()))
            .collect();
        self.reserve_for_batch(&counts);
        let mut inserted = 0usize;
        for (src_vid, batch) in groups {
            let src_idx = *src_vid as usize;
            // Running live width per row: counted once up front, then bumped
            // per inserted edge, so overflow sizing never rescans the row.
            let mut live = self.live_key_count(*src_vid);
            for (endpoint, rank, edge_id, create_ts) in batch {
                let nbr = Nbr::with_create_ts(*endpoint, *rank, *edge_id, *create_ts);
                // Every batch edge is a live insert: keep the running width
                // exact for the overflow sizing below.
                live += 1;
                let degree = self.rows.degrees[src_idx] as usize;
                let cap = self.rows.primary_capacities[src_idx] as usize;
                if degree < cap {
                    let base = self.rows.adj_offsets[src_idx] as usize;
                    self.set_slot(base + degree, nbr);
                    self.rows.degrees[src_idx] += 1;
                } else {
                    self.append_overflow(*src_vid, nbr, live);
                }
                self.edge_count += 1;
                inserted += 1;
            }
            self.rebuild_live_set_for_vertex(*src_vid);
        }
        Ok(inserted)
    }

    /// Delete the edge at `position` when it still holds `expected` id.
    ///
    /// Stale positions (row moved since the read) are refused with `Ok(false)`
    /// instead of touching the wrong edge. Timestamp conflicts follow the
    /// same state machine as the edge-id path.
    pub fn delete_edge_at_position(
        &mut self,
        src_vid: u32,
        position: EdgePosition,
        expected: EdgeId,
        ts: Timestamp,
    ) -> StorageResult<bool> {
        match position {
            EdgePosition::Primary { slot } => {
                let src_idx = src_vid as usize;
                if src_idx >= self.vertex_capacity() {
                    return Ok(false);
                }
                if slot as usize >= self.rows.degrees[src_idx] as usize {
                    return Ok(false);
                }
                let idx = self.rows.adj_offsets[src_idx] as usize + slot as usize;
                let probe = match (self.hot_list.get(idx), self.cold_list.get(idx)) {
                    (Some(hot), Some(cold)) => Nbr::from_parts(*hot, *cold),
                    _ => return Ok(false),
                };
                if probe.edge_id != expected {
                    return Ok(false);
                }
                match decide_slot_delete(&probe, expected, ts)? {
                    DeleteSlotOutcome::AlreadyStamped => {
                        return Ok(false)
                    }
                    DeleteSlotOutcome::Stamped => {}
                }
                self.cold_list[idx].delete_ts = ts;
                self.edge_count -= 1;
                self.note_primary_tombstone(src_idx, slot as usize);
                self.track_live_remove(src_vid, probe.endpoint, probe.rank);
                Ok(true)
            }
            EdgePosition::Overflow { chunk, slot } => {
                let Some(chunks) = self.overflow_chunks.get_mut(src_vid) else {
                    return Ok(false);
                };
                let Some(chunk) = chunks.get_mut(chunk as usize) else {
                    return Ok(false);
                };
                let probe = match (chunk.hot_at(slot as usize), chunk.cold_at(slot as usize)) {
                    (Some(hot), Some(cold)) => Nbr::from_parts(hot, cold),
                    _ => return Ok(false),
                };
                if probe.edge_id != expected {
                    return Ok(false);
                }
                match decide_slot_delete(&probe, expected, ts)? {
                    DeleteSlotOutcome::AlreadyStamped => {
                        return Ok(false)
                    }
                    DeleteSlotOutcome::Stamped => {}
                }
                chunk.cold_at_mut(slot as usize).unwrap().delete_ts = ts;
                self.edge_count -= 1;
                self.track_live_remove(src_vid, probe.endpoint, probe.rank);
                Ok(true)
            }
        }
    }

    /// Revert the deletion at `position` when it still holds `expected` id
    /// and the tombstone predates `ts`. Stale positions are refused with
    /// `false`.
    pub fn revert_delete_at_position(
        &mut self,
        src_vid: u32,
        position: EdgePosition,
        expected: EdgeId,
        ts: Timestamp,
    ) -> bool {
        match position {
            EdgePosition::Primary { slot } => {
                let src_idx = src_vid as usize;
                if src_idx >= self.vertex_capacity() {
                    return false;
                }
                if slot as usize >= self.rows.degrees[src_idx] as usize {
                    return false;
                }
                let idx = self.rows.adj_offsets[src_idx] as usize + slot as usize;
                let probe = match (self.hot_list.get(idx), self.cold_list.get(idx)) {
                    (Some(hot), Some(cold)) => Nbr::from_parts(*hot, *cold),
                    _ => return false,
                };
                if probe.edge_id != expected || !can_revert_delete(&probe, ts) {
                    return false;
                }
                self.cold_list[idx].delete_ts = Timestamp::MAX;
                self.edge_count += 1;
                self.invalidate_reuse_hint(src_idx);
                self.track_live_insert(
                    src_vid,
                    probe.endpoint,
                    probe.rank,
                    EdgePosition::Primary { slot },
                );
                true
            }
            EdgePosition::Overflow { chunk, slot } => {
                let (chunk_idx, slot_idx) = (chunk, slot);
                let Some(chunks) = self.overflow_chunks.get_mut(src_vid) else {
                    return false;
                };
                let Some(chunk) = chunks.get_mut(chunk_idx as usize) else {
                    return false;
                };
                let probe = match (
                    chunk.hot_at(slot_idx as usize),
                    chunk.cold_at(slot_idx as usize),
                ) {
                    (Some(hot), Some(cold)) => Nbr::from_parts(hot, cold),
                    _ => return false,
                };
                if probe.edge_id != expected || !can_revert_delete(&probe, ts) {
                    return false;
                }
                chunk.cold_at_mut(slot_idx as usize).unwrap().delete_ts = Timestamp::MAX;
                self.edge_count += 1;
                self.track_live_insert(
                    src_vid,
                    probe.endpoint,
                    probe.rank,
                    EdgePosition::Overflow {
                        chunk: chunk_idx,
                        slot: slot_idx,
                    },
                );
                true
            }
        }
    }
}
