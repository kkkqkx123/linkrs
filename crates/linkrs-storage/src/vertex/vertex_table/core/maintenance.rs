//! Version-chain GC, id-hole and memory statistics, dirty pages, raw lookups.

use std::sync::atomic::Ordering;

use super::VertexTable;
use crate::vertex::{IdKey, Timestamp};
use linkrs_core::StorageResult;

impl VertexTable {
    /// Consume this shard's compaction-moved-rows flag for table-level
    /// flush coordination.
    pub fn take_pk_baseline_invalidated(&self) -> bool {
        self.id_indexer.take_baseline_invalidated()
    }

    /// Naked identity read for GC and offline tools only: no liveness check.
    pub fn get_internal_id_by_i64_raw(&self, external_id: i64) -> Option<u32> {
        if !self.is_open.load(Ordering::Acquire) {
            return None;
        }
        self.id_indexer.get_index(&IdKey::Int(external_id))
    }

    /// Lookup internal ID from external string without timestamp check.
    /// Returns Some(internal_id) even for deleted vertices.
    /// Naked identity read for GC and offline tools only.
    pub fn get_internal_id_raw(&self, external_id: &str) -> Option<u32> {
        if !self.is_open.load(Ordering::Acquire) {
            return None;
        }
        self.id_indexer
            .get_index(&IdKey::Text(external_id.to_string()))
    }

    pub fn get_external_id(&self, internal_id: u32, ts: Timestamp) -> Option<IdKey> {
        if !self.is_open.load(Ordering::Acquire) || !self.is_row_live_at(internal_id, ts) {
            return None;
        }
        self.id_indexer.get_key(internal_id)
    }

    /// Lookup external ID from internal ID without timestamp check.
    /// Returns the external ID even for deleted vertices.
    /// Naked identity read for GC and offline tools only.
    pub fn get_external_id_raw(&self, internal_id: u32) -> Option<IdKey> {
        if !self.is_open.load(Ordering::Acquire) {
            return None;
        }
        self.id_indexer.get_key(internal_id)
    }

    /// Declared data type of the column `name`, if it exists in the schema.
    pub fn data_type_of(&self, name: &str) -> Option<linkrs_core::types::DataType> {
        self.columns.data_type_of(name)
    }

    /// Mark one column unavailable with a reason. Later strict reads and
    /// writes touching the column fail with the column name; healthy columns
    /// keep serving.
    pub fn mark_column_unavailable(&self, name: &str, reason: String) {
        self.columns.mark_column_unavailable(name, reason);
    }

    pub fn total_count(&self) -> usize {
        self.id_indexer.len()
    }

    /// Live vertex count at `ts` (excludes vertices deleted at or before
    /// `ts`) and total allocated local IDs (the high-water mark, including
    /// free-stack holes awaiting reuse). The gap `allocated - live` is the
    /// number of slots reclaimable by a compaction at `ts`; below the
    /// hole-rate watermark the free stack absorbs deletes without moving
    /// live rows.
    ///
    /// Live derives from the bound-key count minus timestamp tombstones so
    /// stable-GC invalidated slots (unbound, already on the free stack) are
    /// not miscounted as live.
    pub fn id_hole_stats(&self, ts: Timestamp) -> (usize, usize) {
        let allocated = self.next_local_id() as usize;
        let bound = self.id_indexer.len();
        let stamps = self.timestamps.read();
        let deleted = stamps.iter_deleted(ts).count();
        let pending = stamps.pending_len();
        (
            bound.saturating_sub(deleted).saturating_sub(pending),
            allocated,
        )
    }

    /// Mark all columns' page containing `row_idx` as dirty.
    pub fn mark_row_dirty(&self, row_idx: usize) {
        self.columns.mark_row_dirty(row_idx);
    }

    pub fn dirty_pages(&self) -> Vec<crate::persistence::dirty_page::PageId> {
        self.columns.collect_dirty_pages()
    }

    pub fn clear_dirty(&self) {
        self.columns.clear_dirty();
    }

    pub fn memory_size(&self) -> usize {
        let mut total = 0;

        total += self.id_indexer.memory_size();
        total += self.columns.memory_size();
        total += self.timestamps.read().memory_size();

        // Account for label_name string (content only)
        total += self.label_name.len();

        // Account for property_index_cache HashMap (actual entries, not capacity)
        total += self.property_index_cache.len()
            * (std::mem::size_of::<String>() + std::mem::size_of::<usize>());

        total += std::mem::size_of::<Self>();

        total
    }

    pub fn used_memory_size(&self) -> usize {
        let mut total = 0;

        let active_count = self.id_indexer.len();
        total += active_count * std::mem::size_of::<(String, u32)>();

        total += self.columns.used_memory_size();

        total += self.timestamps.read().size() * std::mem::size_of::<Timestamp>();

        // Account for actual label_name usage
        total += self.label_name.len();

        // Account for property_index_cache actual entries
        total += self.property_index_cache.len() * (24 + std::mem::size_of::<usize>()); // String overhead + usize

        total
    }

    // ==================== MVCC Methods ====================
    // Snapshot truth lives in the transaction layer watermarks. The caller
    // passes the watermark safe timestamp; timestamp compaction is always
    // cutoff-gated.

    /// Fold version-chain before-images eligible at `cutoff`.
    ///
    /// Fold-only entry for watermark-coordinated maintenance: drops
    /// before-images no active snapshot can observe without touching the ID
    /// space (no re-densification, no timestamp compaction). Returns the
    /// number of version entries folded. The cutoff must come from the
    /// shared watermark capture of the maintenance pass.
    pub fn fold_version_chains(&self, cutoff: Timestamp) -> usize {
        let removed = self.columns.gc_versions(cutoff);
        self.columns.maybe_rebuild_zones_exact();
        removed
    }

    /// Perform garbage collection on version data older than min_ts
    ///
    /// Reclaims deleted vertices (from the id indexer / timestamps) and drops
    /// property version-chain entries that no active snapshot can observe.
    ///
    /// Returns `(reclaimed vertices, reclaimed version-chain entries)`.
    /// Stable row-id semantic: reclaimed vertices only lose their keys to
    /// the free stack, live rows never move, so caches keyed by internal ID
    /// stay valid across this call. ID re-densification lives exclusively
    /// in the barriered offline remap path, never in background GC.
    pub fn gc_detailed(&self, min_ts: Timestamp) -> StorageResult<(usize, usize)> {
        // Property version-chain GC runs every pass regardless of deleted
        // vertices so before-images of overwritten properties are reclaimed.
        let version_removed = self.columns.gc_versions(min_ts);
        self.columns.maybe_rebuild_zones_exact();
        let version_stats = self.columns.version_chain_stats();
        log::trace!(
            "vertex gc version stats: total_rows={} total_entries={} max_len={} avg_len={:.2} memory_bytes={} removed={}",
            version_stats.total_rows,
            version_stats.total_entries,
            version_stats.max_len,
            version_stats.avg_len,
            version_stats.memory_bytes,
            version_removed
        );
        // Version chains are watermark-collected only: a long-lived snapshot
        // pins every chain, so an abnormally long chain almost always means
        // a stuck snapshot rather than a hot row. Surface it loudly instead
        // of growing silently.
        if version_stats.max_len > Self::VERSION_CHAIN_PRESSURE_WARN_LEN {
            log::warn!(
                "vertex table '{}' has a version chain of length {} (min_active_snapshot_ts={}); \
                 a pinned snapshot may be blocking garbage collection",
                self.label_name,
                version_stats.max_len,
                min_ts,
            );
        }

        // Collect all vertices deleted before min_ts
        let deleted_ids: Vec<u32> = self.timestamps.read().iter_deleted(min_ts).collect();

        if deleted_ids.is_empty() {
            return Ok((0, version_removed));
        }

        let mut count = 0usize;

        // Stable absorption: drop keys to the free stack and invalidate
        // their timestamp slots without moving any live row. Freed ids are
        // recycled by later inserts; columns keep their holes until reuse.
        let mut stamps = self.timestamps.write();
        for id in &deleted_ids {
            if let Some(key) = self.id_indexer.get_key(*id) {
                self.id_indexer.remove(&key);
                stamps.invalidate_slot(*id);
                count += 1;
            }
        }

        Ok((count, version_removed))
    }
}
