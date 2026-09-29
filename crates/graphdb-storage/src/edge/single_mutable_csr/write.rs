//! Write path for `SingleMutableCsr`: insert, delete, revert and rollback.

use graphdb_core::{StorageError, StorageResult};

use super::super::csr_shared::{
    can_revert_delete, decide_slot_delete, decode_endpoint_pair, DeleteSlotOutcome,
};
use super::super::{EdgeId, Timestamp, VertexId, INVALID_EDGE_ID};
use super::{empty_slot, SingleMutableCsr};

impl SingleMutableCsr {
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

        let (existing_hot, existing_cold) = match self.slot_at(src_idx) {
            Some(probe) => (probe.hot(), probe.cold()),
            None => (
                super::super::HotNbr::dead_gap(),
                super::super::ColdStamps::dead_gap(),
            ),
        };

        // Physical uniqueness only: a live slot rejects the second insert
        // regardless of timestamp, while a tombstoned or empty slot accepts
        // any timestamp. Snapshot visibility is decided by the version
        // authority above this layer.
        if existing_cold.is_live() && existing_hot.edge_id != INVALID_EDGE_ID {
            return Err(StorageError::conflict(format!(
                "[SingleMutableCsr] insert conflict on src={}: slot holds live edge {:?}",
                src, existing_hot.edge_id
            )));
        }

        let was_empty = existing_hot.edge_id == INVALID_EDGE_ID || !existing_cold.is_live();
        let (decoded_endpoint, rank) = decode_endpoint_pair(dst).ok_or_else(|| {
            StorageError::invalid_input(format!("Malformed edge endpoint key: {}", dst))
        })?;
        let segment = self.slot_mut_or_alloc(src_idx);
        let (_, off) = Self::locate(src_idx);
        segment.hot[off] = super::super::HotNbr {
            endpoint: decoded_endpoint,
            rank,
            edge_id,
        };
        segment.cold[off] = super::super::ColdStamps {
            delete_ts: Timestamp::MAX,
        };
        self.set_present(src_idx, true);

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

        self.stamp_delete(src_idx, ts);
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

        let (dst_ep, dst_rank) = match decode_endpoint_pair(dst) {
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
            self.clear_delete(src_idx);
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
        let edge_id = self
            .hot_at(src_idx)
            .map(|hot| hot.edge_id)
            .unwrap_or(INVALID_EDGE_ID);
        self.delete_edge(src, edge_id, ts)
    }

    pub fn rollback_insert(&mut self, src: u32, edge_id: EdgeId) -> bool {
        let src_idx = src as usize;
        if src_idx >= self.vertex_capacity() {
            return false;
        }
        let hot = match self.hot_at(src_idx) {
            Some(h) => h,
            None => return false,
        };
        if hot.edge_id == INVALID_EDGE_ID {
            return false;
        }
        if hot.edge_id != edge_id {
            return false;
        }
        let was_live = self.cold_is_live_at(src_idx);
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
            self.clear_delete(src_idx);
            self.edge_count += 1;
            return true;
        }
        false
    }
}
