//! Bulk edge insert partitioned by group, with reservation pre-sizing.

use super::super::super::{
    csr_shared::decode_endpoint_pair, CsrVariant, EdgePut, MutableCsrTrait, Nbr, RowEdgeBatch,
    NO_EDGES_STORED_MSG,
};
use super::super::{group_id_for, local_vid};
use super::CsrShardSet;
use graphdb_core::types::{EdgeId, EdgeStrategy, Timestamp, VertexId};
use graphdb_core::{StorageError, StorageResult};
use std::collections::BTreeMap;

impl CsrShardSet {
    /// Bulk insert pre-grouped edges with one reservation per touched row.
    ///
    /// Input is `(src, dst, edge_id)` triples at global addresses sharing one
    /// `ts`. The shared grouping and write path lives in
    /// [`Self::batch_put_edges_with_ts`].
    pub fn batch_put_edges(
        &mut self,
        edges: &[(u32, VertexId, EdgeId)],
        ts: Timestamp,
        check_duplicates: bool,
    ) -> StorageResult<usize> {
        let quads: Vec<(u32, VertexId, EdgeId, Timestamp)> = edges
            .iter()
            .map(|&(src, dst, edge_id)| (src, dst, edge_id, ts))
            .collect();
        self.batch_put_edges_with_ts(&quads, check_duplicates)
    }

    /// Bulk insert with per-edge timestamps, partitioned by group.
    ///
    /// Direct-write counterpart of `batch_put_edges` for initial loads with
    /// heterogeneous timestamps: input is `(src, dst, edge_id, ts)` quads at
    /// global addresses, rows are grouped by shard, each `Multiple` group is
    /// written through its bulk path (single reservation, single live-set
    /// rebuild per row) preserving each edge timestamp, and other variants
    /// fall back to per-edge inserts. Dirt and append-log entries are
    /// recorded per inserted edge exactly like the single-edge path.
    /// Duplicate keys are rejected before any write when `check_duplicates`
    /// is set; bulk-import callers prevalidate once and pass false.
    pub fn batch_put_edges_with_ts(
        &mut self,
        edges: &[(u32, VertexId, EdgeId, Timestamp)],
        check_duplicates: bool,
    ) -> StorageResult<usize> {
        if self.strategy == EdgeStrategy::None {
            return Err(StorageError::invalid_operation(
                NO_EDGES_STORED_MSG.to_string(),
            ));
        }
        let mut by_group: BTreeMap<usize, Vec<(u32, VertexId, EdgeId, Timestamp)>> =
            BTreeMap::new();
        for (src, dst, edge_id, ts) in edges {
            let gid = group_id_for(*src, self.group_bits);
            by_group
                .entry(gid)
                .or_default()
                .push((*src, *dst, *edge_id, *ts));
        }
        let mut inserted = 0usize;
        for (gid, group_edges) in by_group {
            let group_bits = self.group_bits;
            self.ensure_group_id(gid)?;
            let is_multiple = matches!(
                self.shards.get(&gid).map(|shard| &shard.variant),
                Some(CsrVariant::Multiple(_))
            );
            if is_multiple {
                let batch: Vec<RowEdgeBatch> = {
                    let mut rows: BTreeMap<u32, Vec<EdgePut>> = BTreeMap::new();
                    for (src, dst, edge_id, ts) in &group_edges {
                        let local = local_vid(*src, group_bits);
                        let (endpoint, rank) = decode_endpoint_pair(*dst).ok_or_else(|| {
                            StorageError::invalid_input(format!(
                                "Malformed edge endpoint key: {}",
                                dst
                            ))
                        })?;
                        rows.entry(local)
                            .or_default()
                            .push((endpoint, rank, *edge_id, *ts));
                    }
                    rows.into_iter().collect()
                };
                {
                    let shard = self.shards.get_mut(&gid).ok_or_else(|| {
                        StorageError::invalid_operation(format!("missing group {} on insert", gid))
                    })?;
                    let CsrVariant::Multiple(csr) = &mut shard.variant else {
                        return Err(StorageError::invalid_operation(format!(
                            "missing group {} on insert",
                            gid
                        )));
                    };
                    let counts: Vec<(u32, usize)> = batch
                        .iter()
                        .map(|(local, puts)| (*local, puts.len()))
                        .collect();
                    csr.reserve_for_batch(&counts);
                    inserted += csr.batch_put_edges(&batch, check_duplicates)?;
                }
                for (src, dst, edge_id, ts) in &group_edges {
                    let local = local_vid(*src, group_bits);
                    let (endpoint, rank) = decode_endpoint_pair(*dst).ok_or_else(|| {
                        StorageError::invalid_input(format!("Malformed edge endpoint key: {}", dst))
                    })?;
                    let nbr = Nbr::with_create_ts(endpoint, rank, *edge_id, *ts);
                    self.mark_region_insert(gid, local);
                    self.record_append_insert(gid, local, nbr);
                }
            } else {
                for (src, dst, edge_id, ts) in group_edges {
                    let local = local_vid(src, group_bits);
                    {
                        let shard = self.shards.get_mut(&gid).ok_or_else(|| {
                            StorageError::invalid_operation(format!(
                                "missing group {} on insert",
                                gid
                            ))
                        })?;
                        shard.variant.insert_edge(local, dst, edge_id, ts)?;
                    }
                    let (endpoint, rank) = decode_endpoint_pair(dst).ok_or_else(|| {
                        StorageError::invalid_input(format!("Malformed edge endpoint key: {}", dst))
                    })?;
                    let nbr = Nbr::with_create_ts(endpoint, rank, edge_id, ts);
                    self.mark_region_insert(gid, local);
                    self.record_append_insert(gid, local, nbr);
                    inserted += 1;
                }
            }
        }
        Ok(inserted)
    }

    /// Pre-size touched rows once for an incoming batch.
    ///
    /// Groups missing rows are materialized first; each `Multiple` group
    /// sizes its rows at the packed density target so the following
    /// per-edge inserts land in reserved gaps. Other strategies skip
    /// reservation. Only sizing happens here; inserts still flow through
    /// the regular path so dirt and append logs stay exact.
    pub fn reserve_for_batch(&mut self, counts: &[(u32, usize)]) -> StorageResult<()> {
        if self.strategy == EdgeStrategy::None {
            return Ok(());
        }
        let mut by_group: BTreeMap<usize, Vec<(u32, usize)>> = BTreeMap::new();
        for (vid, incoming) in counts {
            let gid = group_id_for(*vid, self.group_bits);
            by_group
                .entry(gid)
                .or_default()
                .push((local_vid(*vid, self.group_bits), *incoming));
        }
        for (gid, group_counts) in by_group {
            self.ensure_group_id(gid)?;
            let Some(shard) = self.shards.get_mut(&gid) else {
                continue;
            };
            if let CsrVariant::Multiple(csr) = &mut shard.variant {
                csr.reserve_for_batch(&group_counts);
            }
        }
        Ok(())
    }
}
