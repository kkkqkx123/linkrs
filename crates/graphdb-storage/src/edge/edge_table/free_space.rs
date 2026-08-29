//! Reusable CSR buffers for immutable segment allocation.
//!
//! Segment merges temporarily create a new CSR while retiring several old
//! ones. This pool keeps the retired CSR allocations alive and reuses the
//! closest fitting buffer for the next segment.

use super::super::{Csr, Nbr};
use graphdb_core::types::Timestamp;

/// Tracks free segment slots grouped by an upper-bound capacity class.
#[derive(Debug, Default)]
pub struct SegmentFreeList {
    /// Free segment slots grouped by capacity tier.
    free_slots: Vec<Vec<usize>>,
    slot_capacities: Vec<usize>,
    slot_available: Vec<bool>,
    buffers: Vec<Option<Csr>>,
}

impl SegmentFreeList {
    /// Create an empty free-list.
    pub fn new() -> Self {
        Self::default()
    }

    /// Return the capacity tier containing `capacity`.
    fn capacity_tier(capacity: usize) -> usize {
        if capacity <= 1 {
            0
        } else {
            (usize::BITS - (capacity - 1).leading_zeros()) as usize
        }
    }

    fn ensure_slot(&mut self, slot: usize) {
        if slot >= self.slot_capacities.len() {
            self.slot_capacities.resize(slot + 1, 0);
            self.slot_available.resize(slot + 1, false);
            self.buffers.resize_with(slot + 1, || None);
        }
    }

    fn ensure_tier(&mut self, tier: usize) {
        if tier >= self.free_slots.len() {
            self.free_slots.resize_with(tier + 1, Vec::new);
        }
    }

    /// Allocate the smallest free slot whose capacity can hold the request.
    pub fn allocate(&mut self, required_capacity: usize) -> Option<usize> {
        let first_tier = Self::capacity_tier(required_capacity);
        let mut best: Option<(usize, usize, usize, usize)> = None;

        for tier in first_tier..self.free_slots.len() {
            for (position, slot) in self.free_slots[tier].iter().copied().enumerate() {
                let capacity = self.slot_capacities.get(slot).copied().unwrap_or(0);
                let available = self.slot_available.get(slot).copied().unwrap_or(false);
                if !available || capacity < required_capacity {
                    continue;
                }

                let candidate = (capacity, tier, position, slot);
                if best.is_none_or(|current| candidate.0 < current.0) {
                    best = Some(candidate);
                }
            }
        }

        let (_, tier, position, slot) = best?;
        self.free_slots[tier].swap_remove(position);
        self.slot_available[slot] = false;
        Some(slot)
    }

    /// Return a slot to the free list.
    pub fn free(&mut self, slot: usize, capacity: usize) {
        self.ensure_slot(slot);
        if self.slot_available[slot] {
            return;
        }

        self.slot_capacities[slot] = capacity;
        self.slot_available[slot] = true;
        let tier = Self::capacity_tier(capacity);
        self.ensure_tier(tier);
        self.free_slots[tier].push(slot);
    }

    /// Move a retired CSR into the pool and return its slot id.
    pub fn recycle_csr(&mut self, csr: Csr) -> usize {
        let capacity = csr.allocated_memory_size();
        let slot = self.buffers.len();
        self.buffers.push(Some(csr));
        self.slot_capacities.push(capacity);
        self.slot_available.push(false);
        self.free(slot, capacity);
        slot
    }

    /// Take a reusable CSR buffer with enough allocated capacity.
    pub fn take_reusable_csr(&mut self, required_capacity: usize) -> Option<Csr> {
        while let Some(slot) = self.allocate(required_capacity) {
            if let Some(buffer) = self.buffers[slot].take() {
                return Some(buffer);
            }
        }
        None
    }

    /// Build a CSR using a reusable buffer when one is available.
    pub fn build_csr(&mut self, entries: &[(u32, Nbr, Timestamp)], vertex_capacity: usize) -> Csr {
        let required_capacity = Csr::required_memory_size(vertex_capacity, entries.len());
        let mut csr = self
            .take_reusable_csr(required_capacity)
            .unwrap_or_default();
        csr.rebuild_from_nbr_entries(entries, vertex_capacity);
        csr
    }

    /// Drop all retained buffers and reset slot metadata.
    pub fn clear(&mut self) {
        self.free_slots.clear();
        self.slot_capacities.clear();
        self.slot_available.clear();
        self.buffers.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edge::CsrBase;
    use graphdb_core::types::{EdgeId, VertexId};

    #[test]
    fn allocates_best_fit_slot() {
        let mut free_list = SegmentFreeList::new();
        free_list.free(10, 128);
        free_list.free(11, 512);
        free_list.free(12, 256);

        assert_eq!(free_list.allocate(200), Some(12));
        assert_eq!(free_list.allocate(400), Some(11));
        assert_eq!(free_list.allocate(600), None);
    }

    #[test]
    fn recycled_csr_is_reused_for_segment_building() {
        let entries = vec![(0, Nbr::new(VertexId::from_int64(1), EdgeId(1)), 1u64)];
        let mut free_list = SegmentFreeList::new();
        let original = Csr::from_nbr_entries(&entries, 4);
        free_list.recycle_csr(original);

        let rebuilt = free_list.build_csr(&entries, 4);
        assert_eq!(rebuilt.edge_count(), 1);
    }
}
