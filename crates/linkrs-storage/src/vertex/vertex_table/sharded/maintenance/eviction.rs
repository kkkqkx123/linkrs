//! Cold-chunk eviction and unified buffer accounting across shards.

use super::super::ShardedVertexTable;

impl ShardedVertexTable {
    /// Evict cold column chunks across shards oldest-first until `max_bytes`
    /// are released. Returns `(chunks_evicted, bytes_released)`.
    ///
    /// Each shard is handled under its shard read lock with the
    /// evictability rechecked inside the segment latch: eviction is
    /// per-chunk locked, so it runs concurrently with point reads and
    /// writes while a chunk that gained an overlay write or a version
    /// chain since selection is skipped for this pass.
    pub fn evict_cold_chunks(&self, max_bytes: u64) -> (usize, u64) {
        let mut count = 0usize;
        let mut freed = 0u64;
        for shard in &self.shards {
            if freed >= max_bytes {
                break;
            }
            let table = shard.read();
            let (n, bytes) = table
                .columns
                .evict_cold_chunks(max_bytes.saturating_sub(freed));
            count += n;
            freed += bytes;
        }
        (count, freed)
    }

    /// Quota-segmented eviction across shards for background tasks.
    /// `task_quota` caps one segment; over-quota work proceeds in segments
    /// instead of one burst. Returns `(chunks_evicted, bytes_released,
    /// segments)`.
    pub fn evict_cold_chunks_with_quota(
        &self,
        max_bytes: u64,
        task_quota: u64,
    ) -> (usize, u64, usize) {
        if task_quota >= max_bytes {
            let (count, freed) = self.evict_cold_chunks(max_bytes);
            return (count, freed, usize::from(count > 0));
        }
        let mut count = 0usize;
        let mut freed = 0u64;
        let mut segments = 0usize;
        for shard in &self.shards {
            if freed >= max_bytes {
                break;
            }
            let table = shard.read();
            let (n, bytes, segs) = table
                .columns
                .evict_cold_chunks_with_quota(max_bytes.saturating_sub(freed), task_quota);
            count += n;
            freed += bytes;
            segments += segs;
        }
        (count, freed, segments)
    }

    /// Eviction observability: `(resident_chunks, evicted_chunks,
    /// evicted_bytes, resident_bytes)` across shards, all from the unified
    /// buffer ledger (resident includes the overflow side store).
    pub fn eviction_stats(&self) -> (usize, usize, usize, usize) {
        let acc = self.buffer_ledger();
        (
            acc.resident_chunks,
            acc.evicted_chunks,
            acc.evicted_bytes,
            acc.resident_bytes,
        )
    }

    /// Unified buffer ledger across shards, including the overflow subset
    /// and dirty pages under one unified accounting for quota and observability.
    pub fn buffer_ledger(&self) -> crate::vertex::column::BufferLedger {
        let mut acc = crate::vertex::column::BufferLedger::default();
        for shard in &self.shards {
            let table = shard.read();
            let ledger = table.columns.buffer_ledger();
            acc.resident_bytes += ledger.resident_bytes;
            acc.evicted_bytes += ledger.evicted_bytes;
            acc.overflow_bytes += ledger.overflow_bytes;
            acc.dirty_pages += ledger.dirty_pages;
            acc.resident_chunks += ledger.resident_chunks;
            acc.evicted_chunks += ledger.evicted_chunks;
        }
        acc
    }
}
