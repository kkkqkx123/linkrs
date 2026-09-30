//! Version-chain garbage collection across shards.

use super::super::ShardedVertexTable;
use graphdb_core::types::Timestamp;
use graphdb_core::StorageResult;

impl ShardedVertexTable {
    /// GC split into (reclaimed vertices, reclaimed version-chain entries).
    /// Stable row-id semantic: reclaimed vertices only lose their keys to
    /// the free stack, live rows never move, so no cache invalidation is
    /// required for this label. The cutoff must be the global watermark
    /// safe timestamp.
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

    pub fn version_chain_memory_bytes(&self) -> usize {
        self.shards
            .iter()
            .map(|s| s.read().columns.version_chain_stats().memory_bytes)
            .sum()
    }
}
