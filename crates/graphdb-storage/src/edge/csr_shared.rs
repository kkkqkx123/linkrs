//! Shared helpers for the CSR edge stores.
//!
//! `MutableCsr` (multi-edge rows) and `SingleMutableCsr` (one slot per
//! vertex) use different layouts but share the same vertex-capacity growth
//! policy and the same per-slot MVCC state machine (delete conflict, revert
//! window, tombstone GC eligibility). This module is the single source of
//! truth so the two stores cannot drift apart.

use super::{ColdStamps, EdgeId, Nbr, Timestamp, VertexId};
use graphdb_core::{StorageError, StorageResult};

pub(crate) const DEFAULT_VERTEX_CAPACITY: usize = 1024;
pub(crate) const VERTEX_GROWTH_FACTOR: f64 = 1.25;

/// Vertex-capacity growth shared by the CSR stores.
pub(crate) fn grown_vertex_capacity(min_capacity: usize) -> usize {
    ((min_capacity as f64 * VERTEX_GROWTH_FACTOR).ceil() as usize).max(min_capacity)
}

/// Decode a full-match endpoint key into the packed `(endpoint, rank)` pair
/// stored in `Nbr` rows. `decode_edge_endpoint` always yields an
/// int64-encoded vertex id, so `as_int64` is the canonical projection; the
/// low 32 bits match the former `as_u64` call sites bit for bit.
pub(crate) fn decode_endpoint_pair(dst: VertexId) -> (u32, i64) {
    let (vid, rank) = dst.decode_edge_endpoint();
    (vid.as_int64().unwrap_or(0) as u32, rank)
}

/// Outcome of running the shared per-slot delete state machine on an
/// edge-id-matched slot.
pub(crate) enum DeleteSlotOutcome {
    /// Caller must stamp `delete_ts` and account the removal.
    Stamped,
    /// Idempotent re-delete at the same timestamp; report `false`.
    AlreadyStamped,
}

/// Shared delete state machine for one matched slot: a tombstone stamped at
/// another timestamp is a write-write conflict, a tombstone at the same
/// timestamp is idempotent, and an already-live edge is eligible for stamping.
pub(crate) fn decide_slot_delete(
    nbr: &Nbr,
    edge_id: EdgeId,
    ts: Timestamp,
) -> StorageResult<DeleteSlotOutcome> {
    if nbr.delete_ts != Timestamp::MAX {
        if nbr.delete_ts != ts {
            return Err(StorageError::write_write_conflict(format!(
                "edge {:?} already deleted at ts={}, attempted delete at ts={}",
                edge_id, nbr.delete_ts, ts
            )));
        }
        return Ok(DeleteSlotOutcome::AlreadyStamped);
    }
    Ok(DeleteSlotOutcome::Stamped)
}

/// Shared revert window: only deletions at or before the rollback point may
/// be undone.
pub(crate) fn can_revert_delete(nbr: &Nbr, ts: Timestamp) -> bool {
    nbr.delete_ts != Timestamp::MAX && nbr.delete_ts <= ts
}

/// Shared tombstone-eligibility predicate for one slot. Callers keep their
/// own `cutoff == Timestamp::MAX` early-out: without it every tombstone
/// would be eligible since `is_gc_eligible(ts, MAX)` always holds.
///
/// Cold-only form so maintenance sweeps never pull the topology lines into
/// cache for dead-slot checks.
pub(crate) fn is_reclaimable_cold(cold: &ColdStamps, cutoff: Timestamp) -> bool {
    cold.delete_ts != Timestamp::MAX
        && crate::mvcc_visibility::Visibility::is_gc_eligible(cold.delete_ts, cutoff)
}

/// Per-vertex row bookkeeping shared by `MutableCsr` and `PureTopologyCsr`.
///
/// Both CSR variants maintain identical arrays for vertex addressing:
/// `adj_offsets` (start index into the primary column arrays),
/// `degrees` (number of live physical slots), and
/// `primary_capacities` (reserved slots including gaps).
/// This struct captures that shared state so common operations can be
/// expressed once rather than duplicated across the two types.
#[derive(Debug, Clone)]
pub(crate) struct VertexBookkeeping {
    pub(crate) adj_offsets: Vec<u32>,
    pub(crate) degrees: Vec<u32>,
    pub(crate) primary_capacities: Vec<u32>,
}

impl VertexBookkeeping {
    pub(crate) fn with_capacity(vertex_cap: usize) -> Self {
        Self {
            adj_offsets: vec![0; vertex_cap],
            degrees: vec![0; vertex_cap],
            primary_capacities: vec![0; vertex_cap],
        }
    }

    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.adj_offsets.len()
    }

    /// Clamped primary window `(start, end)` for one row, bounded by the
    /// column lengths so callers never overrun.
    #[inline]
    pub(crate) fn primary_window(&self, src_idx: usize, col_len: usize) -> (usize, usize) {
        if src_idx >= self.len() {
            return (0, 0);
        }
        let start = self.adj_offsets[src_idx] as usize;
        let end = start
            .saturating_add(self.degrees[src_idx] as usize)
            .min(col_len);
        (start.min(end), end)
    }

    /// Resize all three arrays to `new_vertex_capacity`, extending with
    /// defaults (zero offset, zero degree, zero capacity).
    pub(crate) fn resize(&mut self, new_vertex_capacity: usize, tail: u32) {
        self.adj_offsets.resize(new_vertex_capacity, tail);
        self.degrees.resize(new_vertex_capacity, 0);
        self.primary_capacities.resize(new_vertex_capacity, 0);
    }

    /// Assign primary block location for `src_idx`.
    pub(crate) fn assign_primary_block(&mut self, src_idx: usize, block_offset: u32, degree: u32) {
        self.adj_offsets[src_idx] = block_offset;
        self.primary_capacities[src_idx] = degree;
    }
}

/// Segment address bits: one segment covers `1 << SEGMENT_SHIFT` vertices.
pub(crate) const SEGMENT_SHIFT: u32 = 10;
/// Vertices per segment. Power of two so routing is shift+mask, no div/mod.
pub(crate) const SEGMENT_SIZE: usize = 1 << SEGMENT_SHIFT;
/// In-segment offset mask.
pub(crate) const SEGMENT_MASK: usize = SEGMENT_SIZE - 1;

/// Segmented sparse row table shared by the overflow and live-key indexes.
///
/// Row addresses inside one CSR are dense vertex ids, so non-empty rows keep
/// direct-subscript addressing: segment by shift, offset by mask, two
/// subscripts per lookup. Segments with no touched row stay `None` and cost
/// only the pointer, so millions of empty vertices no longer pay a slot each.
/// Segments are never freed once allocated; `clear` only empties their slots.
///
/// The live-entry/row counters stay in the owning wrapper (`OverflowTable`,
/// `LiveSetStorage`); this table only owns addressing and storage.
#[derive(Debug, Clone, Default)]
pub struct SegmentedTable<T> {
    segments: Vec<Option<Box<[Option<T>; SEGMENT_SIZE]>>>,
}

impl<T> SegmentedTable<T> {
    pub(crate) fn new() -> Self {
        Self {
            segments: Vec::new(),
        }
    }

    /// Row address split into `(segment, in-segment offset)`.
    #[inline]
    fn locate(vid: u32) -> (usize, usize) {
        let idx = vid as usize;
        (idx >> SEGMENT_SHIFT, idx & SEGMENT_MASK)
    }

    /// Reserve segment pointers so `vertex_capacity` is addressable. Segment
    /// contents are never allocated here: untouched segments stay `None`.
    pub(crate) fn ensure_capacity(&mut self, vertex_capacity: usize) {
        let need = vertex_capacity.div_ceil(SEGMENT_SIZE);
        if self.segments.len() < need {
            self.segments.resize_with(need, || None);
        }
    }

    /// Shared row reference without allocating anything.
    #[inline]
    pub(crate) fn get(&self, vid: u32) -> Option<&T> {
        let (seg, off) = Self::locate(vid);
        self.segments.get(seg)?.as_ref()?.get(off)?.as_ref()
    }

    /// Exclusive row reference without allocating anything.
    #[inline]
    pub(crate) fn get_mut(&mut self, vid: u32) -> Option<&mut T> {
        let (seg, off) = Self::locate(vid);
        self.segments.get_mut(seg)?.as_mut()?.get_mut(off)?.as_mut()
    }

    /// Mutable row slot, allocating the segment on first touch. Live counters
    /// stay with the caller: this only guarantees the slot exists.
    pub(crate) fn slot_mut(&mut self, vid: u32) -> &mut Option<T> {
        let (seg, off) = Self::locate(vid);
        if self.segments.len() <= seg {
            self.segments.resize_with(seg + 1, || None);
        }
        let segment =
            self.segments[seg].get_or_insert_with(|| Box::new(std::array::from_fn(|_| None)));
        &mut segment[off]
    }

    /// Take the row value out, leaving `None` behind. Never allocates.
    pub(crate) fn take(&mut self, vid: u32) -> Option<T> {
        let (seg, off) = Self::locate(vid);
        self.segments.get_mut(seg)?.as_mut()?.get_mut(off)?.take()
    }

    /// Empty every allocated segment in place. Segment allocations and the
    /// pointer table are kept so a reused store does not realloc.
    pub(crate) fn clear(&mut self) {
        for segment in self.segments.iter_mut().flatten() {
            for slot in segment.iter_mut() {
                *slot = None;
            }
        }
    }

    /// Rows in ascending vertex order across segments.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (u32, &T)> {
        self.segments.iter().enumerate().flat_map(|(seg, segment)| {
            let base = (seg << SEGMENT_SHIFT) as u32;
            segment
                .as_ref()
                .map(|slab| slab.iter())
                .into_iter()
                .flatten()
                .enumerate()
                .filter_map(move |(off, slot)| {
                    slot.as_ref().map(|value| (base + off as u32, value))
                })
        })
    }

    /// Rows in ascending vertex order across segments, exclusive.
    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = (u32, &mut T)> {
        self.segments
            .iter_mut()
            .enumerate()
            .flat_map(|(seg, segment)| {
                let base = (seg << SEGMENT_SHIFT) as u32;
                segment
                    .as_mut()
                    .map(|slab| slab.iter_mut())
                    .into_iter()
                    .flatten()
                    .enumerate()
                    .filter_map(move |(off, slot)| {
                        slot.as_mut().map(|value| (base + off as u32, value))
                    })
            })
    }

    /// Number of allocated segments (segments with at least one touched row).
    pub(crate) fn allocated_segments(&self) -> usize {
        self.segments.iter().filter(|seg| seg.is_some()).count()
    }

    /// Reserved index memory: pointer table plus allocated segment slabs.
    pub(crate) fn table_bytes(&self) -> usize {
        self.segments.capacity() * std::mem::size_of::<Option<Box<[Option<T>; SEGMENT_SIZE]>>>()
            + self.allocated_segments() * SEGMENT_SIZE * std::mem::size_of::<Option<T>>()
    }
}

/// Chunk contract for the shared per-vertex overflow table.
///
/// The mutable CSR (`OverflowChunk`: hot-topology plus cold-stamp halves)
/// and the pure CSR (`PureOverflowChunk`: endpoint plus edge-id halves) share
/// the table routing, presence bitmap and live-row ledger below; only the
/// slot encoding differs. Method names mirror the inherent chunk methods so
/// the two chunk types stay in sync with this contract.
pub trait OverflowChunkSpec {
    /// One assembled record as split across the owning chunk halves.
    type Slot;
    fn with_capacity(cap: usize) -> Self;
    fn len(&self) -> usize;
    fn capacity(&self) -> usize;
    fn push_slot(&mut self, slot: Self::Slot);
}

/// Per-vertex overflow storage with segmented sparse row indexing, shared by
/// the mutable and pure CSR variants.
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
pub struct OverflowTable<C: OverflowChunkSpec> {
    table: SegmentedTable<Vec<C>>,
    live_entries: usize,
    present: Vec<u64>,
}

impl<C: OverflowChunkSpec> OverflowTable<C> {
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
    pub fn get(&self, vid: u32) -> Option<&Vec<C>> {
        if !self.has_row(vid) {
            return None;
        }
        self.table.get(vid)
    }

    /// Single-block fast path for consolidated rows.
    ///
    /// Merge passes (write-path repack, rebalance, vertex compaction) leave
    /// merged rows as one contiguous chunk, so most overflow reads touch one
    /// block. Returns it directly when the row holds exactly one chunk;
    /// multi-block rows fall back to the chain walk.
    #[inline]
    pub fn single_chunk(&self, vid: u32) -> Option<&C> {
        let chunks = self.get(vid)?;
        if chunks.len() == 1 {
            chunks.first()
        } else {
            None
        }
    }

    /// Chunk count of one row, zero when the row holds no overflow.
    #[inline]
    pub fn chunk_count(&self, vid: u32) -> usize {
        self.get(vid).map_or(0, Vec::len)
    }

    #[inline]
    pub fn get_mut(&mut self, vid: u32) -> Option<&mut Vec<C>> {
        if !self.has_row(vid) {
            return None;
        }
        self.table.get_mut(vid)
    }

    /// Get mutable reference to the chunk list for `vid`, inserting an empty
    /// entry if absent.
    #[inline]
    pub fn get_or_create(&mut self, vid: u32) -> &mut Vec<C> {
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
        slot: C::Slot,
        chunk_edges: usize,
    ) -> (usize, Option<usize>) {
        let row = self.table.slot_mut(vid);
        if row.is_none() {
            *row = Some(Vec::new());
            self.live_entries += 1;
        }
        let chunks = row.as_mut().expect("slot just created");
        let mut added = None;
        if chunks
            .last()
            .is_none_or(|chunk| chunk.len() >= chunk.capacity().max(1))
        {
            chunks.push(C::with_capacity(chunk_edges));
            added = chunks.last().map(|chunk| chunk.capacity());
        }
        chunks
            .last_mut()
            .expect("tail chunk just ensured")
            .push_slot(slot);
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
    pub fn insert(&mut self, vid: u32, chunks: Vec<C>) {
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
    pub fn contains_key(&self, vid: u32) -> bool {
        self.has_row(vid) && self.get(vid).is_some_and(|chunks| !chunks.is_empty())
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
    pub fn iter(&self) -> impl Iterator<Item = (u32, &Vec<C>)> {
        self.table.iter()
    }

    #[inline]
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (u32, &mut Vec<C>)> {
        self.table.iter_mut()
    }

    /// Remove entry for `vid` and return its chunks if present.
    #[inline]
    pub fn remove(&mut self, vid: u32) -> Option<Vec<C>> {
        if !self.has_row(vid) {
            return None;
        }
        let taken = self.table.take(vid);
        if taken.is_some() {
            self.live_entries = self.live_entries.saturating_sub(1);
            self.set_present(vid, false);
        }
        taken
    }

    /// Total number of slot entries across all overflow chunks.
    pub fn total_entry_count(&self) -> usize {
        self.table
            .iter()
            .flat_map(|(_, chunks)| chunks.iter())
            .map(|chunk| chunk.len())
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
            + self.live_entries * (std::mem::size_of::<u32>() + std::mem::size_of::<Vec<C>>())
    }
}
