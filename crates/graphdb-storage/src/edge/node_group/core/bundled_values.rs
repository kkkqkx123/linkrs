//! Bundled-edge value reads/writes and value-aware traversal.

use super::super::super::{csr_shared::decode_endpoint_pair, Nbr, RecordForm};
use super::super::local_vid;
use super::CsrShardSet;
use graphdb_core::types::{EdgeId, Timestamp, VertexId};
use graphdb_core::{StorageError, StorageResult};

impl CsrShardSet {
    /// Whether this direction stores its single scalar inline.
    pub fn is_bundled(&self) -> bool {
        self.record_form == RecordForm::Bundled
    }

    /// Insert one edge carrying its inline value (`None` stores NULL).
    ///
    /// Dirt and append-log entries mirror the single-edge insert path. Only
    /// bundled sets accept a value; anything else is rejected by the variant.
    pub fn bundled_insert_with_value(
        &mut self,
        src_vid: u32,
        dst: VertexId,
        edge_id: EdgeId,
        ts: Timestamp,
        value: Option<u64>,
    ) -> StorageResult<()> {
        let gid = self.ensure_group_for(src_vid)?;
        let local = local_vid(src_vid, self.group_bits);
        self.shards
            .get_mut(&gid)
            .ok_or_else(|| {
                StorageError::invalid_operation(format!("missing group {} on insert", gid))
            })?
            .variant
            .insert_edge_with_value(local, dst, edge_id, value)?;
        let (decoded_endpoint, decoded_rank) = decode_endpoint_pair(dst).ok_or_else(|| {
            StorageError::invalid_input(format!("Malformed edge endpoint key: {}", dst))
        })?;
        let nbr = Nbr::with_create_ts(decoded_endpoint, decoded_rank, edge_id, ts);
        self.mark_region_insert(gid, local);
        self.record_append_insert(gid, local, nbr);
        Ok(())
    }

    /// Read one edge's inline value within its source row.
    pub fn bundled_value_at(&self, src_vid: u32, edge_id: EdgeId) -> Option<(u64, bool)> {
        let (gid, local) = self.route(src_vid)?;
        let shard = self.shards.get(&gid)?;
        if let Some(mapped) = &shard.mapped {
            return mapped.bundled_value_by_edge_id(local, edge_id);
        }
        shard.variant.bundled_value_by_edge_id(local, edge_id)
    }

    /// Read the inline value of the live edge for one endpoint.
    pub fn bundled_value_by_endpoint(&self, src_vid: u32, endpoint: u32) -> Option<(u64, bool)> {
        let (gid, local) = self.route(src_vid)?;
        let shard = self.shards.get(&gid)?;
        if let Some(mapped) = &shard.mapped {
            return mapped.bundled_value_by_endpoint(local, endpoint);
        }
        shard.variant.bundled_value_by_endpoint(local, endpoint)
    }

    /// Overwrite the inline value of the live edge for one endpoint.
    ///
    /// The value column rides the group base file rather than the append
    /// sidecar, so the update takes delete dirt to force a base rewrite on
    /// the next checkpoint. No append record is written: append replay only
    /// expresses topology inserts and deletes.
    pub fn bundled_set_value_by_endpoint(
        &mut self,
        src_vid: u32,
        endpoint: u32,
        value: Option<u64>,
    ) -> bool {
        let Some((gid, local)) = self.route(src_vid) else {
            return false;
        };
        let updated = self
            .shards
            .get_mut(&gid)
            .map(|shard| {
                shard
                    .variant
                    .bundled_set_value_by_endpoint(local, endpoint, value)
            })
            .unwrap_or(false);
        if updated {
            self.mark_region_delete(gid, local);
        }
        updated
    }

    /// Revert a deletion restoring the caller's value alongside the topology.
    pub fn bundled_revert_with_value(
        &mut self,
        src_vid: u32,
        position: super::super::super::EdgePosition,
        expected: EdgeId,
        ts: Timestamp,
        value: Option<u64>,
    ) -> bool {
        let Some((gid, local)) = self.route(src_vid) else {
            return false;
        };
        let reverted = self
            .shards
            .get_mut(&gid)
            .map(|shard| {
                shard
                    .variant
                    .bundled_revert_with_value(local, position, expected, ts, value)
            })
            .unwrap_or(false);
        if reverted {
            self.mark_region_delete(gid, local);
            if let Some(shard) = self.shards.get_mut(&gid) {
                shard.dead_entries = shard.dead_entries.saturating_sub(1);
                if shard.dead_entries == 0 {
                    shard.reclaim_hint = false;
                }
            }
        }
        reverted
    }

    /// Visit every physically stored entry of one vertex with its inline
    /// value (`None` for NULL slots and non-bundled forms).
    pub fn visit_physical_with_values<F>(&self, src_vid: u32, f: F)
    where
        F: FnMut(Nbr, Option<u64>) -> bool,
    {
        if let Some((gid, local)) = self.route(src_vid) {
            if let Some(shard) = self.shards.get(&gid) {
                if let Some(mapped) = &shard.mapped {
                    mapped.visit_physical_with_values(local, f);
                } else {
                    shard.variant.visit_physical_with_values(local, f);
                }
            }
        }
    }

    /// Fill a caller buffer with paired topology plus inline values.
    ///
    /// Missing groups read as empty and never create groups. Batch scans
    /// reuse one buffer across vertices through the paired walk.
    pub fn fill_physical_with_values_into(&self, src_vid: u32, out: &mut Vec<(Nbr, Option<u64>)>) {
        let Some((gid, local)) = self.route(src_vid) else {
            out.clear();
            return;
        };
        match self.shards.get(&gid) {
            Some(shard) => {
                if let Some(mapped) = &shard.mapped {
                    out.clear();
                    mapped.visit_physical_with_values(local, |nbr, value| {
                        out.push((nbr, value));
                        true
                    });
                } else {
                    shard.variant.fill_physical_with_values_into(local, out);
                }
            }
            None => out.clear(),
        }
    }
}
