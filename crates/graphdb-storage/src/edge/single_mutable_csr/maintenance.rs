//! Positional delete/revert and maintenance for `SingleMutableCsr`:
//! position-addressed writes, reclaim probes and compaction.

use graphdb_core::StorageResult;

use super::super::csr_shared::{
    can_revert_delete, decide_slot_delete, is_reclaimable_cold, DeleteSlotOutcome,
};
use super::super::{EdgeId, EdgePosition, Timestamp, INVALID_EDGE_ID};
use super::{empty_slot, SingleMutableCsr};

impl SingleMutableCsr {
    /// Delete the single slot at `position` when it holds `expected`.
    ///
    /// Only the row-local primary slot zero is valid; every other position
    /// is stale and refused without touching any slot.
    pub fn delete_edge_at_position(
        &mut self,
        src: u32,
        position: EdgePosition,
        expected: EdgeId,
        ts: Timestamp,
    ) -> StorageResult<bool> {
        if !matches!(position, EdgePosition::Primary { slot: 0 }) {
            return Ok(false);
        }
        let src_idx = src as usize;
        if src_idx >= self.vertex_capacity() {
            return Ok(false);
        }
        let probe = match self.slot_at(src_idx) {
            Some(probe) => probe,
            None => return Ok(false),
        };
        if probe.edge_id == INVALID_EDGE_ID || probe.edge_id != expected {
            return Ok(false);
        }
        if matches!(
            decide_slot_delete(&probe, expected, ts)?,
            DeleteSlotOutcome::AlreadyStamped
        ) {
            return Ok(false);
        }
        self.stamp_delete(src_idx, ts);
        self.edge_count -= 1;
        Ok(true)
    }

    /// Revert the single-slot deletion at `position` when it holds `expected`.
    pub fn revert_delete_at_position(
        &mut self,
        src: u32,
        position: EdgePosition,
        expected: EdgeId,
        ts: Timestamp,
    ) -> bool {
        if !matches!(position, EdgePosition::Primary { slot: 0 }) {
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
        if probe.edge_id == INVALID_EDGE_ID || probe.edge_id != expected {
            return false;
        }
        if can_revert_delete(&probe, ts) {
            self.clear_delete(src_idx);
            self.edge_count += 1;
            return true;
        }
        false
    }

    /// Delete the single matching entry, reporting its row-local position.
    pub fn delete_edge_by_dst_reporting_positioned(
        &mut self,
        src: u32,
        dst: super::super::VertexId,
        ts: Timestamp,
        on_deleted: &mut dyn FnMut(EdgeId, Option<EdgePosition>),
    ) -> usize {
        let src_idx = src as usize;
        if src_idx >= self.vertex_capacity() {
            return 0;
        }
        let (dst_ep, dst_rank) = match super::super::csr_shared::decode_endpoint_pair(dst) {
            Some(pair) => pair,
            None => return 0,
        };
        let probe = match self.slot_at(src_idx) {
            Some(probe) => probe,
            None => return 0,
        };
        if probe.edge_id == INVALID_EDGE_ID
            || probe.endpoint != dst_ep
            || probe.rank != dst_rank
            || !self.cold_is_live_at(src_idx)
        {
            return 0;
        }
        self.stamp_delete(src_idx, ts);
        self.edge_count -= 1;
        on_deleted(probe.edge_id, Some(EdgePosition::Primary { slot: 0 }));
        1
    }

    pub fn reclaimable_count(&self, vid: u32, cutoff: Timestamp) -> usize {
        if cutoff == Timestamp::MAX {
            return 0;
        }
        let hot = self
            .hot_at(vid as usize)
            .unwrap_or(super::super::HotNbr::dead_gap());
        let cold = self
            .cold_at(vid as usize)
            .unwrap_or(super::super::ColdStamps::dead_gap());
        if hot.edge_id != INVALID_EDGE_ID && is_reclaimable_cold(&cold, cutoff) {
            1
        } else {
            0
        }
    }

    pub fn vertex_census(&self, vid: u32) -> (usize, usize, usize) {
        let hot = match self.hot_at(vid as usize) {
            Some(h) => h,
            None => return (0, 0, 0),
        };
        let cold = self
            .cold_at(vid as usize)
            .unwrap_or(super::super::ColdStamps::dead_gap());
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
        let hot = match self.hot_at(vid as usize) {
            Some(h) => h,
            None => return (0, 0),
        };
        let cold = self
            .cold_at(vid as usize)
            .unwrap_or(super::super::ColdStamps::dead_gap());
        if hot.edge_id == INVALID_EDGE_ID || cold.is_live() {
            return (0, 0);
        }
        let reclaimable =
            usize::from(cutoff != Timestamp::MAX && is_reclaimable_cold(&cold, cutoff));
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
        let edge_id = self
            .hot_at(src_idx)
            .map(|h| h.edge_id)
            .unwrap_or(INVALID_EDGE_ID);
        let delete_ts = self
            .cold_at(src_idx)
            .map(|c| c.delete_ts)
            .unwrap_or(Timestamp::MAX);
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
        let mut reclaimable: Vec<(EdgeId, Timestamp, usize)> = Vec::new();
        for seg_idx in 0..self.segments.len() {
            let Some(segment) = self.segments.get(seg_idx).and_then(|s| s.as_ref()) else {
                continue;
            };
            for off in 0..Self::SEGMENT_ROWS {
                let hot = segment.hot[off];
                let cold = segment.cold[off];
                if hot.edge_id != INVALID_EDGE_ID && is_reclaimable_cold(&cold, cutoff) {
                    let idx = seg_idx * Self::SEGMENT_ROWS + off;
                    reclaimable.push((hot.edge_id, cold.delete_ts, idx));
                }
            }
        }
        let mut removed = 0usize;
        for (edge_id, delete_ts, idx) in reclaimable {
            on_edge_removed(edge_id, delete_ts);
            self.set_slot(idx, empty_slot());
            removed += 1;
        }
        removed
    }

    pub fn clear(&mut self) {
        self.segments.clear();
        self.present.clear();
        self.vertex_capacity = 0;
        self.edge_count = 0;
    }
}
