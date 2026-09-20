//! Single Mutable CSR Implementation
//!
//! Optimized CSR for scenarios where each vertex has at most one outgoing edge.
//! Uses a simple array instead of offset/degree arrays, providing O(1) access.
//!
//! Use cases:
//! - "Spouse" relationship (one-to-one)
//! - "Current employer" relationship
//! - Any single-edge semantic relationship
//!
//! Contract, unified with the table layer:
//! - Each vertex holds at most one live edge. A second live insert into an
//!   occupied slot is rejected with a conflict error, never silently
//!   overwritten. Callers must delete before rebuilding.
//! - Deletion requires the exact edge id; endpoint plus rank addressing goes
//!   through `delete_edge_by_dst`. No wildcard edge id is supported.
//! - `delete_edge_by_dst` deletes the single matching live entry and reports
//!   the deleted count (0 or 1) so callers can reconcile.
//! - Rebuilding a tombstoned slot needs no timestamp ordering at this layer;
//!   snapshot visibility is decided by the version authority above.
//!
//! If concurrent writes are needed, use MutableCsr (accepts multiple edges).

use crate::persistence::read_u64_le;
use graphdb_core::{StorageError, StorageResult};

use super::csr_shared::{
    can_revert_delete, decide_slot_delete, decode_endpoint_pair, grown_vertex_capacity,
    is_reclaimable_cold, DeleteSlotOutcome, DEFAULT_VERTEX_CAPACITY,
};
use super::mutable_csr::serialization::{
    decode_topology_i64_column, decode_topology_u32_column, decode_topology_u64_column,
    encode_topology_i64_column, encode_topology_u32_column, encode_topology_u64_column,
};
use super::{
    ColdStamps, CsrBase, EdgeId, HotNbr, MutableCsrTrait, Nbr, Timestamp, VertexId, INVALID_EDGE_ID,
};

/// Unassigned single slot: no edge id, never alive at any timestamp.
fn empty_slot() -> Nbr {
    Nbr::dead_gap()
}

pub struct SingleMutableCsr {
    hot_slots: Vec<HotNbr>,
    cold_slots: Vec<ColdStamps>,
    edge_count: u64,
}

impl Clone for SingleMutableCsr {
    fn clone(&self) -> Self {
        Self {
            hot_slots: self.hot_slots.clone(),
            cold_slots: self.cold_slots.clone(),
            edge_count: self.edge_count,
        }
    }
}

impl std::fmt::Debug for SingleMutableCsr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SingleMutableCsr")
            .field("vertex_capacity", &self.vertex_capacity())
            .field("edge_count", &self.edge_count)
            .finish_non_exhaustive()
    }
}

impl SingleMutableCsr {
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_VERTEX_CAPACITY)
    }

    pub fn with_capacity(vertex_capacity: usize) -> Self {
        let vertex_cap = vertex_capacity.max(1);

        Self {
            hot_slots: vec![HotNbr::dead_gap(); vertex_cap],
            cold_slots: vec![ColdStamps::dead_gap(); vertex_cap],
            edge_count: 0,
        }
    }

    pub fn vertex_capacity(&self) -> usize {
        self.hot_slots.len()
    }

    pub fn edge_count(&self) -> u64 {
        self.edge_count
    }

    pub fn resize(&mut self, new_vertex_capacity: usize) {
        if new_vertex_capacity <= self.vertex_capacity() {
            return;
        }

        let additional = new_vertex_capacity - self.vertex_capacity();
        self.hot_slots
            .extend(std::iter::repeat_n(HotNbr::dead_gap(), additional));
        self.cold_slots
            .extend(std::iter::repeat_n(ColdStamps::dead_gap(), additional));
    }

    pub fn ensure_vertex_capacity(&mut self, min_capacity: usize) {
        if min_capacity > self.vertex_capacity() {
            self.resize(grown_vertex_capacity(min_capacity));
        }
    }

    /// Assembled slot copy at one index.
    fn slot_at(&self, idx: usize) -> Option<Nbr> {
        Some(Nbr::from_parts(
            *self.hot_slots.get(idx)?,
            *self.cold_slots.get(idx)?,
        ))
    }

    /// Paired writer for one slot: hot and cold stay in lockstep.
    fn set_slot(&mut self, idx: usize, nbr: Nbr) {
        self.hot_slots[idx] = nbr.hot();
        self.cold_slots[idx] = nbr.cold();
        debug_assert_eq!(self.hot_slots.len(), self.cold_slots.len());
    }

    /// Visit the single hot half of one vertex without allocating.
    pub fn visit_hot<F>(&self, src: u32, mut f: F)
    where
        F: FnMut(HotNbr) -> bool,
    {
        if let Some(hot) = self.hot_slots.get(src as usize) {
            if hot.edge_id != INVALID_EDGE_ID {
                let _ = f(*hot);
            }
        }
    }

    pub fn insert_edge(
        &mut self,
        src: u32,
        dst: VertexId,
        edge_id: EdgeId,
        _ts: Timestamp,
    ) -> StorageResult<()> {
        let src_idx = src as usize;

        if src_idx >= self.vertex_capacity() {
            self.ensure_vertex_capacity(src_idx + 1);
        }

        let hot = &self.hot_slots[src_idx];
        let cold = &self.cold_slots[src_idx];

        // Physical uniqueness only: a live slot rejects the second insert
        // regardless of timestamp, while a tombstoned or empty slot accepts
        // any timestamp. Snapshot visibility is decided by the version
        // authority above this layer.
        if cold.is_live() && hot.edge_id != INVALID_EDGE_ID {
            return Err(StorageError::conflict(format!(
                "[SingleMutableCsr] insert conflict on src={}: slot holds live edge {:?}",
                src, hot.edge_id
            )));
        }

        let was_empty = hot.edge_id == INVALID_EDGE_ID || !cold.is_live();
        let (decoded_endpoint, rank) = decode_endpoint_pair(dst);
        self.hot_slots[src_idx] = HotNbr {
            endpoint: decoded_endpoint,
            rank,
            edge_id,
        };
        self.cold_slots[src_idx] = ColdStamps {
            delete_ts: Timestamp::MAX,
        };

        if was_empty {
            self.edge_count += 1;
        }

        Ok(())
    }

    pub fn delete_edge(&mut self, src: u32, edge_id: EdgeId, ts: Timestamp) -> StorageResult<bool> {
        let src_idx = src as usize;

        if src_idx >= self.vertex_capacity() {
            return Ok(false);
        }

        let probe = match self.slot_at(src_idx) {
            Some(probe) => probe,
            None => return Ok(false),
        };

        if probe.edge_id == INVALID_EDGE_ID {
            return Ok(false);
        }

        if probe.edge_id != edge_id {
            return Ok(false);
        }

        if matches!(
            decide_slot_delete(&probe, probe.edge_id, ts)?,
            DeleteSlotOutcome::AlreadyStamped
        ) {
            return Ok(false);
        }

        self.cold_slots[src_idx].delete_ts = ts;
        self.edge_count -= 1;
        Ok(true)
    }

    /// Delete the single matching live entry for full-match endpoint semantics.
    ///
    /// Returns the deleted count (0 or 1) so callers can reconcile.
    /// Reporting variant stamps the slot and hands the id to `on_deleted`
    /// in the same step, so no second lookup is needed.
    pub fn delete_edge_by_dst_reporting(
        &mut self,
        src: u32,
        dst: VertexId,
        ts: Timestamp,
        on_deleted: &mut dyn FnMut(EdgeId),
    ) -> usize {
        let src_idx = src as usize;

        if src_idx >= self.vertex_capacity() {
            return 0;
        }

        let (dst_ep, dst_rank) = decode_endpoint_pair(dst);
        let probe = match self.slot_at(src_idx) {
            Some(probe) => probe,
            None => return 0,
        };

        if probe.edge_id == INVALID_EDGE_ID
            || probe.endpoint != dst_ep
            || probe.rank != dst_rank
            || probe.delete_ts < Timestamp::MAX
        {
            return 0;
        }

        self.cold_slots[src_idx].delete_ts = ts;
        self.edge_count -= 1;
        on_deleted(probe.edge_id);
        1
    }

    /// Delete the single matching live entry for full-match endpoint semantics.
    ///
    /// Returns the deleted count (0 or 1) so callers can reconcile.
    pub fn delete_edge_by_dst(&mut self, src: u32, dst: VertexId, ts: Timestamp) -> usize {
        let mut noop = |_: EdgeId| {};
        self.delete_edge_by_dst_reporting(src, dst, ts, &mut noop)
    }

    pub fn get_edge(&self, src: u32, dst: VertexId, ts: Timestamp) -> Option<Nbr> {
        let src_idx = src as usize;

        if src_idx >= self.vertex_capacity() {
            return None;
        }

        let (dst_ep, dst_rank) = decode_endpoint_pair(dst);
        let probe = match self.slot_at(src_idx) {
            Some(probe) => probe,
            None => return None,
        };

        if !probe.is_alive_at(ts) {
            return None;
        }

        if probe.endpoint == dst_ep && probe.rank == dst_rank {
            Some(probe)
        } else {
            None
        }
    }

    pub fn revert_delete_by_offset(&mut self, src: u32, offset: i32, ts: Timestamp) -> bool {
        if offset != 0 {
            return false;
        }

        let src_idx = src as usize;

        if src_idx >= self.vertex_capacity() {
            return false;
        }

        let probe = match self.slot_at(src_idx) {
            Some(probe) => probe,
            None => return false,
        };

        // Only revert deletions that happened at or before rollback time.
        if can_revert_delete(&probe, ts) {
            self.cold_slots[src_idx].delete_ts = Timestamp::MAX;
            self.edge_count += 1;
            return true;
        }

        false
    }

    pub fn delete_edge_by_offset(
        &mut self,
        src: u32,
        offset: i32,
        ts: Timestamp,
    ) -> StorageResult<bool> {
        if offset != 0 {
            return Ok(false);
        }
        let src_idx = src as usize;
        if src_idx >= self.vertex_capacity() {
            return Ok(false);
        }
        let edge_id = self.hot_slots[src_idx].edge_id;
        self.delete_edge(src, edge_id, ts)
    }

    pub fn nbr_at_offset(&self, src: u32, offset: i32) -> Option<Nbr> {
        if offset != 0 {
            return None;
        }
        self.slot_at(src as usize)
    }

    pub fn get_edge_physical(&self, src: u32, dst: VertexId) -> Option<Nbr> {
        let probe = self.slot_at(src as usize)?;
        if probe.edge_id == INVALID_EDGE_ID || probe.delete_ts != Timestamp::MAX {
            return None;
        }
        let (dst_ep, dst_rank) = decode_endpoint_pair(dst);
        if probe.endpoint == dst_ep && probe.rank == dst_rank {
            Some(probe)
        } else {
            None
        }
    }

    /// Test and offline use; production scans use `fill_physical_into` or
    /// the visitor paths instead of this allocating accessor.
    pub fn physical_edges_of(&self, src: u32) -> Vec<Nbr> {
        let mut out = Vec::new();
        self.fill_physical_into(src, &mut out);
        out
    }

    /// Fill a caller buffer with the physically stored entry of one slot.
    ///
    /// Same content as the allocating accessor above, without the per-vertex
    /// allocation. Lets batch scans share one buffer across vertices.
    pub fn fill_physical_into(&self, src: u32, out: &mut Vec<Nbr>) {
        out.clear();
        if let Some(probe) = self.slot_at(src as usize) {
            if probe.edge_id != INVALID_EDGE_ID {
                out.push(probe);
            }
        }
    }

    pub fn visit_physical<F>(&self, src: u32, mut f: F)
    where
        F: FnMut(Nbr) -> bool,
    {
        if let Some(probe) = self.slot_at(src as usize) {
            if probe.edge_id != INVALID_EDGE_ID {
                let _ = f(probe);
            }
        }
    }

    pub fn has_physical_entries(&self, vid: u32) -> bool {
        self.hot_slots
            .get(vid as usize)
            .is_some_and(|hot| hot.edge_id != INVALID_EDGE_ID)
    }

    pub fn primary_contains(&self, src: u32, edge_id: EdgeId) -> bool {
        self.hot_slots
            .get(src as usize)
            .is_some_and(|hot| hot.edge_id == edge_id)
    }

    pub fn rollback_insert(&mut self, src: u32, edge_id: EdgeId) -> bool {
        let src_idx = src as usize;
        if src_idx >= self.vertex_capacity() {
            return false;
        }
        let slot = self.hot_slots[src_idx];
        if slot.edge_id == INVALID_EDGE_ID {
            return false;
        }
        if slot.edge_id != edge_id {
            return false;
        }
        let was_live = self.cold_slots[src_idx].is_live();
        self.set_slot(src_idx, empty_slot());
        if was_live {
            self.edge_count -= 1;
        }
        true
    }

    pub fn revert_delete_by_edge_id(&mut self, src: u32, edge_id: EdgeId, ts: Timestamp) -> bool {
        let src_idx = src as usize;
        if src_idx >= self.vertex_capacity() {
            return false;
        }
        let probe = match self.slot_at(src_idx) {
            Some(probe) => probe,
            None => return false,
        };
        if probe.edge_id == INVALID_EDGE_ID {
            return false;
        }
        if probe.edge_id != edge_id {
            return false;
        }
        if can_revert_delete(&probe, ts) {
            self.cold_slots[src_idx].delete_ts = Timestamp::MAX;
            self.edge_count += 1;
            return true;
        }
        false
    }

    pub fn reclaimable_count(&self, vid: u32, cutoff: Timestamp) -> usize {
        if cutoff == Timestamp::MAX {
            return 0;
        }
        let (Some(hot), Some(cold)) = (
            self.hot_slots.get(vid as usize),
            self.cold_slots.get(vid as usize),
        ) else {
            return 0;
        };
        if hot.edge_id != INVALID_EDGE_ID && is_reclaimable_cold(cold, cutoff) {
            1
        } else {
            0
        }
    }

    pub fn vertex_census(&self, vid: u32) -> (usize, usize, usize) {
        let (Some(hot), Some(cold)) = (
            self.hot_slots.get(vid as usize),
            self.cold_slots.get(vid as usize),
        ) else {
            return (0, 0, 0);
        };
        if hot.edge_id == INVALID_EDGE_ID {
            return (0, 0, 0);
        }
        if cold.is_live() {
            (1, 0, 1)
        } else {
            (0, 1, 1)
        }
    }

    /// Single-slot reclaim probe: `(dead, reclaimable)` in one slot read.
    pub fn vertex_reclaim_probe(&self, vid: u32, cutoff: Timestamp) -> (usize, usize) {
        let (Some(hot), Some(cold)) = (
            self.hot_slots.get(vid as usize),
            self.cold_slots.get(vid as usize),
        ) else {
            return (0, 0);
        };
        if hot.edge_id == INVALID_EDGE_ID || cold.is_live() {
            return (0, 0);
        }
        let reclaimable =
            usize::from(cutoff != Timestamp::MAX && is_reclaimable_cold(cold, cutoff));
        (1, reclaimable)
    }

    pub fn compact_vertex_with_reporting(
        &mut self,
        vid: u32,
        cutoff: Timestamp,
        on_edge_removed: &mut dyn FnMut(EdgeId, Timestamp),
    ) -> usize {
        if self.reclaimable_count(vid, cutoff) == 0 {
            return 0;
        }
        let src_idx = vid as usize;
        let (edge_id, delete_ts) = (
            self.hot_slots[src_idx].edge_id,
            self.cold_slots[src_idx].delete_ts,
        );
        on_edge_removed(edge_id, delete_ts);
        self.set_slot(src_idx, empty_slot());
        1
    }

    pub fn compact_with_ts_reporting(
        &mut self,
        cutoff: Timestamp,
        on_edge_removed: &mut dyn FnMut(EdgeId, Timestamp),
    ) -> usize {
        if cutoff == Timestamp::MAX {
            return 0;
        }
        let mut removed = 0usize;
        for idx in 0..self.hot_slots.len() {
            let hot = self.hot_slots[idx];
            let cold = self.cold_slots[idx];
            if hot.edge_id != INVALID_EDGE_ID && is_reclaimable_cold(&cold, cutoff) {
                on_edge_removed(hot.edge_id, cold.delete_ts);
                self.set_slot(idx, empty_slot());
                removed += 1;
            }
        }
        removed
    }

    /// Test and offline use; production reads use `iter_edges_of` directly
    /// instead of collecting through this allocating accessor.
    pub fn edges_of(&self, src: u32, ts: Timestamp) -> Vec<Nbr> {
        self.iter_edges_of(src, ts).into_iter().collect()
    }

    /// Read the single slot of one vertex when it is alive at `ts`.
    ///
    /// Allocation-free counterpart of `edges_of`: no vector is built for the
    /// one-entry row. Returns `None` for out-of-range rows and for slots
    /// that are empty or not alive at `ts`.
    pub fn iter_edges_of(&self, src: u32, ts: Timestamp) -> Option<Nbr> {
        let probe = self.slot_at(src as usize)?;
        probe.is_alive_at(ts).then_some(probe)
    }

    fn get_edge_any_dst(&self, src: u32, ts: Timestamp) -> Option<Nbr> {
        let probe = self.slot_at(src as usize)?;

        if probe.is_alive_at(ts) {
            Some(probe)
        } else {
            None
        }
    }

    pub fn clear(&mut self) {
        self.hot_slots.fill(HotNbr::dead_gap());
        self.cold_slots.fill(ColdStamps::dead_gap());
        self.edge_count = 0;
    }

    /// Dump with integer column encoding for neighbor and edge-id columns.
    ///
    /// Offsets are trivial for the single-edge layout (slot index equals row),
    /// so only neighbor, rank, edge-id and stamp columns go through the
    /// column path with a narrow plain fallback.
    pub fn dump(&self) -> Vec<u8> {
        let mut result = Vec::new();
        self.dump_into(&mut result);
        result
    }

    /// Borrow-based dump into `out`, byte-identical to `dump`. The payload
    /// carries a trailing CRC32 trailer verified on load.
    pub fn dump_into(&self, out: &mut Vec<u8>) {
        let start = out.len();
        out.extend_from_slice(&self.edge_count.to_le_bytes());
        out.extend_from_slice(&(self.hot_slots.len() as u64).to_le_bytes());

        {
            let endpoints: Vec<u32> = self.hot_slots.iter().map(|hot| hot.endpoint).collect();
            let (_, endpoints_payload) = encode_topology_u32_column(&endpoints);
            out.extend_from_slice(&endpoints_payload);
        }
        {
            let ranks: Vec<i64> = self.hot_slots.iter().map(|hot| hot.rank).collect();
            let (_, ranks_payload) = encode_topology_i64_column(&ranks);
            out.extend_from_slice(&ranks_payload);
        }
        {
            let edge_ids: Vec<u64> = self.hot_slots.iter().map(|hot| hot.edge_id.0).collect();
            let (_, edge_ids_payload) = encode_topology_u64_column(&edge_ids);
            out.extend_from_slice(&edge_ids_payload);
        }
        {
            let delete_stamps: Vec<u64> =
                self.cold_slots.iter().map(|cold| cold.delete_ts).collect();
            let (_, delete_payload) = encode_topology_u64_column(&delete_stamps);
            out.extend_from_slice(&delete_payload);
        }
        let crc = crc32fast::hash(&out[start..]);
        out.extend_from_slice(&crc.to_le_bytes());
    }

    /// Reserved slot memory plus struct overhead.
    ///
    /// Per-shape accounting by design: the single-slot layout owns no offset
    /// arrays, live sets or overflow maps, so its shard-level total is
    /// narrower than the multi-edge layout. Cross-shape comparison belongs at
    /// the table layer after the shared authority estimate is added, never at
    /// the shard level directly.
    pub fn used_memory_size(&self) -> usize {
        self.hot_slots.capacity() * std::mem::size_of::<HotNbr>()
            + self.cold_slots.capacity() * std::mem::size_of::<ColdStamps>()
            + std::mem::size_of::<Self>()
    }

    /// Load the single persisted layout. The trailing
    /// CRC32 is verified before any parsing.
    pub fn load(&mut self, data: &[u8]) -> StorageResult<()> {
        if data.len() < 16 {
            return Err(StorageError::deserialize_error(
                "Single CSR data too short for header",
            ));
        }
        let (body, trailer) = data.split_at(data.len() - 4);
        let mut stored_bytes = [0u8; 4];
        stored_bytes.copy_from_slice(trailer);
        let stored = u32::from_le_bytes(stored_bytes);
        let computed = crc32fast::hash(body);
        if stored != computed {
            return Err(StorageError::deserialize_error(format!(
                "Single CSR dump CRC mismatch: stored={:#x} computed={:#x}",
                stored, computed
            )));
        }
        let data = body;

        let mut offset = 0usize;

        let edge_count = read_u64_le(data, &mut offset)?;
        let slot_count = read_u64_le(data, &mut offset)? as usize;

        let endpoints = decode_topology_u32_column(data, &mut offset)?;
        let ranks = decode_topology_i64_column(data, &mut offset)?;
        let edge_ids = decode_topology_u64_column(data, &mut offset)?;
        let delete_stamps = decode_topology_u64_column(data, &mut offset)?;
        if endpoints.len() != slot_count
            || ranks.len() != slot_count
            || edge_ids.len() != slot_count
            || delete_stamps.len() != slot_count
        {
            return Err(StorageError::deserialize_error(
                "Single CSR column length mismatch",
            ));
        }
        let mut hot_slots = Vec::with_capacity(slot_count);
        let mut cold_slots = Vec::with_capacity(slot_count);
        for index in 0..slot_count {
            hot_slots.push(HotNbr {
                endpoint: endpoints[index],
                rank: ranks[index],
                edge_id: EdgeId(edge_ids[index]),
            });
            cold_slots.push(ColdStamps {
                delete_ts: delete_stamps[index],
            });
        }
        let recomputed = hot_slots
            .iter()
            .zip(cold_slots.iter())
            .filter(|(hot, cold)| hot.edge_id != INVALID_EDGE_ID && cold.is_live())
            .count() as u64;
        if recomputed != edge_count {
            return Err(StorageError::deserialize_error(format!(
                "Single CSR edge count mismatch: stored={}, recomputed={}",
                edge_count, recomputed
            )));
        }
        if offset != data.len() {
            return Err(StorageError::deserialize_error(
                "Single CSR has trailing bytes: unsupported format",
            ));
        }

        self.hot_slots = hot_slots;
        self.cold_slots = cold_slots;
        self.edge_count = edge_count;

        Ok(())
    }

    pub fn iter(&self, ts: Timestamp) -> SingleMutableCsrIterator<'_> {
        SingleMutableCsrIterator::new(self, ts)
    }

    /// Iterate over all physically present entries, including tombstoned ones.
    pub fn iter_all(&self) -> SingleMutableCsrIterator<'_> {
        SingleMutableCsrIterator::new_all(self)
    }
}

impl Default for SingleMutableCsr {
    fn default() -> Self {
        Self::new()
    }
}

pub struct SingleMutableCsrIterator<'a> {
    csr: &'a SingleMutableCsr,
    current_vertex: usize,
    ts: Timestamp,
    include_deleted: bool,
}

impl<'a> SingleMutableCsrIterator<'a> {
    pub fn new(csr: &'a SingleMutableCsr, ts: Timestamp) -> Self {
        Self {
            csr,
            current_vertex: 0,
            ts,
            include_deleted: false,
        }
    }

    /// Iterator over every stored entry, including tombstoned ones.
    pub fn new_all(csr: &'a SingleMutableCsr) -> Self {
        Self {
            csr,
            current_vertex: 0,
            ts: 0,
            include_deleted: true,
        }
    }
}

impl<'a> Iterator for SingleMutableCsrIterator<'a> {
    type Item = (VertexId, Nbr);

    fn next(&mut self) -> Option<Self::Item> {
        while self.current_vertex < self.csr.vertex_capacity() {
            let vid = self.current_vertex;
            self.current_vertex += 1;

            let nbr = if self.include_deleted {
                self.csr
                    .slot_at(vid)
                    .filter(|n| n.edge_id != INVALID_EDGE_ID)
            } else {
                self.csr.get_edge_any_dst(vid as u32, self.ts)
            };
            if let Some(nbr) = nbr {
                return Some((VertexId::from_int64(vid as i64), nbr));
            }
        }
        None
    }
}

impl CsrBase for SingleMutableCsr {
    fn vertex_capacity(&self) -> usize {
        SingleMutableCsr::vertex_capacity(self)
    }

    fn edge_count(&self) -> u64 {
        self.edge_count
    }

    fn dump(&self) -> Vec<u8> {
        SingleMutableCsr::dump(self)
    }

    fn dump_into(&self, out: &mut Vec<u8>) {
        SingleMutableCsr::dump_into(self, out)
    }

    fn load(&mut self, data: &[u8]) -> StorageResult<()> {
        SingleMutableCsr::load(self, data)
    }
}

impl MutableCsrTrait for SingleMutableCsr {
    fn insert_edge(
        &mut self,
        src: u32,
        dst: VertexId,
        edge_id: EdgeId,
        ts: Timestamp,
    ) -> StorageResult<()> {
        SingleMutableCsr::insert_edge(self, src, dst, edge_id, ts)
    }

    fn delete_edge(&mut self, src: u32, edge_id: EdgeId, ts: Timestamp) -> StorageResult<bool> {
        SingleMutableCsr::delete_edge(self, src, edge_id, ts)
    }

    fn delete_edge_by_dst(&mut self, src: u32, dst: VertexId, ts: Timestamp) -> usize {
        SingleMutableCsr::delete_edge_by_dst(self, src, dst, ts)
    }

    fn delete_edge_by_dst_reporting(
        &mut self,
        src: u32,
        dst: VertexId,
        ts: Timestamp,
        on_deleted: &mut dyn FnMut(EdgeId),
    ) -> usize {
        SingleMutableCsr::delete_edge_by_dst_reporting(self, src, dst, ts, on_deleted)
    }

    fn delete_edge_by_offset(
        &mut self,
        src: u32,
        offset: i32,
        ts: Timestamp,
    ) -> StorageResult<bool> {
        SingleMutableCsr::delete_edge_by_offset(self, src, offset, ts)
    }

    fn revert_delete_by_offset(&mut self, src: u32, offset: i32, ts: Timestamp) -> bool {
        SingleMutableCsr::revert_delete_by_offset(self, src, offset, ts)
    }

    fn nbr_at_offset(&self, src: u32, offset: i32) -> Option<Nbr> {
        SingleMutableCsr::nbr_at_offset(self, src, offset)
    }

    fn get_edge_physical(&self, src: u32, dst: VertexId) -> Option<Nbr> {
        SingleMutableCsr::get_edge_physical(self, src, dst)
    }

    fn physical_edges_of(&self, src: u32) -> Vec<Nbr> {
        SingleMutableCsr::physical_edges_of(self, src)
    }

    fn fill_physical_into(&self, src: u32, out: &mut Vec<Nbr>) {
        SingleMutableCsr::fill_physical_into(self, src, out)
    }

    fn has_physical_entries(&self, vid: u32) -> bool {
        SingleMutableCsr::has_physical_entries(self, vid)
    }

    fn primary_contains(&self, src_vid: u32, edge_id: EdgeId) -> bool {
        SingleMutableCsr::primary_contains(self, src_vid, edge_id)
    }

    fn rollback_insert(&mut self, src_vid: u32, edge_id: EdgeId) -> bool {
        SingleMutableCsr::rollback_insert(self, src_vid, edge_id)
    }

    fn revert_delete_by_edge_id(&mut self, src_vid: u32, edge_id: EdgeId, ts: Timestamp) -> bool {
        SingleMutableCsr::revert_delete_by_edge_id(self, src_vid, edge_id, ts)
    }

    fn get_edge(&self, src: u32, dst: VertexId, ts: Timestamp) -> Option<Nbr> {
        SingleMutableCsr::get_edge(self, src, dst, ts)
    }

    fn edges_of(&self, src: u32, ts: Timestamp) -> Vec<Nbr> {
        SingleMutableCsr::edges_of(self, src, ts)
    }

    fn reclaimable_count(&self, vid: u32, cutoff: Timestamp) -> usize {
        SingleMutableCsr::reclaimable_count(self, vid, cutoff)
    }

    fn vertex_census(&self, vid: u32) -> (usize, usize, usize) {
        SingleMutableCsr::vertex_census(self, vid)
    }

    fn vertex_reclaim_probe(&self, vid: u32, cutoff: Timestamp) -> (usize, usize) {
        SingleMutableCsr::vertex_reclaim_probe(self, vid, cutoff)
    }

    fn compact_vertex_with_reporting(
        &mut self,
        vid: u32,
        cutoff: Timestamp,
        on_edge_removed: &mut dyn FnMut(EdgeId, Timestamp),
    ) -> usize {
        SingleMutableCsr::compact_vertex_with_reporting(self, vid, cutoff, on_edge_removed)
    }

    fn used_memory_size(&self) -> usize {
        SingleMutableCsr::used_memory_size(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_operations() {
        let mut csr = SingleMutableCsr::with_capacity(10);

        csr.insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 100)
            .unwrap();
        assert!(csr
            .insert_edge(0u32, VertexId::from_int64(2), EdgeId(101), 99)
            .is_err());
        assert!(csr
            .insert_edge(0u32, VertexId::from_int64(2), EdgeId(102), 101)
            .is_err());

        assert_eq!(csr.edge_count(), 1);
    }

    #[test]
    fn test_second_live_edge_rejected_at_csr_layer() {
        let mut csr = SingleMutableCsr::with_capacity(4);
        csr.insert_edge(0u32, VertexId::from_int64(10), EdgeId(100), 100)
            .unwrap();
        let err = csr
            .insert_edge(0u32, VertexId::from_int64(11), EdgeId(101), 200)
            .expect_err("second live edge must be rejected");
        assert!(err.to_string().contains("conflict"));
        assert_eq!(csr.edge_count(), 1);
        assert!(csr.delete_edge(0, EdgeId(100), 150).unwrap());
        csr.insert_edge(0u32, VertexId::from_int64(11), EdgeId(101), 151)
            .unwrap();
        assert_eq!(csr.edge_count(), 1);
    }

    #[test]
    fn test_physical_lookup_skips_tombstone() {
        let mut csr = SingleMutableCsr::with_capacity(4);
        csr.insert_edge(0u32, VertexId::from_int64(10), EdgeId(100), 100)
            .unwrap();
        assert!(csr.delete_edge(0, EdgeId(100), 150).unwrap());
        assert!(csr.get_edge_physical(0, VertexId::from_int64(10)).is_none());
        assert_eq!(csr.physical_edges_of(0).len(), 1);
    }

    #[test]
    fn test_exact_edge_id_required_for_delete() {
        let mut csr = SingleMutableCsr::with_capacity(4);
        csr.insert_edge(0u32, VertexId::from_int64(10), EdgeId(100), 100)
            .unwrap();
        assert!(!csr.delete_edge(0, EdgeId(999), 150).unwrap());
        assert!(!csr
            .delete_edge(0, crate::edge::INVALID_EDGE_ID, 150)
            .unwrap());
        assert!(csr.delete_edge(0, EdgeId(100), 150).unwrap());
    }

    #[test]
    fn test_delete_missing_id_on_tombstone_returns_not_found() {
        let mut csr = SingleMutableCsr::with_capacity(4);
        csr.insert_edge(0u32, VertexId::from_int64(10), EdgeId(100), 100)
            .unwrap();
        assert!(csr.delete_edge(0, EdgeId(100), 150).unwrap());
        assert!(!csr.delete_edge(0, EdgeId(999), 160).unwrap());
        assert!(csr.delete_edge(0, EdgeId(100), 160).is_err());
        assert!(!csr.delete_edge(0, EdgeId(100), 150).unwrap());
    }

    #[test]
    fn test_delete_by_dst_reports_count() {
        let mut csr = SingleMutableCsr::with_capacity(4);
        csr.insert_edge(0u32, VertexId::from_int64(10), EdgeId(100), 100)
            .unwrap();
        assert_eq!(csr.delete_edge_by_dst(0, VertexId::from_int64(11), 150), 0);
        assert_eq!(csr.delete_edge_by_dst(0, VertexId::from_int64(10), 150), 1);
        assert_eq!(csr.delete_edge_by_dst(0, VertexId::from_int64(10), 150), 0);
    }

    #[test]
    fn test_dump_and_load() {
        let mut csr1 = SingleMutableCsr::with_capacity(10);

        // Use insert_edge to populate data
        csr1.insert_edge(0u32, VertexId::from_int64(10), EdgeId(100), 100)
            .unwrap();
        csr1.insert_edge(1u32, VertexId::from_int64(20), EdgeId(101), 100)
            .unwrap();
        csr1.insert_edge(2u32, VertexId::from_int64(30), EdgeId(102), 100)
            .unwrap();

        let data = csr1.dump();

        let mut csr2 = SingleMutableCsr::new();
        csr2.load(&data).unwrap();

        assert_eq!(csr2.vertex_capacity(), csr1.vertex_capacity());
        assert_eq!(csr2.edge_count(), csr1.edge_count());
    }

    #[test]
    fn test_load_rejects_tampered_edge_count() {
        let mut csr1 = SingleMutableCsr::with_capacity(10);
        csr1.insert_edge(0u32, VertexId::from_int64(10), EdgeId(100), 100)
            .unwrap();
        csr1.insert_edge(1u32, VertexId::from_int64(20), EdgeId(101), 100)
            .unwrap();
        let data = csr1.dump();
        let mut ok = SingleMutableCsr::new();
        ok.load(&data).expect("normal payload must load");

        let mut tampered = data.clone();
        let stored = u64::from_le_bytes(tampered[4..12].try_into().unwrap());
        tampered[4..12].copy_from_slice(&(stored + 1).to_le_bytes());
        let mut csr2 = SingleMutableCsr::new();
        let err = csr2.load(&tampered).expect_err("tampered count must fail");
        assert!(err.to_string().contains("CRC mismatch"));

        // Re-seal the trailer so the CRC passes: the structural edge-count
        // validation underneath must still catch the tamper.
        let body_len = tampered.len() - 4;
        let resealed = crc32fast::hash(&tampered[..body_len]);
        tampered[body_len..].copy_from_slice(&resealed.to_le_bytes());
        let mut csr3 = SingleMutableCsr::new();
        let err = csr3.load(&tampered).expect_err("resealed count must fail");
        assert!(err.to_string().contains("edge count mismatch"));
    }

    #[test]
    fn test_dump_and_load_roundtrip() {
        let mut csr1 = SingleMutableCsr::with_capacity(10);
        csr1.insert_edge(0u32, VertexId::from_int64(10), EdgeId(100), 100)
            .unwrap();

        let data = csr1.dump();
        let mut csr2 = SingleMutableCsr::new();
        csr2.load(&data).unwrap();

        assert!(csr2.get_edge(0, VertexId::from_int64(10), 99).is_some());
        assert!(csr2.get_edge(0, VertexId::from_int64(10), 100).is_some());
        assert_eq!(csr2.edges_of(0, 99).len(), 1);
        assert_eq!(csr2.edges_of(0, 100).len(), 1);
    }

    #[test]
    fn test_load_rejects_truncated_and_trailing_data() {
        let mut csr1 = SingleMutableCsr::with_capacity(4);
        csr1.insert_edge(0u32, VertexId::from_int64(10), EdgeId(100), 100)
            .unwrap();
        let data = csr1.dump();

        // Truncated payload.
        let mut csr2 = SingleMutableCsr::new();
        assert!(csr2.load(&data[..data.len() - 8]).is_err());

        // Trailing bytes.
        let mut trailing = data.clone();
        trailing.push(0xff);
        assert!(csr2.load(&trailing).is_err());
    }

    #[test]
    fn test_offset_delete_propagates_conflict() {
        let mut csr = SingleMutableCsr::with_capacity(4);
        csr.insert_edge(0u32, VertexId::from_int64(10), EdgeId(100), 100)
            .unwrap();
        assert!(csr.delete_edge(0, EdgeId(100), 150).unwrap());
        assert!(!csr.delete_edge(0, EdgeId(100), 150).unwrap());
        assert!(csr.delete_edge(0, EdgeId(100), 160).is_err());
        // Offset path surfaces the same conflict instead of folding it.
        assert!(csr.delete_edge_by_offset(0, 0, 160).is_err());
        assert!(!csr.delete_edge_by_offset(0, 1, 160).unwrap());
    }

    #[test]
    fn test_resurrect_allows_any_timestamp() {
        let mut csr = SingleMutableCsr::with_capacity(4);
        csr.insert_edge(0u32, VertexId::from_int64(10), EdgeId(100), 100)
            .unwrap();
        assert!(csr.delete_edge(0, EdgeId(100), 150).unwrap());
        csr.insert_edge(0u32, VertexId::from_int64(11), EdgeId(101), 140)
            .unwrap();
        assert_eq!(csr.edge_count(), 1);
    }

    #[test]
    fn test_resurrect_with_equal_timestamp() {
        let mut csr = SingleMutableCsr::with_capacity(4);
        csr.insert_edge(0u32, VertexId::from_int64(10), EdgeId(100), 100)
            .unwrap();
        assert!(csr.delete_edge(0, EdgeId(100), 150).unwrap());
        csr.insert_edge(0u32, VertexId::from_int64(11), EdgeId(101), 150)
            .unwrap();
        assert_eq!(csr.edge_count(), 1);
    }

    #[test]
    fn test_single_reclaim_reports_and_clears_slot() {
        let mut csr = SingleMutableCsr::with_capacity(4);
        csr.insert_edge(0u32, VertexId::from_int64(10), EdgeId(100), 100)
            .unwrap();
        assert!(csr.delete_edge(0, EdgeId(100), 150).unwrap());
        assert_eq!(csr.reclaimable_count(0, 100), 0);
        assert_eq!(csr.reclaimable_count(0, 150), 1);
        assert_eq!(csr.vertex_census(0), (0, 1, 1));
        let mut reported = Vec::new();
        assert_eq!(
            csr.compact_vertex_with_reporting(0, 150, &mut |id, ts| reported.push((id, ts))),
            1
        );
        assert_eq!(reported, vec![(EdgeId(100), 150)]);
        assert_eq!(csr.vertex_census(0), (0, 0, 0));
        assert!(!csr.has_physical_entries(0));
        csr.insert_edge(0u32, VertexId::from_int64(11), EdgeId(101), 160)
            .unwrap();
        assert_eq!(csr.edge_count(), 1);
    }

    #[test]
    fn test_single_remove_and_revert_by_id() {
        let mut csr = SingleMutableCsr::with_capacity(4);
        csr.insert_edge(0u32, VertexId::from_int64(10), EdgeId(100), 100)
            .unwrap();
        assert!(csr.rollback_insert(0, EdgeId(100)));
        assert_eq!(csr.edge_count(), 0);
        assert!(!csr.has_physical_entries(0));
        csr.insert_edge(0u32, VertexId::from_int64(10), EdgeId(101), 110)
            .unwrap();
        assert!(csr.delete_edge(0, EdgeId(101), 120).unwrap());
        assert!(csr.revert_delete_by_edge_id(0, EdgeId(101), 130));
        assert_eq!(csr.edges_of(0, 130).len(), 1);
    }

    #[test]
    fn test_single_topology_encoding_roundtrip() {
        let mut csr = SingleMutableCsr::with_capacity(8);
        csr.insert_edge(0u32, VertexId::from_int64(10), EdgeId(100), 100)
            .unwrap();
        csr.insert_edge(3u32, VertexId::from_int64(11), EdgeId(101), 100)
            .unwrap();
        let payload = csr.dump();
        let mut loaded = SingleMutableCsr::new();
        loaded.load(&payload).expect("encoded load must succeed");
        assert_eq!(loaded.edge_count(), 2);
        assert_eq!(loaded.edges_of(0, 200).len(), 1);
        assert_eq!(loaded.edges_of(3, 200).len(), 1);
        assert_eq!(loaded.edges_of(1, 200).len(), 0);
    }

    #[test]
    fn test_single_topology_encoding_rejects_garbage() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&5u64.to_le_bytes());
        payload.extend_from_slice(&[0u8; 24]);
        let mut csr = SingleMutableCsr::new();
        assert!(csr.load(&payload).is_err());
    }
}
