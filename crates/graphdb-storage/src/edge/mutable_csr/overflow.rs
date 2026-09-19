use super::super::{csr_shared::SegmentedTable, ColdStamps, HotNbr, Nbr};

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
pub(crate) const OVERFLOW_REPACK_CHUNKS_PER_VERTEX: usize = 8;

/// Per-vertex overflow storage with segmented sparse row indexing.
///
/// Row addresses inside one CSR are dense vertex ids, so non-empty rows are
/// addressed by direct subscript (segment by shift, offset by mask) instead
/// of hashing. Segments with no touched row stay unallocated and cost only
/// the pointer, so millions of empty vertices no longer pay a slot each;
/// sparsity across groups is still handled by the group map above this layer.
///
/// A one-bit-per-vertex presence map fronts every lookup: rows without
/// overflow answer from a single bit test instead of paying the segment
/// routing on every read, write and full-scan step.
#[derive(Debug, Clone, Default)]
pub struct OverflowStorage {
    table: SegmentedTable<Vec<OverflowChunk>>,
    live_entries: usize,
    present: Vec<u64>,
}

impl OverflowStorage {
    pub fn new() -> Self {
        Self {
            table: SegmentedTable::new(),
            live_entries: 0,
            present: Vec::new(),
        }
    }

    /// Bit-test fast path shared by the lookup entries.
    #[inline]
    fn has_row(&self, vid: u32) -> bool {
        let word = vid as usize / 64;
        let bit = vid as usize % 64;
        self.present
            .get(word)
            .is_some_and(|w| w & (1u64 << bit) != 0)
    }

    #[inline]
    fn set_present(&mut self, vid: u32, value: bool) {
        let word = vid as usize / 64;
        let bit = vid as usize % 64;
        if self.present.len() <= word {
            self.present.resize(word + 1, 0);
        }
        if value {
            self.present[word] |= 1u64 << bit;
        } else {
            self.present[word] &= !(1u64 << bit);
        }
    }

    /// Reserve segment pointers so `vertex_capacity` is addressable. Segment
    /// contents are never allocated here: untouched rows stay pointer-only.
    pub fn ensure_capacity(&mut self, vertex_capacity: usize) {
        self.table.ensure_capacity(vertex_capacity);
        let words = vertex_capacity.div_ceil(64);
        if self.present.len() < words {
            self.present.resize(words, 0);
        }
    }

    #[inline]
    pub fn get(&self, vid: &u32) -> Option<&Vec<OverflowChunk>> {
        if !self.has_row(*vid) {
            return None;
        }
        self.table.get(*vid)
    }

    /// Single-block fast path for consolidated rows.
    ///
    /// Merge passes (write-path repack, rebalance, vertex compaction) leave
    /// merged rows as one contiguous chunk, so most overflow reads touch one
    /// block. Returns it directly when the row holds exactly one chunk;
    /// multi-block rows fall back to the chain walk.
    #[inline]
    pub fn single_chunk(&self, vid: &u32) -> Option<&OverflowChunk> {
        let chunks = self.get(vid)?;
        if chunks.len() == 1 {
            chunks.first()
        } else {
            None
        }
    }

    /// Chunk count of one row, zero when the row holds no overflow.
    #[inline]
    pub fn chunk_count(&self, vid: &u32) -> usize {
        self.get(vid).map_or(0, Vec::len)
    }

    #[inline]
    pub fn get_mut(&mut self, vid: &u32) -> Option<&mut Vec<OverflowChunk>> {
        if !self.has_row(*vid) {
            return None;
        }
        self.table.get_mut(*vid)
    }

    /// Get mutable reference to the chunk list for `vid`, inserting an empty
    /// entry if absent.
    #[inline]
    pub fn get_or_create(&mut self, vid: u32) -> &mut Vec<OverflowChunk> {
        let slot = self.table.slot_mut(vid);
        if slot.is_none() {
            *slot = Some(Vec::new());
            self.live_entries += 1;
        }
        // Inline bit set on the disjoint `present` field: the table borrow
        // in `slot` is still live at the return below.
        let word = vid as usize / 64;
        if self.present.len() <= word {
            self.present.resize(word + 1, 0);
        }
        self.present[word] |= 1u64 << (vid as usize % 64);
        slot.as_mut().expect("slot just created")
    }

    /// Append one record to the row, allocating a fresh chunk when the tail
    /// chunk is full. Single table routing per call; returns the live chunk
    /// count plus the reserved capacity when a new chunk was allocated so the
    /// caller can ledger it without a second lookup.
    #[inline]
    pub fn push_to_row(
        &mut self,
        vid: u32,
        nbr: Nbr,
        chunk_edges: usize,
    ) -> (usize, Option<usize>) {
        let slot = self.table.slot_mut(vid);
        if slot.is_none() {
            *slot = Some(Vec::new());
            self.live_entries += 1;
        }
        let chunks = slot.as_mut().expect("slot just created");
        let mut added = None;
        if chunks
            .last()
            .is_none_or(|chunk| chunk.len() >= chunk.capacity().max(1))
        {
            chunks.push(OverflowChunk::with_capacity(chunk_edges));
            added = chunks.last().map(|chunk| chunk.capacity());
        }
        chunks
            .last_mut()
            .expect("tail chunk just ensured")
            .push(nbr);
        let pushed_len = chunks.len();
        // Inline bit set on the disjoint `present` field; see `get_or_create`.
        let word = vid as usize / 64;
        if self.present.len() <= word {
            self.present.resize(word + 1, 0);
        }
        self.present[word] |= 1u64 << (vid as usize % 64);
        (pushed_len, added)
    }

    #[inline]
    pub fn insert(&mut self, vid: u32, chunks: Vec<OverflowChunk>) {
        let slot = self.table.slot_mut(vid);
        if slot.is_none() && !chunks.is_empty() {
            self.live_entries += 1;
        } else if slot.is_some() && chunks.is_empty() {
            self.live_entries = self.live_entries.saturating_sub(1);
        }
        if chunks.is_empty() {
            *slot = None;
            self.set_present(vid, false);
        } else {
            *slot = Some(chunks);
            self.set_present(vid, true);
        }
    }

    #[inline]
    pub fn contains_key(&self, vid: &u32) -> bool {
        self.has_row(*vid) && self.get(vid).is_some_and(|chunks| !chunks.is_empty())
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.live_entries
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.live_entries == 0
    }

    #[inline]
    pub fn clear(&mut self) {
        self.table.clear();
        self.live_entries = 0;
        self.present.clear();
    }

    #[inline]
    pub fn iter(&self) -> impl Iterator<Item = (u32, &Vec<OverflowChunk>)> {
        self.table.iter()
    }

    #[inline]
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (u32, &mut Vec<OverflowChunk>)> {
        self.table.iter_mut()
    }

    /// Remove entry for `vid` and return its chunks if present.
    #[inline]
    pub fn remove(&mut self, vid: &u32) -> Option<Vec<OverflowChunk>> {
        if !self.has_row(*vid) {
            return None;
        }
        let taken = self.table.take(*vid);
        if taken.is_some() {
            self.live_entries = self.live_entries.saturating_sub(1);
            self.set_present(*vid, false);
        }
        taken
    }

    /// Total number of Nbr entries across all overflow chunks.
    pub fn total_nbr_count(&self) -> usize {
        self.table
            .iter()
            .flat_map(|(_, chunks)| chunks.iter())
            .map(OverflowChunk::len)
            .sum()
    }

    /// Estimate of wasted capacity inside overflow chunks (capacity - len).
    pub fn wasted_capacity(&self) -> usize {
        self.table
            .iter()
            .flat_map(|(_, chunks)| chunks.iter())
            .map(|c| c.capacity().saturating_sub(c.len()))
            .sum()
    }

    /// Segment pointer table plus allocated segment slabs, plus per-row
    /// chunk lists. Untouched segments contribute only their pointer.
    pub fn index_bytes(&self) -> usize {
        self.table.table_bytes()
            + self.live_entries
                * (std::mem::size_of::<u32>() + std::mem::size_of::<Vec<OverflowChunk>>())
    }
}

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
        assert!(storage.get(&999_999).is_none());
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
            let chunks = storage.get(&vid).expect("row present");
            assert_eq!(chunks[0].hot_at(0).expect("slot").endpoint, endpoint);
        }
        let taken = storage.remove(&hi).expect("row removed");
        assert_eq!(taken[0].hot_at(0).expect("slot").endpoint, 20);
        assert_eq!(storage.len(), 2);
        assert!(storage.get(&hi).is_none());
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
        assert!(storage.get(&(SEGMENT_SIZE as u32 + 1)).is_none());
        storage.insert(3, chunks_with_first_endpoint(2));
        assert_eq!(storage.len(), 1);
    }
}
