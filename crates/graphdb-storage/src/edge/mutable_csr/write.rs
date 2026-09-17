use std::collections::HashSet;
use std::sync::atomic::Ordering;

use super::MutableCsr;
use super::overflow::OVERFLOW_REPACK_CHUNKS_PER_VERTEX;
use super::super::csr_shared::{
    DeleteSlotOutcome, can_revert_delete, decide_slot_delete, decode_endpoint_pair,
};
use super::super::{EdgeId, Nbr, Timestamp, VertexId};
use graphdb_core::{StorageError, StorageResult};

impl MutableCsr {
    fn append_overflow(&mut self, src_vid: u32, nbr: Nbr) {
        let live_hint = self
            .live_sets
            .get(&src_vid)
            .map_or(1, HashSet::len)
            .saturating_add(1);
        let chunk_edges = self.effective_chunk_edges(live_hint);
        let chunks = self.overflow_chunks.get_or_create(src_vid);
        // A chunk is full at its created capacity, so earlier small-tier
        // chunks never stretch into later tiers.
        let needs_chunk = chunks
            .last()
            .is_none_or(|chunk| chunk.len() >= chunk.capacity().max(1));
        if needs_chunk {
            chunks.push(Vec::with_capacity(chunk_edges));
            let new_cap = chunks.last().map_or(chunk_edges, Vec::capacity);
            self.total_edge_capacity = self.total_edge_capacity.saturating_add(new_cap);
        }
        if let Some(chunk) = chunks.last_mut() {
            chunk.push(nbr);
        }
        if nbr.delete_ts == Timestamp::MAX {
            self.track_live_insert(src_vid, nbr.endpoint, nbr.rank);
        }
        // Per-vertex overflow bound: single benchmarked threshold. Past the
        // limit with dead entries the row is repacked; past the limit with
        // only live entries the row waits for a region or full compaction
        // instead of repeatedly repacking live data.
        if let Some(chunks) = self.overflow_chunks.get(&src_vid) {
            if chunks.len() > OVERFLOW_REPACK_CHUNKS_PER_VERTEX {
                let dead = chunks
                    .iter()
                    .flat_map(|c| c.iter())
                    .filter(|nbr| nbr.delete_ts != Timestamp::MAX)
                    .count();
                if dead > 0 {
                    // Hot write path carries no watermark capture, so repack
                    // only moves entries and preserves pinned tombstones
                    // without dropping. Watermark-confirmed reclaim runs
                    // through the vertex-level reporting passes that promote
                    // deletions to the authority.
                    let mut noop = |_id: EdgeId, _ts: Timestamp| {};
                    self.compact_overflow_for_vertex(src_vid, Timestamp::MAX, &mut noop);
                } else {
                    log::debug!(
                        "MutableCsr vertex {} holds {} overflow chunks of live entries; row rebalance or compaction will merge them",
                        src_vid,
                        chunks.len()
                    );
                }
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
        if self
            .live_sets
            .get(&src_vid)
            .is_some_and(|set| set.contains(&(decoded_endpoint, decoded_rank)))
        {
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
            self.edge_count.fetch_add(1, Ordering::Relaxed);
            return Ok(());
        }

        self.append_overflow(src_vid, nbr_with_ts);
        self.edge_count.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn find_overflow_positions<F>(&self, src_vid: u32, mut matches: F) -> Vec<(usize, usize)>
    where
        F: FnMut(&Nbr) -> bool,
    {
        let mut result = Vec::new();
        let Some(chunks) = self.overflow_chunks.get(&src_vid) else {
            return result;
        };
        for (chunk_idx, chunk) in chunks.iter().enumerate() {
            for (edge_idx, nbr) in chunk.iter().enumerate() {
                if matches(nbr) {
                    result.push((chunk_idx, edge_idx));
                }
            }
        }
        result
    }

    fn scan_overflow_for_edge_id(&self, src_vid: u32, edge_id: EdgeId) -> Option<(usize, usize)> {
        self.find_overflow_positions(src_vid, |nbr| nbr.edge_id == edge_id)
            .into_iter()
            .next()
    }

    fn scan_overflow_for_dst(&self, src_vid: u32, dst: VertexId) -> Vec<(usize, usize)> {
        let (decoded_endpoint, decoded_rank) = decode_endpoint_pair(dst);
        self.find_overflow_positions(src_vid, |nbr| {
            nbr.endpoint == decoded_endpoint && nbr.rank == decoded_rank
        })
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
                self.edge_count.fetch_sub(1, Ordering::Relaxed);
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
                self.edge_count.fetch_sub(1, Ordering::Relaxed);
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
    pub fn delete_edge_by_dst(&mut self, src_vid: u32, dst: VertexId, ts: Timestamp) -> usize {
        let (decoded_endpoint, decoded_rank) = decode_endpoint_pair(dst);
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return 0;
        }

        let mut deleted = 0usize;

        // Scan primary
        let degree = self.degrees[src_idx] as usize;
        let offset = self.adj_offsets[src_idx] as usize;
        for i in 0..degree {
            let nbr = &mut self.nbr_list[offset + i];
            if nbr.endpoint == decoded_endpoint
                && nbr.rank == decoded_rank
                && nbr.delete_ts == Timestamp::MAX
            {
                let create_ts = nbr.create_ts;
                if create_ts <= ts {
                    nbr.delete_ts = ts;
                    self.edge_count.fetch_sub(1, Ordering::Relaxed);
                    self.track_live_remove(src_vid, decoded_endpoint, decoded_rank);
                    deleted += 1;
                }
            }
        }

        // Scan overflow
        let indices = self.scan_overflow_for_dst(src_vid, dst);
        // Collect endpoints for set removal before mutable borrow.
        let mut overflow_deleted_endpoints: Vec<(u32, i64)> = Vec::new();
        if let Some(chunks) = self.overflow_chunks.get_mut(&src_vid) {
            for (chunk_idx, edge_idx) in indices {
                let nbr = &mut chunks[chunk_idx][edge_idx];
                if nbr.delete_ts == Timestamp::MAX {
                    let create_ts = nbr.create_ts;
                    if create_ts <= ts {
                        let ep = nbr.endpoint;
                        let rk = nbr.rank;
                        nbr.delete_ts = ts;
                        self.edge_count.fetch_sub(1, Ordering::Relaxed);
                        overflow_deleted_endpoints.push((ep, rk));
                        deleted += 1;
                    }
                }
            }
        }
        for (ep, rk) in overflow_deleted_endpoints {
            self.track_live_remove(src_vid, ep, rk);
        }

        deleted
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
                self.edge_count.fetch_sub(1, Ordering::Relaxed);
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
            self.edge_count.fetch_add(1, Ordering::Relaxed);
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
                    self.edge_count.fetch_sub(1, Ordering::Relaxed);
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
            if let Some(chunks) = self.overflow_chunks.get_mut(&src_vid) {
                chunks[chunk_idx].remove(edge_idx);
                // Clean up empty chunk vectors to keep per-vertex chunk count bounded.
                if chunks[chunk_idx].is_empty() {
                    let removed = chunks.remove(chunk_idx);
                    self.total_edge_capacity =
                        self.total_edge_capacity.saturating_sub(removed.capacity());
                    if chunks.is_empty() {
                        // Drop the per-vertex entry so later lookups stay constant time.
                        // The unified live set still covers primary rows, so
                        // rebuild it instead of dropping the whole entry.
                        if was_live {
                            self.edge_count.fetch_sub(1, Ordering::Relaxed);
                        }
                        self.overflow_chunks.remove(&src_vid);
                        self.rebuild_live_set_for_vertex(src_vid);
                        return true;
                    }
                }
                if was_live {
                    self.track_live_remove(src_vid, endpoint, rank);
                    self.edge_count.fetch_sub(1, Ordering::Relaxed);
                }
                return true;
            }
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
                self.edge_count.fetch_add(1, Ordering::Relaxed);
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
                    self.edge_count.fetch_add(1, Ordering::Relaxed);
                    self.track_live_insert(src_vid, endpoint, rank);
                    return true;
                }
            }
        }

        false
    }
}
