//! Per-group timestamp shard persistence.

use super::super::core::EdgeStore;
use super::layout::ts_group_path;
use graphdb_core::StorageResult;
use std::collections::{HashMap, HashSet};
use std::path::Path;

impl EdgeStore {
    /// Owner groups holding uncheckpointed timestamp writes: the owner
    /// direction's topology dirt. Timestamps change only on insert and
    /// delete, both of which dirty the owner topology group.
    fn timestamp_dirty_owners(&self) -> Vec<u32> {
        if self.schema.oe_strategy != crate::edge::EdgeStrategy::None {
            self.out_csr
                .dirty_group_ids()
                .into_iter()
                .map(|gid| gid as u32)
                .collect()
        } else if self.schema.ie_strategy != crate::edge::EdgeStrategy::None {
            self.in_csr
                .dirty_group_ids()
                .into_iter()
                .map(|gid| gid as u32)
                .collect()
        } else {
            Vec::new()
        }
    }

    /// Group authority entries of the wanted shards by owning shard, falling
    /// back to the smallest existing owner when the recorded owner is gone
    /// (reclaimed groups whose tombstone survives). Missing owners read as
    /// empty shards. Grouping memory follows the wanted set rather than the
    /// table size.
    fn grouped_timestamps(
        &self,
        wanted: &HashSet<u32>,
    ) -> HashMap<
        u32,
        Vec<(
            graphdb_core::types::EdgeId,
            crate::edge::edge_table::mvcc::EdgeTimestamps,
        )>,
    > {
        let owners = self.owner_group_ids();
        let fallback = owners.first().copied();
        let live: HashSet<u32> = owners.into_iter().collect();
        let mut grouped: HashMap<
            u32,
            Vec<(
                graphdb_core::types::EdgeId,
                crate::edge::edge_table::mvcc::EdgeTimestamps,
            )>,
        > = HashMap::new();
        let mut fallback_hits = 0usize;
        for (edge_id, ts) in self.mvcc.edge_timestamps.iter() {
            let (gid, fell_back) =
                Self::resolve_owner_gid(&edge_id, &self.edge_owner, &live, fallback);
            fallback_hits += usize::from(fell_back);
            if wanted.contains(&gid) {
                grouped.entry(gid).or_default().push((edge_id, *ts));
            }
        }
        if fallback_hits > 0 {
            log::debug!(
                "grouped_timestamps: {} authority entries fell back to smallest owner",
                fallback_hits
            );
        }
        for entries in grouped.values_mut() {
            entries.sort_by_key(|(edge_id, _)| edge_id.0);
        }
        grouped
    }

    pub(crate) fn flush_timestamp_shards(
        &self,
        dir: &Path,
        page_size: usize,
        level: i32,
    ) -> StorageResult<u64> {
        let mut dirty_set: HashSet<u32> = self.timestamp_dirty_owners().into_iter().collect();
        // A fresh checkpoint directory holds no shard files yet. Dirty
        // tracking alone would skip every owner after a preceding save
        // cleared the flags, leaving CSR bases without authority shards.
        // Missing files always join the write set so each checkpoint is a
        // complete snapshot while repeated flushes to the same data dir
        // stay incremental.
        for gid in self.owner_group_ids() {
            if !ts_group_path(dir, gid).exists() {
                dirty_set.insert(gid);
            }
        }
        if dirty_set.is_empty() {
            return Ok(0);
        }
        let mut dirty: Vec<u32> = dirty_set.iter().copied().collect();
        dirty.sort_unstable();
        let grouped = self.grouped_timestamps(&dirty_set);
        let mut written = 0u64;
        for gid in dirty {
            let entries = grouped.get(&gid).cloned().unwrap_or_default();
            let path = ts_group_path(dir, gid);
            if entries.is_empty() {
                if path.exists() {
                    let _ = std::fs::remove_file(&path);
                }
                continue;
            }
            let mut payload = Vec::new();
            super::super::persistence::serialize_timestamp_shard(
                &entries,
                crate::persistence::section::EDGE_TS_SHARD,
                &mut payload,
            )?;
            super::super::persistence::write_pages_to_file(
                &path,
                &payload,
                page_size,
                level,
                entries.len() as u32,
            )?;
            written += super::layout::file_bytes(&path);
        }
        Ok(written)
    }

    pub(crate) fn load_timestamp_shards(
        &mut self,
        dir: &Path,
        manifest: &crate::edge::node_group::TableShardManifest,
    ) -> StorageResult<()> {
        use graphdb_core::StorageError;
        self.mvcc.edge_timestamps.clear();
        let owners = self.owner_list_for_load(manifest);
        for gid in &owners {
            let path = ts_group_path(dir, *gid);
            if !path.exists() {
                continue;
            }
            let entries = super::super::persistence::load_timestamp_shard(
                &path,
                crate::persistence::section::EDGE_TS_SHARD,
            )?;
            for (edge_id, ts) in entries {
                if let Some(prev) = self.mvcc.edge_timestamps.get(&edge_id) {
                    if prev.create_ts != ts.create_ts || prev.delete_ts != ts.delete_ts {
                        return Err(StorageError::deserialize_error(format!(
                            "duplicate timestamp shard entry for edge {:?}",
                            edge_id
                        )));
                    }
                    continue;
                }
                self.mvcc.edge_timestamps.insert(edge_id, ts);
                self.edge_owner.or_insert(edge_id, *gid);
            }
        }
        Ok(())
    }
}
