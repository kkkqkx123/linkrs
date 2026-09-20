use super::super::{csr_shared::OverflowTable, ColdStamps, HotNbr, Nbr};

/// Single overflow chunk: hot topology halves plus cold stamp halves.
///
/// The two vectors grow in lockstep; index `i` of both addresses one slot.
/// Scans that only need topology (edge-id lookup) or only liveness walk one
/// vector, keeping the other half out of cache.
#[derive(Debug, Clone, Default)]
pub struct OverflowChunk {
    hot: Vec<HotNbr>,
    cold: Vec<ColdStamps>,
}

impl OverflowChunk {
    pub fn with_capacity(cap: usize) -> Self {
        Self {
            hot: Vec::with_capacity(cap),
            cold: Vec::with_capacity(cap),
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.hot.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.hot.is_empty()
    }

    /// Reserved slots of this chunk (hot and cold grow together).
    #[inline]
    pub fn capacity(&self) -> usize {
        self.hot.capacity()
    }

    /// Assembled slot copy at `index`.
    #[inline]
    pub fn slot_at(&self, index: usize) -> Option<Nbr> {
        Some(Nbr::from_parts(
            *self.hot.get(index)?,
            *self.cold.get(index)?,
        ))
    }

    /// Hot half at `index` without touching the stamp lines.
    #[inline]
    pub fn hot_at(&self, index: usize) -> Option<HotNbr> {
        self.hot.get(index).copied()
    }

    /// Cold half at `index` without touching the topology lines.
    #[inline]
    pub fn cold_at(&self, index: usize) -> Option<ColdStamps> {
        self.cold.get(index).copied()
    }

    /// Hot slice for topology-only scans (edge-id lookup, key matching).
    #[inline]
    pub fn hot_slice(&self) -> &[HotNbr] {
        &self.hot
    }

    /// Cold slice for liveness-only scans.
    #[inline]
    pub fn cold_slice(&self) -> &[ColdStamps] {
        &self.cold
    }

    /// Mutable cold half for in-place stamping.
    #[inline]
    pub fn cold_at_mut(&mut self, index: usize) -> Option<&mut ColdStamps> {
        self.cold.get_mut(index)
    }

    /// Append one assembled record, splitting it into the halves.
    #[inline]
    pub fn push(&mut self, nbr: Nbr) {
        self.hot.push(nbr.hot());
        self.cold.push(nbr.cold());
        debug_assert_eq!(self.hot.len(), self.cold.len());
    }

    /// Remove and return the assembled record at `index`.
    #[inline]
    pub fn remove(&mut self, index: usize) -> Nbr {
        let hot = self.hot.remove(index);
        let cold = self.cold.remove(index);
        Nbr::from_parts(hot, cold)
    }

    /// Extend the chunk from an assembled slice.
    pub fn extend_from_slice(&mut self, slots: &[Nbr]) {
        self.hot.reserve(slots.len());
        self.cold.reserve(slots.len());
        for nbr in slots {
            self.hot.push(nbr.hot());
            self.cold.push(nbr.cold());
        }
    }

    /// Single contiguous chunk holding `slots`.
    ///
    /// Consolidation target for the repack passes: a repacked row reads as
    /// its primary block plus at most one overflow block instead of a chain
    /// of graded blocks. Built from the default (empty) chunk so the halves
    /// reserve exactly the staged length with no graded over-reservation.
    pub fn consolidated(slots: &[Nbr]) -> Self {
        let mut chunk = OverflowChunk::default();
        chunk.extend_from_slice(slots);
        chunk
    }
}

/// Single benchmarked per-vertex overflow bound. Past this many chunks a row
/// is repacked on the write path into one contiguous chunk; a row holding
/// only live entries is merged the same way so skewed rows cannot grow
/// unbounded chains. Region and full compactions still handle watermark
/// reclaim separately.
///
/// Source: 8 chunks bound the pointer-chase depth of the widest rows while
/// keeping the amortized repack cost off the insert hot path (covered by
/// `test_supernode_overflow_consolidates_repack_into_single_block`).
/// Recommended range 4..=16; lower repacks too eagerly on skewed inserts,
/// higher lets chains degrade point lookups. Retuning requires rerunning
/// the supernode benchmark first.
pub(crate) const OVERFLOW_REPACK_CHUNKS_PER_VERTEX: usize = 8;

impl super::super::csr_shared::OverflowChunkSpec for OverflowChunk {
    type Slot = Nbr;

    fn with_capacity(cap: usize) -> Self {
        OverflowChunk::with_capacity(cap)
    }

    #[inline]
    fn len(&self) -> usize {
        self.len()
    }

    #[inline]
    fn capacity(&self) -> usize {
        self.capacity()
    }

    #[inline]
    fn push_slot(&mut self, slot: Nbr) {
        self.push(slot);
    }
}

/// Mutable-CSR overflow storage: the shared overflow table over hot/cold
/// stamp chunks. See [`OverflowTable`](super::super::csr_shared::OverflowTable).
pub type OverflowStorage = OverflowTable<OverflowChunk>;

#[cfg(test)]
mod tests {
    use super::super::super::csr_shared::SEGMENT_SIZE;
    use super::*;
    use graphdb_core::types::EdgeId;

    fn chunks_with_first_endpoint(endpoint: u32) -> Vec<OverflowChunk> {
        let mut chunk = OverflowChunk::default();
        chunk.push(Nbr::new(endpoint, 0, EdgeId::new(1)));
        vec![chunk]
    }

    #[test]
    fn empty_vertices_cost_only_pointers() {
        let mut storage = OverflowStorage::new();
        storage.ensure_capacity(1_000_000);
        assert!(storage.is_empty());
        // One million empty rows: only the segment pointer table, well under
        // the hundreds of megabytes a dense slot array would reserve.
        assert!(storage.index_bytes() < 1_000_000);
        assert!(storage.get(999_999).is_none());
    }

    #[test]
    fn routes_across_segment_boundary() {
        let mut storage = OverflowStorage::new();
        let lo = SEGMENT_SIZE as u32 - 1;
        let hi = SEGMENT_SIZE as u32;
        let far = 2 * SEGMENT_SIZE as u32 + 5;
        storage.insert(lo, chunks_with_first_endpoint(10));
        storage.insert(hi, chunks_with_first_endpoint(20));
        storage.insert(far, chunks_with_first_endpoint(30));
        assert_eq!(storage.len(), 3);
        for (vid, endpoint) in [(lo, 10), (hi, 20), (far, 30)] {
            let chunks = storage.get(vid).expect("row present");
            assert_eq!(chunks[0].hot_at(0).expect("slot").endpoint, endpoint);
        }
        let taken = storage.remove(hi).expect("row removed");
        assert_eq!(taken[0].hot_at(0).expect("slot").endpoint, 20);
        assert_eq!(storage.len(), 2);
        assert!(storage.get(hi).is_none());
    }

    #[test]
    fn iter_walks_rows_in_vertex_order() {
        let mut storage = OverflowStorage::new();
        let far = 3 * SEGMENT_SIZE as u32 + 7;
        let near = 5u32;
        storage.insert(far, chunks_with_first_endpoint(30));
        storage.insert(near, chunks_with_first_endpoint(10));
        storage.insert(SEGMENT_SIZE as u32, chunks_with_first_endpoint(20));
        let order: Vec<u32> = storage.iter().map(|(vid, _)| vid).collect();
        assert_eq!(order, vec![near, SEGMENT_SIZE as u32, far]);
    }

    #[test]
    fn clear_empties_allocated_segments() {
        let mut storage = OverflowStorage::new();
        storage.insert(SEGMENT_SIZE as u32 + 1, chunks_with_first_endpoint(1));
        storage.clear();
        assert!(storage.is_empty());
        assert_eq!(storage.len(), 0);
        assert!(storage.get(SEGMENT_SIZE as u32 + 1).is_none());
        storage.insert(3, chunks_with_first_endpoint(2));
        assert_eq!(storage.len(), 1);
    }
}
