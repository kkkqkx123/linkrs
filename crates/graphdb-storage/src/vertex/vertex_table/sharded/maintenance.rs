use super::ShardedVertexTable;
use crate::vertex::IdKey;
use graphdb_core::types::Timestamp;
use graphdb_core::StorageResult;

/// Fragmentation ratio threshold above which a shard is considered for
/// selective compaction. Segments with low fragmentation are skipped to
/// avoid global remapping.
pub(super) const SHARD_FRAGMENTATION_THRESHOLD: f64 = 0.25;

impl ShardedVertexTable {
    /// GC split into (reclaimed vertices, reclaimed version-chain entries).
    /// A nonzero vertex count means some shard re-densified internal IDs:
    /// caches keyed by internal ID must be invalidated for this label.
    /// The cutoff must be the global watermark safe timestamp.
    pub fn gc_detailed(&self, min_ts: Timestamp) -> StorageResult<(usize, usize)> {
        let mut reclaimed_vertices = 0;
        let mut version_entries = 0;
        for shard in &self.shards {
            let (vertices, versions) = shard.write().gc_detailed(min_ts)?;
            reclaimed_vertices += vertices;
            version_entries += versions;
        }
        Ok((reclaimed_vertices, version_entries))
    }

    /// Compact vertices deleted at or before `ts` across all shards.
    /// Watermark-gated vertex compaction across shards.
    ///
    /// The cutoff must be the watermark safe timestamp, never a bare
    /// transaction stamp.
    ///
    /// Returns the removed external keys and the old-to-new *global* internal
    /// ID mapping (shard-local rows translated into encoded global IDs), which
    /// callers must propagate to edge CSR rows before dependent queries.
    ///
    /// Shards whose fragmentation ratio is below
    /// [`SHARD_FRAGMENTATION_THRESHOLD`] are skipped (segment-level
    /// compaction): lazy ID recycling already reclaims their holes without a
    /// global remap, avoiding cross-shard coordination.
    pub fn compact_with_cutoff_collect_mapping(
        &self,
        cutoff: Timestamp,
    ) -> StorageResult<(Vec<IdKey>, std::collections::HashMap<u32, u32>)> {
        let mut all_removed = Vec::new();
        let mut all_mapping = std::collections::HashMap::new();
        for (idx, shard) in self.shards.iter().enumerate() {
            // Selective compaction: only compact shards with significant
            // fragmentation to avoid global remapping overhead.
            {
                let table = shard.read();
                let (live, allocated) = table.id_hole_stats(cutoff);
                if allocated > 0 {
                    let frag = if live >= allocated {
                        0.0
                    } else {
                        1.0 - (live as f64 / allocated as f64)
                    };
                    if frag < SHARD_FRAGMENTATION_THRESHOLD {
                        // Shard is sufficiently dense; lazy recycling will
                        // reclaim holes without compaction.
                        continue;
                    }
                }
            }
            let mut table = shard.write();
            let (removed, local_mapping) = table.compact_with_cutoff_collect_mapping(cutoff)?;
            for (old_local, new_local) in local_mapping {
                all_mapping.insert(
                    self.encode_id(idx, old_local),
                    self.encode_id(idx, new_local),
                );
            }
            all_removed.extend(removed);
        }
        Ok((all_removed, all_mapping))
    }

    pub fn version_history_ref(
        &self,
    ) -> std::sync::Arc<std::sync::Mutex<crate::schema::LabelVersionHistory>> {
        self.shards[0].read().version_history_ref()
    }

    pub fn version_chain_memory_bytes(&self) -> usize {
        self.shards
            .iter()
            .map(|s| s.read().columns.version_chain_stats().memory_bytes)
            .sum()
    }

    pub fn memory_size(&self) -> usize {
        let mut total = std::mem::size_of::<Self>();
        for shard in &self.shards {
            total += shard.read().memory_size();
        }
        total
    }

    pub fn used_memory_size(&self) -> usize {
        let mut total = std::mem::size_of::<Self>();
        for shard in &self.shards {
            total += shard.read().used_memory_size();
        }
        total
    }

    pub fn active_snapshot_count(&self) -> usize {
        0
    }
}
