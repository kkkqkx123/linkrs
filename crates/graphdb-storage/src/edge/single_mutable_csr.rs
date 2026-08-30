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
//! ⚠️  CONCURRENCY LIMITATION:
//! ================================
//! This CSR does NOT support concurrent updates at the same timestamp.
//!
//! - Each vertex can have at most 1 effective edge.
//! - Newer timestamps overwrite older ones automatically.
//! - If two updates arrive with the same (or non-monotonic) timestamp,
//!   the later one will be SILENTLY REJECTED.
//!
//! Example of problematic scenario:
//! ```ignore
//! T1: insert_edge(v0, dst=v1, ts=100) ✓ succeeds
//! T2: insert_edge(v0, dst=v1, ts=99)  ✗ rejected (99 < 100)
//! T3: insert_edge(v0, dst=v1, ts=100) ✗ rejected (100 == 100, not strictly greater)
//! ```
//!
//! WHEN TO USE:
//! - Strictly one-to-one relationships where updates are ordered by global timestamp.
//! - Systems where timestamp monotonicity is guaranteed by upstream layers (WAL, MVCC).
//!
//! WHEN NOT TO USE:
//! - Distributed systems with concurrent writes from multiple clients.
//! - Scenarios requiring multiple historical versions (use MutableCsr instead).
//! - Cases where updates may arrive out-of-order or with equal timestamps.
//!
//! RECOMMENDED WORKAROUNDS:
//! 1. If concurrent writes are needed, use MutableCsr (accepts multiple edges).
//! 2. If single-edge semantics with multi-value support is needed,
//!    consider a new MultiSingleMutableCsr variant (under design).
//! 3. Ensure timestamp ordering at the upper layer (WAL, transaction log).

use std::sync::atomic::{AtomicU64, Ordering};

use crate::persistence::read_u64_le;
use graphdb_core::{StorageError, StorageResult};

use super::{CsrBase, EdgeId, MutableCsrTrait, Nbr, Timestamp, VertexId, INVALID_EDGE_ID};

fn write_vertex_id(out: &mut Vec<u8>, id: VertexId) {
    let bytes = id.as_bytes();
    out.push(bytes.len() as u8);
    out.extend_from_slice(bytes);
}

fn read_vertex_id(data: &[u8], offset: &mut usize) -> StorageResult<VertexId> {
    if *offset >= data.len() {
        return Err(StorageError::deserialize_error(
            "Single CSR data too short for vertex id length",
        ));
    }

    let len = data[*offset] as usize;
    *offset += 1;
    if data.len().saturating_sub(*offset) < len {
        return Err(StorageError::deserialize_error(
            "Single CSR data too short for vertex id bytes",
        ));
    }

    let id = VertexId::from_bytes(data[*offset..*offset + len].to_vec());
    *offset += len;
    Ok(id)
}

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
        prop_offset: u32,
    ) -> StorageResult<()> {
        let src_idx = src as usize;

        if src_idx >= self.vertex_capacity() {
            self.ensure_vertex_capacity(src_idx + 1);
        }

        let nbr = &mut self.nbr_list[src_idx];

        // Reject if there's an active edge with newer or equal timestamp
        if nbr.delete_ts == Timestamp::MAX && ts <= nbr.create_ts {
            return Err(StorageError::conflict(format!(
                "[SingleMutableCsr] insert conflict on src={}: ts={} <= existing create_ts={}",
                src, ts, nbr.create_ts
            )));
        }

        let was_empty = nbr.delete_ts < Timestamp::MAX;
        let (endpoint_vid, rank) = dst.decode_edge_endpoint();
        nbr.endpoint = endpoint_vid.as_int64().unwrap_or(0) as u32;
        nbr.rank = rank;
        nbr.edge_id = edge_id;
        nbr.create_ts = ts;
        nbr.delete_ts = Timestamp::MAX;
        nbr.prop_offset = prop_offset;

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

        if edge_id.0 != u64::MAX && nbr.edge_id != edge_id {
            return Ok(false);
        }

        nbr.delete_ts = ts;
        self.edge_count.fetch_sub(1, Ordering::Relaxed);
        Ok(true)
    }

    pub fn delete_edge_by_dst(&mut self, src: u32, dst: VertexId, ts: Timestamp) -> bool {
        let src_idx = src as usize;

        if src_idx >= self.vertex_capacity() {
            return false;
        }

        let (dst_ep_vid, dst_rank) = dst.decode_edge_endpoint();
        let dst_ep = dst_ep_vid.as_int64().unwrap_or(0) as u32;
        let nbr = &mut self.nbr_list[src_idx];

        if nbr.endpoint != dst_ep || nbr.rank != dst_rank || nbr.delete_ts < Timestamp::MAX {
            return false;
        }

        let create_ts = nbr.create_ts;
        if create_ts > ts {
            return false;
        }

        nbr.delete_ts = ts;
        self.edge_count.fetch_sub(1, Ordering::Relaxed);
        true
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

    pub fn delete_edge_by_offset(&mut self, src: u32, offset: i32, ts: Timestamp) -> bool {
        if offset != 0 {
            return false;
        }
        let src_idx = src as usize;
        if src_idx >= self.vertex_capacity() {
            return false;
        }
        let edge_id = self.nbr_list[src_idx].edge_id;
        self.delete_edge(src, edge_id, ts).unwrap_or(false)
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

    pub fn compact_with_ts(&mut self, _ts: Timestamp, _reserve_ratio: f32) -> usize {
        // No-op for single CSR - no tombstones to compact
        // Returns 0 as no edges are removed
        0
    }

    pub fn dump(&self) -> Vec<u8> {
        let mut result = Vec::new();

        result.extend_from_slice(&self.edge_count.load(Ordering::Relaxed).to_le_bytes());

        for nbr in &self.nbr_list {
            write_vertex_id(&mut result, nbr.to_vertex_id());
            result.extend_from_slice(&nbr.edge_id.to_le_bytes());
            result.extend_from_slice(&nbr.delete_ts.to_le_bytes());
        }

        result
    }

    pub fn used_memory_size(&self) -> usize {
        self.nbr_list.len() * std::mem::size_of::<Nbr>() + std::mem::size_of::<Self>()
    }

    pub fn load(&mut self, data: &[u8]) -> StorageResult<()> {
        if data.len() < 8 {
            return Err(StorageError::deserialize_error(
                "Single CSR data too short for header",
            ));
        }

        let mut offset = 0usize;

        let edge_count = read_u64_le(data, &mut offset)?;

        let mut nbr_list = Vec::new();
        while offset < data.len() {
            let neighbor = read_vertex_id(data, &mut offset)?;
            let raw_edge_id = read_u64_le(data, &mut offset)?;
            let delete_ts = read_u64_le(data, &mut offset)?;

            let edge_id = EdgeId(raw_edge_id);
            let (endpoint_vid, rank) = neighbor.decode_edge_endpoint();
            nbr_list.push(Nbr::with_timestamps(
                endpoint_vid.as_int64().unwrap_or(0) as u32,
                rank,
                edge_id,
                delete_ts,
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
        prop_offset: u32,
    ) -> StorageResult<()> {
        SingleMutableCsr::insert_edge(self, src, dst, edge_id, ts, prop_offset)
    }

    fn delete_edge(&mut self, src: u32, edge_id: EdgeId, ts: Timestamp) -> StorageResult<bool> {
        SingleMutableCsr::delete_edge(self, src, edge_id, ts)
    }

    fn delete_edge_by_dst(&mut self, src: u32, dst: VertexId, ts: Timestamp) -> bool {
        SingleMutableCsr::delete_edge_by_dst(self, src, dst, ts)
    }

    fn delete_edge_by_offset(&mut self, src: u32, offset: i32, ts: Timestamp) -> bool {
        SingleMutableCsr::delete_edge_by_offset(self, src, offset, ts)
    }

    fn revert_delete_by_offset(&mut self, src: u32, offset: i32, ts: Timestamp) -> bool {
        SingleMutableCsr::revert_delete_by_offset(self, src, offset, ts)
    }

    fn get_edge(&self, src: u32, dst: VertexId, ts: Timestamp) -> Option<Nbr> {
        SingleMutableCsr::get_edge(self, src, dst, ts)
    }

    fn edges_of(&self, src: u32, ts: Timestamp) -> Vec<Nbr> {
        SingleMutableCsr::edges_of(self, src, ts)
    }

    fn compact_with_ts(&mut self, ts: Timestamp, reserve_ratio: f32) -> usize {
        SingleMutableCsr::compact_with_ts(self, ts, reserve_ratio)
    }

    fn used_memory_size(&self) -> usize {
        SingleMutableCsr::used_memory_size(self)
    }

    fn create_ts_of(&self, edge_id: EdgeId) -> Option<Timestamp> {
        self.nbr_list
            .iter()
            .find(|n| n.edge_id == edge_id)
            .map(|n| n.create_ts)
    }

    fn rebuild_create_ts(&mut self, iter: impl Iterator<Item = (EdgeId, Timestamp)>) {
        let map: std::collections::HashMap<EdgeId, Timestamp> = iter.collect();
        for nbr in self.nbr_list.iter_mut() {
            if let Some(&ts) = map.get(&nbr.edge_id) {
                nbr.create_ts = ts;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_operations() {
        let mut csr = SingleMutableCsr::with_capacity(10);

        csr.insert_edge(
            0u32,
            VertexId::from_int64(1),
            EdgeId(100),
            100,
            crate::edge::property_schema::PROP_OFFSET_NONE,
        )
        .unwrap();
        assert!(csr
            .insert_edge(
                0u32,
                VertexId::from_int64(2),
                EdgeId(101),
                99,
                crate::edge::property_schema::PROP_OFFSET_NONE
            )
            .is_err());
        csr.insert_edge(
            0u32,
            VertexId::from_int64(2),
            EdgeId(102),
            101,
            crate::edge::property_schema::PROP_OFFSET_NONE,
        )
        .unwrap();

        assert_eq!(csr.edge_count(), 1);
    }

    #[test]
    fn test_dump_and_load() {
        let mut csr1 = SingleMutableCsr::with_capacity(10);

        // Use insert_edge to populate data
        csr1.insert_edge(
            0u32,
            VertexId::from_int64(10),
            EdgeId(100),
            100,
            crate::edge::property_schema::PROP_OFFSET_NONE,
        )
        .unwrap();
        csr1.insert_edge(
            1u32,
            VertexId::from_int64(20),
            EdgeId(101),
            100,
            crate::edge::property_schema::PROP_OFFSET_NONE,
        )
        .unwrap();
        csr1.insert_edge(
            2u32,
            VertexId::from_int64(30),
            EdgeId(102),
            100,
            crate::edge::property_schema::PROP_OFFSET_NONE,
        )
        .unwrap();

        let data = csr1.dump();

        let mut csr2 = SingleMutableCsr::new();
        let _ = csr2.load(&data);

        assert_eq!(csr2.vertex_capacity(), csr1.vertex_capacity());
        assert_eq!(csr2.edge_count(), csr1.edge_count());
    }
}
