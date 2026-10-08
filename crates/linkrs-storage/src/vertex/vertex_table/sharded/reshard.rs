//! Offline redistribution: read-only dry run and full rebuild to a new
//! shard count, with generation evolution and id-translation mapping.

use super::routing::ShardLayout;
use super::ShardedVertexTable;

/// Read-only redistribution preview: live rows plus their distribution under
/// the target shard count. Produced without copying any row; the rebuild
/// entry reuses it as its pre-copy gate and checks the produced mapping
/// against it afterwards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReshardDryRun {
    pub live_rows: usize,
    pub new_num_shards: usize,
    pub per_shard_rows: Vec<usize>,
}

impl ShardedVertexTable {
    /// Offline redistribution to a new shard count.
    ///
    /// Reads every live row at the maximum timestamp and rebuilds it in a
    /// fresh table with `new_num_shards`, returning the rebuilt table plus
    /// the old-global to new-global internal id mapping the rebuild
    /// produced. Row creation stamps travel with their rows so snapshot
    /// reads keep their original visibility lower bound; per-value version
    /// chains stay memory-only and are not migrated. The source stays
    /// untouched; the caller flushes the returned table and checkpoints it
    /// as the new baseline, then retires the old checkpoint directory.
    /// Online shard count changes stay rejected by the table manifest; this
    /// is the only adjustment outlet.
    ///
    /// Offline fence: the caller must hold the maintenance barrier with no
    /// concurrent writes, and no staged schema change may be pending on any
    /// shard. A pending schema change rejects the rebuild so a half-applied
    /// schema cannot leak into the new shard layout.
    ///
    /// The rebuilt table carries the next redistribution generation, so a
    /// checkpoint flushed from it can never be mistaken for one from the
    /// source lineage at open.
    ///
    /// The read-only precheck runs first: illegal layouts, pending schema
    /// changes, malformed keys, index entries without a retrievable record,
    /// and target capacity overflows fail before any row is copied. After
    /// the copy the produced mapping must cover every live row, otherwise
    /// the rebuild fails instead of handing out a partial edge-translation
    /// map.
    pub fn dry_run_reshard_to(
        &self,
        new_num_shards: usize,
    ) -> linkrs_core::StorageResult<ReshardDryRun> {
        use linkrs_core::types::MAX_TIMESTAMP;
        let target = ShardLayout::for_new_table(new_num_shards);
        if !target.is_consistent() {
            return Err(linkrs_core::StorageError::invalid_operation(format!(
                "reshard refused: target layout for {} shards is inconsistent",
                new_num_shards
            )));
        }
        if target == self.layout {
            return Err(linkrs_core::StorageError::invalid_operation(format!(
                "reshard is a no-op: table already uses {} shards",
                self.layout.num_shards
            )));
        }
        for (idx, shard) in self.shards.iter().enumerate() {
            if shard.read().has_pending_schema_change() {
                return Err(linkrs_core::StorageError::invalid_operation(format!(
                    "reshard refused: shard {} holds a pending schema change; \
                     finish or abort it before offline redistribution",
                    idx
                )));
            }
        }
        let ts = MAX_TIMESTAMP - 1;
        let mut per_shard_rows = vec![0usize; target.num_shards];
        let mut live_rows = 0usize;
        let mask = target.num_shards - 1;
        for key in self.external_id_keys() {
            match &key {
                crate::vertex::IdKey::Text(name) => {
                    if name.len() > linkrs_core::types::VERTEX_ID_MAX_SIZE {
                        return Err(linkrs_core::StorageError::invalid_input(format!(
                            "reshard refused: text key of {} bytes exceeds the limit",
                            name.len()
                        )));
                    }
                    let Some(old_global) = self.get_internal_id(name, ts) else {
                        continue;
                    };
                    let Some(_) = self.get_by_internal_id_offline(old_global, ts) else {
                        return Err(linkrs_core::StorageError::invalid_operation(
                            "reshard refused: indexed text key maps to a missing record; \
                             refusing a partial edge-translation map"
                                .to_string(),
                        ));
                    };
                    let shard = (super::routing::fxhash(name) as usize) & mask;
                    per_shard_rows[shard] += 1;
                    live_rows += 1;
                }
                crate::vertex::IdKey::Int(n) => {
                    if *n < 0 {
                        return Err(linkrs_core::StorageError::invalid_input(format!(
                            "reshard refused: negative integer key {}",
                            n
                        )));
                    }
                    let Some(old_global) = self.get_internal_id_by_i64(*n, ts) else {
                        continue;
                    };
                    let Some(_) = self.get_by_internal_id_offline(old_global, ts) else {
                        return Err(linkrs_core::StorageError::invalid_operation(format!(
                            "reshard refused: indexed integer key {} maps to a missing record; \
                             refusing a partial edge-translation map",
                            n
                        )));
                    };
                    let shard = (super::routing::fxhash_i64(*n) as usize) & mask;
                    per_shard_rows[shard] += 1;
                    live_rows += 1;
                }
            }
        }
        let per_shard_capacity = (target.total_segments / target.num_shards as u32) as u64
            * target.segment_slots() as u64;
        for (shard, rows) in per_shard_rows.iter().enumerate() {
            if *rows as u64 > per_shard_capacity {
                return Err(linkrs_core::StorageError::invalid_operation(format!(
                    "reshard refused: target shard {} would hold {} rows beyond the address space {}",
                    shard, rows, per_shard_capacity
                )));
            }
        }
        Ok(ReshardDryRun {
            live_rows,
            new_num_shards: target.num_shards,
            per_shard_rows,
        })
    }

    pub fn reshard_to(
        &self,
        new_num_shards: usize,
    ) -> linkrs_core::StorageResult<(Self, std::collections::HashMap<u32, u32>)> {
        use linkrs_core::types::MAX_TIMESTAMP;
        let preview = self.dry_run_reshard_to(new_num_shards)?;
        let target = ShardLayout::for_new_table(preview.new_num_shards);
        let schema = self.schema();
        let rebuilt = Self::with_layout(
            self.label,
            self.label_name.clone(),
            schema,
            target,
            self.generation.saturating_add(1),
        );
        let ts = MAX_TIMESTAMP - 1;
        let mut id_mapping: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
        for key in self.external_id_keys() {
            let (old_global, record, orig_create) = match &key {
                crate::vertex::IdKey::Text(name) => {
                    let Some(old_global) = self.get_internal_id(name, ts) else {
                        continue;
                    };
                    let Some(record) = self.get_by_internal_id_offline(old_global, ts) else {
                        continue;
                    };
                    let create = self
                        .row_timestamps(old_global)
                        .map(|(create, _)| create)
                        .unwrap_or(ts);
                    (old_global, record, create)
                }
                crate::vertex::IdKey::Int(n) => {
                    let Some(old_global) = self.get_internal_id_by_i64(*n, ts) else {
                        continue;
                    };
                    let Some(record) = self.get_by_internal_id_offline(old_global, ts) else {
                        continue;
                    };
                    let create = self
                        .row_timestamps(old_global)
                        .map(|(create, _)| create)
                        .unwrap_or(ts);
                    (old_global, record, create)
                }
            };
            let new_global = match &key {
                crate::vertex::IdKey::Text(name) => rebuilt.insert(name, &record.properties, ts)?,
                crate::vertex::IdKey::Int(n) => {
                    rebuilt.insert_by_i64(*n, &record.properties, ts)?
                }
            };
            if orig_create < ts {
                let (new_shard, new_local) = rebuilt.decode_id(new_global);
                rebuilt.shards[new_shard]
                    .read()
                    .backdate_row_for_reshard(new_local, orig_create);
            }
            id_mapping.insert(old_global, new_global);
        }
        if id_mapping.len() != preview.live_rows
            || rebuilt.approximate_total_count() != preview.live_rows
        {
            return Err(linkrs_core::StorageError::invalid_operation(format!(
                "reshard incomplete: preview counted {} live rows but the rebuild mapped {} into {} rows; \
                 refusing a partial edge-translation map",
                preview.live_rows,
                id_mapping.len(),
                rebuilt.approximate_total_count(),
            )));
        }
        Ok((rebuilt, id_mapping))
    }
}
