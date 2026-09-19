use super::super::csr_shared::{
    can_revert_delete, decide_slot_delete, decode_endpoint_pair, is_reclaimable_slot,
    DeleteSlotOutcome,
};
use super::super::{EdgeId, Nbr, Timestamp, VertexId};
use super::overflow::OVERFLOW_REPACK_CHUNKS_PER_VERTEX;
use super::MutableCsr;
use graphdb_core::{StorageError, StorageResult};

/// Hot-path bound for the tombstone-reuse scan. Rows wider than this leave
/// leftover reclaimable slots to the maintenance pass instead of scanning
/// the whole row on every insert.
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
    fn append_overflow(&mut self, src_vid: u32, nbr: Nbr) {
        let live_hint = self.live_key_count(src_vid).saturating_add(1);
        let chunk_edges = self.effective_chunk_edges(live_hint);
        // A chunk is full at its created capacity, so earlier small-tier
        // chunks never stretch into later tiers.
        let needs_chunk = self.overflow_chunks.get(&src_vid).is_none_or(|chunks| {
            chunks
                .last()
                .is_none_or(|chunk| chunk.len() >= chunk.capacity().max(1))
        });
        if needs_chunk {
            self.overflow_chunks
                .get_or_create(src_vid)
                .push(Vec::with_capacity(chunk_edges));
            let new_cap = self
                .overflow_chunks
                .get(&src_vid)
                .and_then(|chunks| chunks.last())
                .map_or(chunk_edges, Vec::capacity);
            self.add_capacity(new_cap);
        }
        if let Some(chunks) = self.overflow_chunks.get_mut(&src_vid) {
            if let Some(chunk) = chunks.last_mut() {
                chunk.push(nbr);
            }
        }
        if nbr.delete_ts == Timestamp::MAX {
            self.track_live_insert(src_vid, nbr.endpoint, nbr.rank);
        }
        // Per-vertex overflow bound: past the limit the row is repacked into
        // graded chunks whether or not it holds dead entries, so skewed rows
        // cannot grow unbounded pointer chains. The repack preserves every
        // entry (no watermark here); watermark-confirmed reclaim runs
        // through the vertex-level reporting passes. Repacking collapses the
        // chain back to one or two chunks, so the cost stays amortized.
        if let Some(chunks) = self.overflow_chunks.get(&src_vid) {
            if chunks.len() > OVERFLOW_REPACK_CHUNKS_PER_VERTEX {
                let mut noop = |_id: EdgeId, _ts: Timestamp| {};
                self.compact_overflow_for_vertex(src_vid, Timestamp::MAX, &mut noop);
            }
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
        if self.primary_capacities[src_idx] == 0 {
            self.allocate_primary_block(src_idx);
        }

        // Duplicate check via the single live set covering primary and
        // overflow. The set is authoritative: it is rebuilt on load and
        // compact and updated on every write, so no linear scan fallback
        // exists.
        if self.live_key_present(src_vid, decoded_endpoint, decoded_rank) {
            return Err(StorageError::edge_already_exists(format!(
                "{} -> {:?}",
                src_vid, dst
            )));
        }

        // Record create_ts in the Nbr before writing
        let nbr_with_ts = Nbr::with_create_ts(decoded_endpoint, decoded_rank, edge_id, ts);

        // Steady-state gap fill: primary trailing gaps are filled first
        // even when overflow exists, so everyday writes land in row gaps.
        // Row rebalances that pull overflow back into freed gaps run on the
        // maintenance path, never on this hot path, keeping inserts O(1).
        let degree = self.degrees[src_idx] as usize;
        if degree < self.primary_capacities[src_idx] as usize {
            let base = self.adj_offsets[src_idx] as usize;
            self.nbr_list[base + degree] = nbr_with_ts;
            self.degrees[src_idx] += 1;
            self.track_live_insert(src_vid, decoded_endpoint, decoded_rank);
            self.edge_count += 1;
            return Ok(());
        }

        // Full row: with a watermark-derived reuse cutoff, reclaim one
        // GC-eligible primary tombstone in place instead of growing
        // overflow. The eligibility predicate matches the maintenance
        // compaction exactly, so the hot path never drops a tombstone the
        // reclaim pass would keep. Overflow tombstones stay for the region
        // compaction; the scan is bounded to a fixed prefix so high-degree
        // rows never pay O(degree) per insert.
        if self.tombstone_reuse_cutoff != Timestamp::MAX {
            let base = self.adj_offsets[src_idx] as usize;
            let cutoff = self.tombstone_reuse_cutoff;
            let bound = degree.min(TOMBSTONE_REUSE_SCAN_BOUND);
            for i in 0..bound {
                let reclaimable = self
                    .nbr_list
                    .get(base + i)
                    .is_some_and(|nbr| is_reclaimable_slot(nbr, cutoff));
                if reclaimable {
                    self.nbr_list[base + i] = nbr_with_ts;
                    self.track_live_insert(src_vid, decoded_endpoint, decoded_rank);
                    self.edge_count += 1;
                    return Ok(());
                }
            }
        }

        self.append_overflow(src_vid, nbr_with_ts);
        self.edge_count += 1;
        Ok(())
    }

    /// Locate one overflow entry by edge id, stopping at the first match.
    ///
    /// Point-lookup fast path: never builds a match vector, so hot single
    /// deletes pay no heap allocation. Callers needing every match use the
    /// in-place stamping passes instead.
    fn scan_overflow_for_edge_id(&self, src_vid: u32, edge_id: EdgeId) -> Option<(usize, usize)> {
        let chunks = self.overflow_chunks.get(&src_vid)?;
        for (chunk_idx, chunk) in chunks.iter().enumerate() {
            for (edge_idx, nbr) in chunk.iter().enumerate() {
                if nbr.edge_id == edge_id {
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

        // Scan primary
        let degree = self.degrees[src_idx] as usize;
        let offset = self.adj_offsets[src_idx] as usize;
        for i in 0..degree {
            let nbr = &mut self.nbr_list[offset + i];
            if nbr.edge_id == edge_id {
                match decide_slot_delete(nbr, edge_id, ts)? {
                    DeleteSlotOutcome::AlreadyStamped | DeleteSlotOutcome::NotYetCreated => {
                        return Ok(false);
                    }
                    DeleteSlotOutcome::Stamped => {}
                }
                let (endpoint, rank) = (nbr.endpoint, nbr.rank);
                nbr.delete_ts = ts;
                self.edge_count -= 1;
                self.track_live_remove(src_vid, endpoint, rank);
                return Ok(true);
            }
        }

        // Scan overflow
        if let Some((chunk_idx, edge_idx)) = self.scan_overflow_for_edge_id(src_vid, edge_id) {
            // Capture endpoint/rank before mutable borrow ends for live set update.
            let (endpoint, rank) = {
                let chunks = self.overflow_chunks.get(&src_vid).unwrap();
                let n = &chunks[chunk_idx][edge_idx];
                (n.endpoint, n.rank)
            };
            if let Some(chunks) = self.overflow_chunks.get_mut(&src_vid) {
                let nbr = &mut chunks[chunk_idx][edge_idx];
                match decide_slot_delete(nbr, edge_id, ts)? {
                    DeleteSlotOutcome::AlreadyStamped | DeleteSlotOutcome::NotYetCreated => {
                        return Ok(false);
                    }
                    DeleteSlotOutcome::Stamped => {}
                }
                nbr.delete_ts = ts;
                self.edge_count -= 1;
                self.track_live_remove(src_vid, endpoint, rank);
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

        // Stamp primary matches in place.
        let degree = self.degrees[src_idx] as usize;
        let offset = self.adj_offsets[src_idx] as usize;
        for i in 0..degree {
            let nbr = &mut self.nbr_list[offset + i];
            if nbr.endpoint == decoded_endpoint
                && nbr.rank == decoded_rank
                && nbr.delete_ts == Timestamp::MAX
                && nbr.create_ts <= ts
            {
                let edge_id = nbr.edge_id;
                nbr.delete_ts = ts;
                self.edge_count -= 1;
                on_deleted(edge_id, EdgePosition::Primary { slot: i as u32 });
                deleted += 1;
            }
        }

        // Stamp overflow matches in the same pass.
        if let Some(chunks) = self.overflow_chunks.get_mut(&src_vid) {
            for (chunk_idx, chunk) in chunks.iter_mut().enumerate() {
                for (slot_idx, nbr) in chunk.iter_mut().enumerate() {
                    if nbr.endpoint == decoded_endpoint
                        && nbr.rank == decoded_rank
                        && nbr.delete_ts == Timestamp::MAX
                        && nbr.create_ts <= ts
                    {
                        let edge_id = nbr.edge_id;
                        nbr.delete_ts = ts;
                        self.edge_count -= 1;
                        on_deleted(
                            edge_id,
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
        if src_idx >= self.vertex_capacity() || self.primary_capacities[src_idx] == 0 {
            return Ok(false);
        }
        if offset as usize >= self.degrees[src_idx] as usize {
            return Ok(false);
        }
        let idx = self.adj_offsets[src_idx] as usize + offset as usize;
        if idx >= self.nbr_list.len() {
            return Ok(false);
        }
        let nbr = &mut self.nbr_list[idx];
        // Tombstone timestamp check mirrors the other delete entries: a
        // repeat at the same timestamp stays idempotent while a different
        // timestamp surfaces as a write-write conflict instead of folding
        // into not-found.
        if nbr.delete_ts != Timestamp::MAX {
            match decide_slot_delete(nbr, nbr.edge_id, ts)? {
                DeleteSlotOutcome::AlreadyStamped | DeleteSlotOutcome::NotYetCreated => {
                    return Ok(false);
                }
                DeleteSlotOutcome::Stamped => {
                    return Ok(false);
                }
            }
        }
        if nbr.delete_ts == Timestamp::MAX {
            let create_ts = nbr.create_ts;
            if create_ts <= ts {
                let (endpoint, rank) = (nbr.endpoint, nbr.rank);
                nbr.delete_ts = ts;
                self.edge_count -= 1;
                self.track_live_remove(src_vid, endpoint, rank);
                return Ok(true);
            }
        }
        Ok(false)
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
        if src_idx >= self.vertex_capacity() || self.primary_capacities[src_idx] == 0 {
            return false;
        }
        if offset as usize >= self.degrees[src_idx] as usize {
            return false;
        }

        let base_offset = self.adj_offsets[src_idx] as usize;
        let idx = base_offset + offset as usize;

        if idx >= self.nbr_list.len() {
            return false;
        }

        let nbr = &mut self.nbr_list[idx];
        // Only revert deletions that happened at or before rollback time.
        // Prevents rolling back deletions that occur after the rollback point.
        if can_revert_delete(nbr, ts) {
            let (endpoint, rank) = (nbr.endpoint, nbr.rank);
            nbr.delete_ts = Timestamp::MAX;
            self.edge_count += 1;
            self.track_live_insert(src_vid, endpoint, rank);
            return true;
        }
        false
    }

    /// Physically remove an edge by edge id from primary or overflow.
    ///
    /// Reclaims the slot and updates degree/edge count; no tombstone trace is
    /// left behind. Used to roll back the out-direction when the in-direction
    /// insertion fails.
    pub fn remove_edge(&mut self, src_vid: u32, edge_id: EdgeId) -> bool {
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return false;
        }

        // Scan primary
        let degree = self.degrees[src_idx] as usize;
        let offset = self.adj_offsets[src_idx] as usize;
        for i in 0..degree {
            if self.nbr_list[offset + i].edge_id == edge_id {
                let was_live = self.nbr_list[offset + i].delete_ts == Timestamp::MAX;
                let (endpoint, rank) = {
                    let n = &self.nbr_list[offset + i];
                    (n.endpoint, n.rank)
                };
                // Shift left to close the gap, then decrement the degree.
                for j in i..degree - 1 {
                    self.nbr_list[offset + j] = self.nbr_list[offset + j + 1];
                }
                self.degrees[src_idx] -= 1;
                if was_live {
                    self.track_live_remove(src_vid, endpoint, rank);
                    self.edge_count -= 1;
                }
                return true;
            }
        }

        // Scan overflow
        if let Some((chunk_idx, edge_idx)) = self.scan_overflow_for_edge_id(src_vid, edge_id) {
            // Capture live status before removal for set maintenance.
            let was_live = {
                let chunks = self.overflow_chunks.get(&src_vid).unwrap();
                chunks[chunk_idx][edge_idx].delete_ts == Timestamp::MAX
            };
            let (endpoint, rank) = {
                let chunks = self.overflow_chunks.get(&src_vid).unwrap();
                let n = &chunks[chunk_idx][edge_idx];
                (n.endpoint, n.rank)
            };
            // Detach the entry, capturing the ledger delta before the shard
            // borrow ends so the release routes through the ledger primitive.
            // `Some((freed, emptied))` when an empty chunk detached.
            let detached = if let Some(chunks) = self.overflow_chunks.get_mut(&src_vid) {
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
                    // Drop the per-vertex entry so later lookups stay constant time.
                    // The unified live set still covers primary rows, so
                    // rebuild it instead of dropping the whole entry.
                    if was_live {
                        self.edge_count -= 1;
                    }
                    self.overflow_chunks.remove(&src_vid);
                    self.rebuild_live_set_for_vertex(src_vid);
                    return true;
                }
            }
            if was_live {
                self.track_live_remove(src_vid, endpoint, rank);
                self.edge_count -= 1;
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

        // Scan primary
        let degree = self.degrees[src_idx] as usize;
        let offset = self.adj_offsets[src_idx] as usize;
        for i in 0..degree {
            let nbr = &mut self.nbr_list[offset + i];
            if nbr.edge_id == edge_id && can_revert_delete(nbr, ts) {
                let (endpoint, rank) = (nbr.endpoint, nbr.rank);
                nbr.delete_ts = Timestamp::MAX;
                self.edge_count += 1;
                self.track_live_insert(src_vid, endpoint, rank);
                return true;
            }
        }

        // Scan overflow
        if let Some((chunk_idx, edge_idx)) = self.scan_overflow_for_edge_id(src_vid, edge_id) {
            let (endpoint, rank) = {
                let chunks = self.overflow_chunks.get(&src_vid).unwrap();
                let n = &chunks[chunk_idx][edge_idx];
                (n.endpoint, n.rank)
            };
            if let Some(chunks) = self.overflow_chunks.get_mut(&src_vid) {
                let nbr = &mut chunks[chunk_idx][edge_idx];
                if can_revert_delete(nbr, ts) {
                    nbr.delete_ts = Timestamp::MAX;
                    self.edge_count += 1;
                    self.track_live_insert(src_vid, endpoint, rank);
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
    pub fn reserve_for_batch(&mut self, counts: &[(u32, usize)]) {
        for (src_vid, incoming) in counts {
            let src_idx = *src_vid as usize;
            if src_idx >= self.vertex_capacity() {
                self.ensure_vertex_capacity(src_idx + 1);
            }
            if self.primary_capacities[src_idx] == 0 {
                self.allocate_primary_block(src_idx);
            }
            let live = self.live_key_count(*src_vid);
            let want = Self::sized_row_capacity(live.saturating_add(*incoming));
            let have = self.primary_capacities[src_idx] as usize;
            if want > have {
                let base = self.adj_offsets[src_idx] as usize;
                let extra = want - have;
                let insert_at = base + have;
                self.nbr_list.splice(
                    insert_at..insert_at,
                    std::iter::repeat(Nbr::dead_gap()).take(extra),
                );
                for offset in self.adj_offsets.iter_mut() {
                    if *offset as usize > base {
                        *offset = offset.saturating_add(extra as u32);
                    }
                }
                self.primary_capacities[src_idx] = want as u32;
                self.add_capacity(extra);
            }
        }
    }

    /// Bulk insert pre-grouped edges: `groups` holds one `(src, batch)` per
    /// touched row with decoded `(endpoint, rank, edge_id, create_ts)` tuples.
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
            for (src_vid, batch) in groups {
                let mut keys: Vec<(u32, i64)> = batch
                    .iter()
                    .map(|(endpoint, rank, _, _)| (*endpoint, *rank))
                    .collect();
                keys.sort_unstable();
                for window in keys.windows(2) {
                    if window[0] == window[1] {
                        return Err(StorageError::edge_already_exists(format!(
                            "duplicate key in bulk batch for vertex {}",
                            src_vid
                        )));
                    }
                }
                for (endpoint, rank, _, _) in batch {
                    if self.live_key_present(*src_vid, *endpoint, *rank) {
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
            for (endpoint, rank, edge_id, create_ts) in batch {
                let nbr = Nbr::with_create_ts(*endpoint, *rank, *edge_id, *create_ts);
                let degree = self.degrees[src_idx] as usize;
                let cap = self.primary_capacities[src_idx] as usize;
                if degree < cap {
                    let base = self.adj_offsets[src_idx] as usize;
                    self.nbr_list[base + degree] = nbr;
                    self.degrees[src_idx] += 1;
                } else {
                    self.append_overflow(*src_vid, nbr);
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
                if slot as usize >= self.degrees[src_idx] as usize {
                    return Ok(false);
                }
                let idx = self.adj_offsets[src_idx] as usize + slot as usize;
                let nbr = &mut self.nbr_list[idx];
                if nbr.edge_id != expected {
                    return Ok(false);
                }
                match decide_slot_delete(nbr, expected, ts)? {
                    DeleteSlotOutcome::AlreadyStamped | DeleteSlotOutcome::NotYetCreated => {
                        return Ok(false)
                    }
                    DeleteSlotOutcome::Stamped => {}
                }
                let (endpoint, rank) = (nbr.endpoint, nbr.rank);
                nbr.delete_ts = ts;
                self.edge_count -= 1;
                self.track_live_remove(src_vid, endpoint, rank);
                Ok(true)
            }
            EdgePosition::Overflow { chunk, slot } => {
                let Some(chunks) = self.overflow_chunks.get_mut(&src_vid) else {
                    return Ok(false);
                };
                let Some(chunk) = chunks.get_mut(chunk as usize) else {
                    return Ok(false);
                };
                let Some(nbr) = chunk.get_mut(slot as usize) else {
                    return Ok(false);
                };
                if nbr.edge_id != expected {
                    return Ok(false);
                }
                match decide_slot_delete(nbr, expected, ts)? {
                    DeleteSlotOutcome::AlreadyStamped | DeleteSlotOutcome::NotYetCreated => {
                        return Ok(false)
                    }
                    DeleteSlotOutcome::Stamped => {}
                }
                let (endpoint, rank) = (nbr.endpoint, nbr.rank);
                nbr.delete_ts = ts;
                self.edge_count -= 1;
                self.track_live_remove(src_vid, endpoint, rank);
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
                if slot as usize >= self.degrees[src_idx] as usize {
                    return false;
                }
                let idx = self.adj_offsets[src_idx] as usize + slot as usize;
                let nbr = &mut self.nbr_list[idx];
                if nbr.edge_id != expected || !can_revert_delete(nbr, ts) {
                    return false;
                }
                let (endpoint, rank) = (nbr.endpoint, nbr.rank);
                nbr.delete_ts = Timestamp::MAX;
                self.edge_count += 1;
                self.track_live_insert(src_vid, endpoint, rank);
                true
            }
            EdgePosition::Overflow { chunk, slot } => {
                let Some(chunks) = self.overflow_chunks.get_mut(&src_vid) else {
                    return false;
                };
                let Some(chunk) = chunks.get_mut(chunk as usize) else {
                    return false;
                };
                let Some(nbr) = chunk.get_mut(slot as usize) else {
                    return false;
                };
                if nbr.edge_id != expected || !can_revert_delete(nbr, ts) {
                    return false;
                }
                let (endpoint, rank) = (nbr.endpoint, nbr.rank);
                nbr.delete_ts = Timestamp::MAX;
                self.edge_count += 1;
                self.track_live_insert(src_vid, endpoint, rank);
                true
            }
        }
    }
}
