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

    /// Fold version-chain before-images eligible at `cutoff` across shards.
    ///
    /// Fold-only (no ID remap): safe to run inside coordinated maintenance
    /// passes that already handle ID re-densification separately. The cutoff
    /// must be the watermark safe timestamp shared with the rest of the pass.
    /// Returns the total number of version entries folded.
    pub fn fold_version_chains(&self, cutoff: Timestamp) -> usize {
        self.shards
            .iter()
            .map(|shard| shard.write().fold_version_chains(cutoff))
            .sum()
    }

    /// Aggregate version-chain pressure across shards for observability.
    ///
    /// Returns `(total_entries, max_chain_len, memory_bytes)` after any
    /// pending fold, so maintenance logs can show whether the shared cutoff
    /// actually drained the chains or a pinned snapshot holds them.
    pub fn version_chain_pressure(&self) -> (usize, usize, usize) {
        let mut total_entries = 0usize;
        let mut max_len = 0usize;
        let mut memory_bytes = 0usize;
        for shard in &self.shards {
            let stats = shard.read().columns.version_chain_stats();
            total_entries += stats.total_entries;
            max_len = max_len.max(stats.max_len);
            memory_bytes += stats.memory_bytes;
        }
        (total_entries, max_len, memory_bytes)
    }

    /// Compact vertices deleted at or before `ts` across all shards.
    /// Watermark-gated vertex compaction across shards.
    ///
    /// The cutoff must be the watermark safe timestamp, never a bare
    /// transaction stamp.
    ///
    /// Returns the removed external keys, the old-to-new *global* internal
    /// ID mapping (shard-local rows translated into encoded global IDs),
    /// and the combined journal of the per-shard executions, which callers
    /// must propagate to edge CSR rows before dependent queries.
    ///
    /// Shards whose fragmentation ratio is below
    /// [`SHARD_FRAGMENTATION_THRESHOLD`] are skipped (segment-level
    /// compaction): lazy ID recycling already reclaims their holes without a
    /// global remap, avoiding cross-shard coordination. The threshold is the
    /// hole-rate watermark for stable row IDs: below it, free-stack reuse
    /// absorbs deletes and live rows never move; above it, a barriered
    /// compact reclaims the shard and the returned mapping must propagate
    /// to edge endpoints before any new write is admitted.
    ///
    /// Each compacted shard runs under its shard write lock, which is the
    /// per-shard commit barrier: concurrent writes to that shard block
    /// while reads continue on their snapshots.
    pub fn compact_with_cutoff_collect_mapping(
        &self,
        cutoff: Timestamp,
    ) -> StorageResult<(
        Vec<IdKey>,
        std::collections::HashMap<u32, u32>,
        super::super::compaction::CompactionJournal,
    )> {
        let mut all_removed = Vec::new();
        let mut all_mapping = std::collections::HashMap::new();
        let mut journals = Vec::new();
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
            let (removed, local_mapping, journal) =
                table.compact_with_cutoff_collect_mapping(cutoff)?;
            journals.push(journal);
            for (old_local, new_local) in local_mapping {
                all_mapping.insert(
                    self.encode_id(idx, old_local),
                    self.encode_id(idx, new_local),
                );
            }
            all_removed.extend(removed);
        }
        let combined = super::super::compaction::CompactionJournal::combine(&journals);
        Ok((all_removed, all_mapping, combined))
    }

    /// Evict cold column chunks across shards oldest-first until `max_bytes`
    /// are released. Returns `(chunks_evicted, bytes_released)`.
    ///
    /// Each shard is handled under its shard write lock with the
    /// evictability rechecked inside: a chunk that gained an overlay write
    /// or a version chain since selection is skipped for this pass.
    pub fn evict_cold_chunks(&self, max_bytes: u64) -> (usize, u64) {
        let mut count = 0usize;
        let mut freed = 0u64;
        for shard in &self.shards {
            if freed >= max_bytes {
                break;
            }
            let mut table = shard.write();
            let (n, bytes) = table
                .columns
                .evict_cold_chunks(max_bytes.saturating_sub(freed));
            count += n;
            freed += bytes;
        }
        (count, freed)
    }

    /// Eviction observability: `(resident_chunks, evicted_chunks,
    /// evicted_bytes, resident_bytes)` across shards.
    pub fn eviction_stats(&self) -> (usize, usize, usize, usize) {
        let mut resident_chunks = 0usize;
        let mut evicted_chunks = 0usize;
        let mut evicted_bytes = 0usize;
        let mut resident_bytes = 0usize;
        for shard in &self.shards {
            let table = shard.read();
            resident_chunks += table.columns.resident_chunk_count();
            evicted_chunks += table.columns.evicted_chunk_count();
            evicted_bytes += table.columns.evicted_bytes();
            resident_bytes += table.columns.resident_memory_usage();
        }
        (
            resident_chunks,
            evicted_chunks,
            evicted_bytes,
            resident_bytes,
        )
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

    #[allow(dead_code)]
    pub fn active_snapshot_count(&self) -> usize {
        // Snapshots are global to VersionManager; tables never pin.
        // Always zero by design. Use the GC coordinator diagnostics
        // active count for the pass-wide value.
        0
    }
}
