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
    /// Edge not yet created at `ts`; report `false`.
    NotYetCreated,
}

/// Shared delete state machine for one matched slot: a tombstone stamped at
/// another timestamp is a write-write conflict, a tombstone at the same
/// timestamp is idempotent, and an edge created after `ts` is not deletable.
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
    if nbr.create_ts > ts {
        return Ok(DeleteSlotOutcome::NotYetCreated);
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
/// The live-entry/row counters stay in the owning wrapper (`OverflowStorage`,
/// `LiveSetStorage`); this table only owns addressing and storage.
#[derive(Debug, Clone, Default)]
pub(crate) struct SegmentedTable<T> {
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
