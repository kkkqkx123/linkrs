//! Explicit freeze and unfreeze of single groups.
//!
//! Freezing packs one group's mutable variant into the frozen packed form:
//! one contiguous neighbor segment plus a degree table, with no capacities,
//! overflow chains, live index or locks. The packed rows preserve the logical
//! content of the source group (tombstones verbatim, gap sentinels dropped)
//! in sorted `(endpoint, rank, edge_id)` row order, so
//! timestamp-filtered reads observe the same logical entries before and
//! after. Unfreezing rebuilds a fresh mutable variant by replaying the packed
//! rows through the regular insert and delete entries, restoring the same
//! logical content in fresh physical order.
//!
//! Both operations are explicit. Freeze optionally reclaims first at a caller
//! cutoff through the shared group compaction, reporting removals for the
//! authority above. Frozen groups reject writes; freezing marks the group
//! deleted-dirty so the next checkpoint rewrites the base file and drops the
//! absorbed append sidecar.

use graphdb_core::types::{EdgeId, Timestamp};
use graphdb_core::{StorageError, StorageResult};

use super::super::{CsrVariant, ImmutableCsr, MutableCsrTrait, INVALID_EDGE_ID};
use super::CsrShardSet;

impl CsrShardSet {
    /// Whether one existing group is frozen.
    pub fn is_frozen(&self, gid: usize) -> bool {
        self.shards.get(&gid).is_some_and(|shard| {
            matches!(shard.variant, CsrVariant::Frozen(_) | CsrVariant::Mapped(_))
        })
    }

    /// Whether the group owning `vid` exists and is frozen.
    pub fn is_group_frozen_for(&self, vid: u32) -> bool {
        let Some((gid, _)) = self.route(vid) else {
            return false;
        };
        self.is_frozen(gid)
    }

    /// Freeze one group in place, returning its packed live edge count.
    ///
    /// Reclaims first at `cutoff` through the shared group compaction when
    /// the caller passes an active cutoff, reporting removals through
    /// `on_edge_removed` for the authority above; a maximum cutoff packs
    /// verbatim with no removals. Only `Multiple`, `Single`, `Pure` and
    /// `Bundled` groups freeze; missing groups, `None` groups and
    /// already-frozen groups are rejected.
    pub fn freeze_group(
        &mut self,
        gid: usize,
        cutoff: Timestamp,
        reserve_ratio: f32,
        on_edge_removed: &mut dyn FnMut(EdgeId, Timestamp),
    ) -> StorageResult<u64> {
        let Some(shard) = self.shards.get(&gid) else {
            return Err(StorageError::invalid_operation(format!(
                "group {} missing, nothing to freeze",
                gid
            )));
        };
        // The frozen packer stores topology only: freezing a bundled group
        // with valid inline values would drop them, so it is rejected up
        // front. Run migrate_record_form to the columnar form first when a
        // freeze is required; all-NULL bundled groups pack like pure topologies.
        if shard.variant.bundled_has_valid_values() {
            return Err(StorageError::invalid_operation(format!(
                "group {} holds bundled inline values; run migrate_record_form to the columnar form before freeze",
                gid
            )));
        }
        match shard.variant {
            CsrVariant::Multiple(_)
            | CsrVariant::Single(_)
            | CsrVariant::Pure(_)
            | CsrVariant::Bundled(_) => {}
            CsrVariant::Frozen(_) | CsrVariant::Mapped(_) => {
                return Err(StorageError::invalid_operation(format!(
                    "group {} is already frozen",
                    gid
                )));
            }
            CsrVariant::None { .. } => {
                return Err(StorageError::invalid_operation(
                    "no edges stored for this edge type".to_string(),
                ));
            }
        }
        self.compact_group_with_reporting(gid, cutoff, reserve_ratio, on_edge_removed);
        let shard = self.shards.get_mut(&gid).ok_or_else(|| {
            StorageError::invalid_operation(format!("group {} missing on freeze", gid))
        })?;
        let frozen = match &shard.variant {
            CsrVariant::Multiple(csr) => ImmutableCsr::pack_from_mutable(csr),
            CsrVariant::Single(csr) => ImmutableCsr::pack_single_from(csr),
            CsrVariant::Pure(csr) => {
                let rows = csr.vertex_capacity();
                let mut temp = super::super::MutableCsr::with_capacity(rows, 0);
                for local in 0..rows as u32 {
                    let edges = csr.physical_edges_of(local);
                    for nbr in edges {
                        if nbr.edge_id == INVALID_EDGE_ID {
                            continue;
                        }
                        temp.insert_edge(local, nbr.to_vertex_id(), nbr.edge_id, 0)?;
                    }
                }
                ImmutableCsr::pack_from_mutable(&temp)
            }
            CsrVariant::Bundled(csr) => {
                let rows = csr.vertex_capacity();
                let mut temp = super::super::MutableCsr::with_capacity(rows, 0);
                for local in 0..rows as u32 {
                    let edges = csr.physical_edges_of(local);
                    for nbr in edges {
                        if nbr.edge_id == INVALID_EDGE_ID {
                            continue;
                        }
                        temp.insert_edge(local, nbr.to_vertex_id(), nbr.edge_id, 0)?;
                    }
                }
                ImmutableCsr::pack_from_mutable(&temp)
            }
            CsrVariant::Frozen(_) | CsrVariant::Mapped(_) | CsrVariant::None { .. } => {
                return Err(StorageError::invalid_operation(format!(
                    "group {} changed under freeze",
                    gid
                )));
            }
        };
        let packed_edges = frozen.edge_count();
        shard.variant = CsrVariant::Frozen(Box::new(frozen));
        shard.dirty.deleted = true;
        for region in shard.regions.iter_mut() {
            region.deleted = true;
        }
        shard.reclaim_hint = false;
        self.clear_group_append_log(gid);
        Ok(packed_edges)
    }

    /// Unfreeze one group, rebuilding its mutable variant.
    ///
    /// Replays every packed row through the regular insert entry and restores
    /// deletion stamps through the regular delete entry, so the rebuilt rows
    /// carry the same logical content in fresh physical order. Gap sentinels
    /// are not replayed: they are reserved-slot fillers, never edges. The
    /// rebuilt shape follows the set strategy. Only frozen groups unfreeze;
    /// anything else is rejected.
    pub fn unfreeze_group(&mut self, gid: usize) -> StorageResult<u64> {
        let frozen = match self.shards.get(&gid) {
            Some(shard) => match &shard.variant {
                CsrVariant::Frozen(csr) => csr.clone(),
                // Mapped groups replay from identical authoritative bytes:
                // the dump is byte-equal to the heap frozen dump by
                // construction, so the rebuilt content matches.
                CsrVariant::Mapped(csr) => {
                    let mut heap = ImmutableCsr::new();
                    heap.load(&csr.dump())?;
                    Box::new(heap)
                }
                _ => {
                    return Err(StorageError::invalid_operation(format!(
                        "group {} is not frozen",
                        gid
                    )));
                }
            },
            None => {
                return Err(StorageError::invalid_operation(format!(
                    "group {} missing, nothing to unfreeze",
                    gid
                )));
            }
        };
        let mut variant = self.fresh_variant()?;
        let rows = frozen.vertex_capacity();
        let mut row_buf = Vec::new();
        for local in 0..rows {
            frozen.fill_physical_into(local as u32, &mut row_buf);
            for nbr in &row_buf {
                if nbr.edge_id == INVALID_EDGE_ID {
                    continue;
                }
                variant.insert_edge(
                    local as u32,
                    nbr.to_vertex_id(),
                    nbr.edge_id,
                    Timestamp::MAX,
                )?;
                if nbr.delete_ts != Timestamp::MAX {
                    let deleted = variant.delete_edge(local as u32, nbr.edge_id, nbr.delete_ts)?;
                    if !deleted {
                        return Err(StorageError::invalid_operation(format!(
                            "unfreeze replay missed edge {:?} in group {}",
                            nbr.edge_id, gid
                        )));
                    }
                }
            }
        }
        let restored = frozen.edge_count();
        let shard = self.shards.get_mut(&gid).ok_or_else(|| {
            StorageError::invalid_operation(format!("group {} missing on unfreeze", gid))
        })?;
        shard.variant = variant;
        shard.dirty.deleted = true;
        for region in shard.regions.iter_mut() {
            region.deleted = true;
        }
        shard.reclaim_hint = true;
        self.clear_group_append_log(gid);
        Ok(restored)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edge::csr_trait::CsrBase;
    use graphdb_core::types::{EdgeStrategy, VertexId};

    use super::super::DEFAULT_NODE_GROUP_BITS;

    use super::super::RecordForm;

    fn sample_set() -> CsrShardSet {
        let mut set = CsrShardSet::new(
            EdgeStrategy::Multiple,
            DEFAULT_NODE_GROUP_BITS,
            64,
            RecordForm::Columnar,
        )
        .unwrap();
        for src in 0..32u32 {
            for k in 0..3u32 {
                let dst = VertexId::edge_endpoint_key(src + k + 1, 0);
                set.insert_edge(src, dst, EdgeId((src * 10 + k) as u64), 1)
                    .unwrap();
            }
        }
        set.delete_edge(0, EdgeId(1), 5).unwrap();
        set
    }

    #[test]
    fn freeze_rejects_writes_and_unfreeze_restores() {
        let mut set = sample_set();
        let before: Vec<_> = (0..32u32).map(|src| set.physical_edges_of(src)).collect();
        let packed = set
            .freeze_group(0, Timestamp::MAX, 0.0, &mut |_, _| {})
            .unwrap();
        assert_eq!(packed, set.edge_count());
        assert!(set.is_frozen(0));
        assert!(set.is_group_frozen_for(3));
        assert!(!set.is_group_frozen_for(1 << DEFAULT_NODE_GROUP_BITS));

        for src in 0..32u32 {
            assert_eq!(set.physical_edges_of(src), before[src as usize]);
            let expected: Vec<_> = before[src as usize]
                .iter()
                .filter(|nbr| nbr.is_alive_at(6))
                .copied()
                .collect();
            assert_eq!(set.edges_of(src, 6), expected);
        }
        assert!(set
            .insert_edge(1, VertexId::edge_endpoint_key(99, 0), EdgeId(1000), 9)
            .is_err());
        assert!(set.delete_edge(1, EdgeId(10), 9).is_err());
        assert_eq!(
            set.delete_edge_by_dst(1, VertexId::edge_endpoint_key(11, 0), 9),
            0
        );

        let restored = set.unfreeze_group(0).unwrap();
        assert_eq!(restored, packed);
        assert!(!set.is_frozen(0));
        for src in 0..32u32 {
            assert_eq!(set.physical_edges_of(src), before[src as usize]);
        }
        set.insert_edge(1, VertexId::edge_endpoint_key(99, 0), EdgeId(1000), 9)
            .unwrap();
        assert_eq!(set.edge_count(), packed + 1);
    }

    #[test]
    fn freeze_guards_bad_states() {
        let mut set = sample_set();
        assert!(set
            .freeze_group(77, Timestamp::MAX, 0.0, &mut |_, _| {})
            .is_err());
        set.freeze_group(0, Timestamp::MAX, 0.0, &mut |_, _| {})
            .unwrap();
        assert!(set
            .freeze_group(0, Timestamp::MAX, 0.0, &mut |_, _| {})
            .is_err());
        assert!(set.unfreeze_group(77).is_err());

        let mut none_set = CsrShardSet::new(
            EdgeStrategy::None,
            DEFAULT_NODE_GROUP_BITS,
            64,
            RecordForm::Columnar,
        )
        .unwrap();
        assert!(none_set
            .freeze_group(0, Timestamp::MAX, 0.0, &mut |_, _| {})
            .is_err());

        let mut single = CsrShardSet::new(
            EdgeStrategy::Single,
            DEFAULT_NODE_GROUP_BITS,
            64,
            RecordForm::Columnar,
        )
        .unwrap();
        single
            .insert_edge(5, VertexId::edge_endpoint_key(6, 0), EdgeId(1), 1)
            .unwrap();
        let packed = single
            .freeze_group(0, Timestamp::MAX, 0.0, &mut |_, _| {})
            .unwrap();
        assert_eq!(packed, 1);
        assert_eq!(
            single.edges_of(5, 1),
            vec![single.physical_edges_of(5).pop().unwrap()]
        );
        single.unfreeze_group(0).unwrap();
        assert!(!single.is_frozen(0));
        assert_eq!(single.edge_count(), 1);
    }
}
