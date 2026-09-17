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
//! - Resurrection of a tombstoned slot still requires a timestamp past both
//!   creation and deletion stamps; live-slot rejection needs no monotonicity
//!   assumption.
//!
//! If concurrent writes are needed, use MutableCsr (accepts multiple edges).

use std::sync::atomic::{AtomicU64, Ordering};

use crate::persistence::{read_u32_le, read_u64_le};
use graphdb_core::{StorageError, StorageResult};

use super::mutable_csr::serialization::{
    decode_topology_i64_column, decode_topology_u32_column, decode_topology_u64_column,
    encode_topology_i64_column, encode_topology_u32_column, encode_topology_u64_column,
};
use super::{CsrBase, EdgeId, MutableCsrTrait, Nbr, Timestamp, VertexId, INVALID_EDGE_ID};

/// Persistence version for the single-edge topology columns. Version 2
/// carries the integer column path for neighbor and edge-id columns plus a
/// version header; versionless payloads are rejected, never converted.
pub(crate) const SINGLE_CSR_FORMAT_VERSION: u32 = 2;

const DEFAULT_VERTEX_CAPACITY: usize = 1024;
const VERTEX_GROWTH_FACTOR: f64 = 1.25;

pub struct SingleMutableCsr {
    nbr_list: Vec<Nbr>,
    edge_count: AtomicU64,
}

impl Clone for SingleMutableCsr {
    fn clone(&self) -> Self {
        Self {
            nbr_list: self.nbr_list.clone(),
            edge_count: AtomicU64::new(self.edge_count.load(Ordering::Relaxed)),
        }
    }
}

impl std::fmt::Debug for SingleMutableCsr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SingleMutableCsr")
            .field("vertex_capacity", &self.vertex_capacity())
            .field("edge_count", &self.edge_count.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl SingleMutableCsr {
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_VERTEX_CAPACITY)
    }

    pub fn with_capacity(vertex_capacity: usize) -> Self {
        let vertex_cap = vertex_capacity.max(1);
        let nbr_list = vec![Nbr::with_timestamps(0, 0, INVALID_EDGE_ID, 0); vertex_cap];

        Self {
            nbr_list,
            edge_count: AtomicU64::new(0),
        }
    }

    pub fn vertex_capacity(&self) -> usize {
        self.nbr_list.len()
    }

    pub fn edge_count(&self) -> u64 {
        self.edge_count.load(Ordering::Relaxed)
    }

    pub fn resize(&mut self, new_vertex_capacity: usize) {
        if new_vertex_capacity <= self.vertex_capacity() {
            return;
        }

        let additional = new_vertex_capacity - self.vertex_capacity();
        self.nbr_list.extend(std::iter::repeat_n(
            Nbr::with_timestamps(0, 0, INVALID_EDGE_ID, 0),
            additional,
        ));
    }

    pub fn ensure_vertex_capacity(&mut self, min_capacity: usize) {
        if min_capacity > self.vertex_capacity() {
            let new_capacity =
                ((min_capacity as f64 * VERTEX_GROWTH_FACTOR).ceil() as usize).max(min_capacity);
            self.resize(new_capacity);
        }
    }

    pub fn insert_edge(
        &mut self,
        src: u32,
        dst: VertexId,
        edge_id: EdgeId,
        ts: Timestamp,
    ) -> StorageResult<()> {
        let src_idx = src as usize;

        if src_idx >= self.vertex_capacity() {
            self.ensure_vertex_capacity(src_idx + 1);
        }

        let nbr = &mut self.nbr_list[src_idx];

        // Reject any second live edge in the same slot, regardless of timestamp.
        // Matches the table-layer Single contract; no silent overwrite and no
        // monotonicity assumption for the live case.
        if nbr.delete_ts == Timestamp::MAX && nbr.edge_id != INVALID_EDGE_ID {
            return Err(StorageError::conflict(format!(
                "[SingleMutableCsr] insert conflict on src={}: slot holds live edge {:?}",
                src, nbr.edge_id
            )));
        }
        // Resurrection follows the same monotonicity as live writes: the new
        // timestamp must advance past both the creation and deletion stamps.
        if nbr.delete_ts != Timestamp::MAX
            && nbr.edge_id != INVALID_EDGE_ID
            && (ts <= nbr.create_ts || ts <= nbr.delete_ts)
        {
            return Err(StorageError::conflict(format!(
                "[SingleMutableCsr] resurrect conflict on src={}: ts={} <= create_ts={} or delete_ts={}",
                src, ts, nbr.create_ts, nbr.delete_ts
            )));
        }

        let was_empty = nbr.edge_id == INVALID_EDGE_ID || nbr.delete_ts != Timestamp::MAX;
        let (endpoint_vid, rank) = dst.decode_edge_endpoint();
        nbr.endpoint = endpoint_vid.as_int64().unwrap_or(0) as u32;
        nbr.rank = rank;
        nbr.edge_id = edge_id;
        nbr.create_ts = ts;
        nbr.delete_ts = Timestamp::MAX;

        if was_empty {
            self.edge_count.fetch_add(1, Ordering::Relaxed);
        }

        Ok(())
    }

    pub fn delete_edge(&mut self, src: u32, edge_id: EdgeId, ts: Timestamp) -> StorageResult<bool> {
        let src_idx = src as usize;

        if src_idx >= self.vertex_capacity() {
            return Ok(false);
        }

        let nbr = &mut self.nbr_list[src_idx];

        if nbr.edge_id == INVALID_EDGE_ID {
            return Ok(false);
        }

        if nbr.delete_ts < Timestamp::MAX {
            if nbr.delete_ts != ts {
                return Err(StorageError::write_write_conflict(format!(
                    "edge {:?} already deleted at ts={}, attempted delete at ts={}",
                    nbr.edge_id, nbr.delete_ts, ts
                )));
            }
            // Idempotent re-delete at the same timestamp.
            return Ok(false);
        }

        let create_ts = nbr.create_ts;
        if create_ts > ts {
            return Ok(false);
        }

        if nbr.edge_id != edge_id {
            return Ok(false);
        }

        nbr.delete_ts = ts;
        self.edge_count.fetch_sub(1, Ordering::Relaxed);
        Ok(true)
    }

    /// Delete the single matching live entry for full-match endpoint semantics.
    ///
    /// Returns the deleted count (0 or 1) so table rollback can reconcile by
    /// count. One call deletes the whole match; no first-only variant exists.
    pub fn delete_edge_by_dst(&mut self, src: u32, dst: VertexId, ts: Timestamp) -> usize {
        let src_idx = src as usize;

        if src_idx >= self.vertex_capacity() {
            return 0;
        }

        let (dst_ep_vid, dst_rank) = dst.decode_edge_endpoint();
        let dst_ep = dst_ep_vid.as_int64().unwrap_or(0) as u32;
        let nbr = &mut self.nbr_list[src_idx];

        if nbr.edge_id == INVALID_EDGE_ID
            || nbr.endpoint != dst_ep
            || nbr.rank != dst_rank
            || nbr.delete_ts < Timestamp::MAX
        {
            return 0;
        }

        let create_ts = nbr.create_ts;
        if create_ts > ts {
            return 0;
        }

        nbr.delete_ts = ts;
        self.edge_count.fetch_sub(1, Ordering::Relaxed);
        1
    }

    pub fn get_edge(&self, src: u32, dst: VertexId, ts: Timestamp) -> Option<Nbr> {
        let src_idx = src as usize;

        if src_idx >= self.vertex_capacity() {
            return None;
        }

        let (dst_ep_vid, dst_rank) = dst.decode_edge_endpoint();
        let dst_ep = dst_ep_vid.as_int64().unwrap_or(0) as u32;
        let nbr = &self.nbr_list[src_idx];

        if !nbr.is_alive_at(ts) {
            return None;
        }

        if nbr.endpoint == dst_ep && nbr.rank == dst_rank {
            Some(*nbr)
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

        let nbr = &mut self.nbr_list[src_idx];

        // Only revert deletions that happened at or before rollback time.
        if nbr.delete_ts < Timestamp::MAX && nbr.delete_ts <= ts {
            nbr.delete_ts = Timestamp::MAX;
            self.edge_count.fetch_add(1, Ordering::Relaxed);
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
        let edge_id = self.nbr_list[src_idx].edge_id;
        self.delete_edge(src, edge_id, ts)
    }

    pub fn nbr_at_offset(&self, src: u32, offset: i32) -> Option<Nbr> {
        if offset != 0 {
            return None;
        }
        self.nbr_list.get(src as usize).copied()
    }

    pub fn get_edge_physical(&self, src: u32, dst: VertexId) -> Option<Nbr> {
        let slot = self.nbr_list.get(src as usize)?;
        if slot.edge_id == INVALID_EDGE_ID {
            return None;
        }
        let (dst_ep_vid, dst_rank) = dst.decode_edge_endpoint();
        let dst_ep = dst_ep_vid.as_int64().unwrap_or(0) as u32;
        if slot.endpoint == dst_ep && slot.rank == dst_rank {
            Some(*slot)
        } else {
            None
        }
    }

    pub fn physical_edges_of(&self, src: u32) -> Vec<Nbr> {
        match self.nbr_list.get(src as usize) {
            Some(nbr) if nbr.edge_id != INVALID_EDGE_ID => vec![*nbr],
            _ => Vec::new(),
        }
    }

    pub fn visit_physical<F>(&self, src: u32, mut f: F)
    where
        F: FnMut(Nbr) -> bool,
    {
        if let Some(nbr) = self.nbr_list.get(src as usize) {
            if nbr.edge_id != INVALID_EDGE_ID {
                let _ = f(*nbr);
            }
        }
    }

    pub fn has_physical_entries(&self, vid: u32) -> bool {
        self.nbr_list
            .get(vid as usize)
            .is_some_and(|nbr| nbr.edge_id != INVALID_EDGE_ID)
    }

    pub fn primary_contains(&self, src: u32, edge_id: EdgeId) -> bool {
        self.nbr_list
            .get(src as usize)
            .is_some_and(|slot| slot.edge_id == edge_id)
    }

    pub fn remove_edge(&mut self, src: u32, edge_id: EdgeId) -> bool {
        let src_idx = src as usize;
        if src_idx >= self.vertex_capacity() {
            return false;
        }
        let slot = &mut self.nbr_list[src_idx];
        if slot.edge_id == INVALID_EDGE_ID {
            return false;
        }
        if slot.edge_id != edge_id {
            return false;
        }
        let was_live = slot.delete_ts == Timestamp::MAX;
        *slot = Nbr::with_timestamps(0, 0, INVALID_EDGE_ID, 0);
        if was_live {
            self.edge_count.fetch_sub(1, Ordering::Relaxed);
        }
        true
    }

    pub fn revert_delete_by_edge_id(&mut self, src: u32, edge_id: EdgeId, ts: Timestamp) -> bool {
        let src_idx = src as usize;
        if src_idx >= self.vertex_capacity() {
            return false;
        }
        let slot = &mut self.nbr_list[src_idx];
        if slot.edge_id == INVALID_EDGE_ID {
            return false;
        }
        if slot.edge_id != edge_id {
            return false;
        }
        if slot.delete_ts != Timestamp::MAX && slot.delete_ts <= ts {
            slot.delete_ts = Timestamp::MAX;
            self.edge_count.fetch_add(1, Ordering::Relaxed);
            return true;
        }
        false
    }

    pub fn reclaimable_count(&self, vid: u32, cutoff: Timestamp) -> usize {
        if cutoff == Timestamp::MAX {
            return 0;
        }
        let Some(slot) = self.nbr_list.get(vid as usize) else {
            return 0;
        };
        if slot.edge_id != INVALID_EDGE_ID
            && slot.delete_ts != Timestamp::MAX
            && crate::mvcc_visibility::Visibility::is_gc_eligible(slot.delete_ts, cutoff)
        {
            1
        } else {
            0
        }
    }

    pub fn vertex_census(&self, vid: u32) -> (usize, usize, usize) {
        let Some(slot) = self.nbr_list.get(vid as usize) else {
            return (0, 0, 0);
        };
        if slot.edge_id == INVALID_EDGE_ID {
            return (0, 0, 0);
        }
        if slot.delete_ts == Timestamp::MAX {
            (1, 0, 1)
        } else {
            (0, 1, 1)
        }
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
            self.nbr_list[src_idx].edge_id,
            self.nbr_list[src_idx].delete_ts,
        );
        on_edge_removed(edge_id, delete_ts);
        self.nbr_list[src_idx] = Nbr::with_timestamps(0, 0, INVALID_EDGE_ID, 0);
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
        for idx in 0..self.nbr_list.len() {
            let slot = self.nbr_list[idx];
            if slot.edge_id != INVALID_EDGE_ID
                && slot.delete_ts != Timestamp::MAX
                && crate::mvcc_visibility::Visibility::is_gc_eligible(slot.delete_ts, cutoff)
            {
                on_edge_removed(slot.edge_id, slot.delete_ts);
                self.nbr_list[idx] = Nbr::with_timestamps(0, 0, INVALID_EDGE_ID, 0);
                removed += 1;
            }
        }
        removed
    }

    pub fn edges_of(&self, src: u32, ts: Timestamp) -> Vec<Nbr> {
        let src_idx = src as usize;

        if src_idx >= self.vertex_capacity() {
            return Vec::new();
        }

        let nbr = &self.nbr_list[src_idx];

        if !nbr.is_alive_at(ts) {
            return Vec::new();
        }

        vec![*nbr]
    }

    fn get_edge_any_dst(&self, src: u32, ts: Timestamp) -> Option<Nbr> {
        let src_idx = src as usize;

        if src_idx >= self.vertex_capacity() {
            return None;
        }

        let nbr = &self.nbr_list[src_idx];

        if nbr.is_alive_at(ts) {
            Some(*nbr)
        } else {
            None
        }
    }

    pub fn clear(&mut self) {
        for nbr in &mut self.nbr_list {
            *nbr = Nbr::with_timestamps(0, 0, INVALID_EDGE_ID, 0);
        }
        self.edge_count.store(0, Ordering::Relaxed);
    }

    /// Dump with integer column encoding for neighbor and edge-id columns.
    ///
    /// Offsets are trivial for the single-edge layout (slot index equals row),
    /// so only neighbor, rank, edge-id and stamp columns go through the
    /// column path with a plain fallback. Versionless payloads are rejected
    /// on load.
    pub fn dump(&self) -> Vec<u8> {
        let mut result = Vec::new();

        result.extend_from_slice(&SINGLE_CSR_FORMAT_VERSION.to_le_bytes());
        result.extend_from_slice(&self.edge_count.load(Ordering::Relaxed).to_le_bytes());
        result.extend_from_slice(&(self.nbr_list.len() as u64).to_le_bytes());

        let endpoints: Vec<u32> = self.nbr_list.iter().map(|nbr| nbr.endpoint).collect();
        let ranks: Vec<i64> = self.nbr_list.iter().map(|nbr| nbr.rank).collect();
        let edge_ids: Vec<u64> = self.nbr_list.iter().map(|nbr| nbr.edge_id.0).collect();
        let create_stamps: Vec<u64> = self.nbr_list.iter().map(|nbr| nbr.create_ts).collect();
        let delete_stamps: Vec<u64> = self.nbr_list.iter().map(|nbr| nbr.delete_ts).collect();
        let (_, endpoints_payload) = encode_topology_u32_column(&endpoints);
        result.extend_from_slice(&endpoints_payload);
        let (_, ranks_payload) = encode_topology_i64_column(&ranks);
        result.extend_from_slice(&ranks_payload);
        let (_, edge_ids_payload) = encode_topology_u64_column(&edge_ids);
        result.extend_from_slice(&edge_ids_payload);
        let (_, create_payload) = encode_topology_u64_column(&create_stamps);
        result.extend_from_slice(&create_payload);
        let (_, delete_payload) = encode_topology_u64_column(&delete_stamps);
        result.extend_from_slice(&delete_payload);

        result
    }

    pub fn used_memory_size(&self) -> usize {
        self.nbr_list.len() * std::mem::size_of::<Nbr>() + std::mem::size_of::<Self>()
    }

    /// Load version 2 only; versionless payloads fail closed.
    pub fn load(&mut self, data: &[u8]) -> StorageResult<()> {
        if data.len() < 16 {
            return Err(StorageError::deserialize_error(
                "Single CSR data too short for header",
            ));
        }

        let mut offset = 0usize;

        let version = read_u32_le(data, &mut offset)?;
        if version != SINGLE_CSR_FORMAT_VERSION {
            return Err(StorageError::deserialize_error(format!(
                "Unsupported single CSR format version: {}",
                version
            )));
        }
        let edge_count = read_u64_le(data, &mut offset)?;
        let slot_count = read_u64_le(data, &mut offset)? as usize;

        let endpoints = decode_topology_u32_column(data, &mut offset)?;
        let ranks = decode_topology_i64_column(data, &mut offset)?;
        let edge_ids = decode_topology_u64_column(data, &mut offset)?;
        let create_stamps = decode_topology_u64_column(data, &mut offset)?;
        let delete_stamps = decode_topology_u64_column(data, &mut offset)?;
        if endpoints.len() != slot_count
            || ranks.len() != slot_count
            || edge_ids.len() != slot_count
            || create_stamps.len() != slot_count
            || delete_stamps.len() != slot_count
        {
            return Err(StorageError::deserialize_error(
                "Single CSR column length mismatch",
            ));
        }
        let mut nbr_list = Vec::with_capacity(slot_count);
        for index in 0..slot_count {
            let mut nbr = Nbr::with_timestamps(
                endpoints[index],
                ranks[index],
                EdgeId(edge_ids[index]),
                delete_stamps[index],
            );
            nbr.create_ts = create_stamps[index];
            nbr_list.push(nbr);
        }
        if offset != data.len() {
            return Err(StorageError::deserialize_error(
                "Single CSR has trailing bytes: unsupported format",
            ));
        }

        self.nbr_list = nbr_list;
        self.edge_count.store(edge_count, Ordering::Relaxed);

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
                    .nbr_list
                    .get(vid)
                    .copied()
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
        self.edge_count.load(Ordering::Relaxed)
    }

    fn dump(&self) -> Vec<u8> {
        SingleMutableCsr::dump(self)
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

    fn has_physical_entries(&self, vid: u32) -> bool {
        SingleMutableCsr::has_physical_entries(self, vid)
    }

    fn primary_contains(&self, src_vid: u32, edge_id: EdgeId) -> bool {
        SingleMutableCsr::primary_contains(self, src_vid, edge_id)
    }

    fn remove_edge(&mut self, src_vid: u32, edge_id: EdgeId) -> bool {
        SingleMutableCsr::remove_edge(self, src_vid, edge_id)
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
    fn test_dump_and_load_preserves_create_ts() {
        let mut csr1 = SingleMutableCsr::with_capacity(10);
        csr1.insert_edge(0u32, VertexId::from_int64(10), EdgeId(100), 100)
            .unwrap();

        let data = csr1.dump();
        let mut csr2 = SingleMutableCsr::new();
        csr2.load(&data).unwrap();

        // Time travel survives the roundtrip: invisible before creation.
        assert!(csr2.get_edge(0, VertexId::from_int64(10), 99).is_none());
        assert!(csr2.get_edge(0, VertexId::from_int64(10), 100).is_some());
        assert_eq!(csr2.edges_of(0, 99).len(), 0);
        assert_eq!(csr2.edges_of(0, 100).len(), 1);
    }

    #[test]
    fn test_load_rejects_truncated_and_trailing_data() {
        let mut csr1 = SingleMutableCsr::with_capacity(4);
        csr1.insert_edge(0u32, VertexId::from_int64(10), EdgeId(100), 100)
            .unwrap();
        let data = csr1.dump();

        // Truncated payload (old format without create_ts is one such case).
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
    fn test_resurrect_requires_monotonic_timestamp() {
        let mut csr = SingleMutableCsr::with_capacity(4);
        csr.insert_edge(0u32, VertexId::from_int64(10), EdgeId(100), 100)
            .unwrap();
        assert!(csr.delete_edge(0, EdgeId(100), 150).unwrap());
        assert!(csr
            .insert_edge(0u32, VertexId::from_int64(11), EdgeId(101), 140)
            .is_err());
        assert!(csr
            .insert_edge(0u32, VertexId::from_int64(11), EdgeId(101), 150)
            .is_err());
        csr.insert_edge(0u32, VertexId::from_int64(11), EdgeId(101), 151)
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
        assert!(csr.remove_edge(0, EdgeId(100)));
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
    fn test_single_topology_encoding_rejects_versionless() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&5u64.to_le_bytes());
        payload.extend_from_slice(&[0u8; 24]);
        let mut csr = SingleMutableCsr::new();
        assert!(csr.load(&payload).is_err());
    }
}
