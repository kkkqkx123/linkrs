use std::sync::atomic::Ordering;
use crate::core::{StorageError, StorageResult};
use crate::core::types::{CompactConfig, LabelId, Timestamp};
use crate::storage::engine::data_store::EdgeTableKey;

use super::GraphStorageContext;

impl GraphStorageContext {
    pub(crate) fn compact_maintenance(
        &self,
        config: &CompactConfig,
        ts: Timestamp,
    ) -> StorageResult<()> {
        if !self.persistent.is_open.load(Ordering::Acquire) {
            return Err(StorageError::storage_not_open());
        }

        let cleanup_ts = self.persistent.version_manager.snapshot_tracker().cleanup_threshold();
        log::info!(
            "Compact maintenance started: compact_ts={}, cleanup_threshold={}",
            ts,
            cleanup_ts
        );

        let mut last_compacted_vertices = self.persistent.last_compacted_vertices.lock();
        last_compacted_vertices.clear();

        let vertex_labels: Vec<LabelId>;
        {
            let mut vertex_tables = self.persistent.data_store.vertex_tables().write();
            vertex_labels = vertex_tables.keys().copied().collect();

            for &label_id in &vertex_labels {
                let table = vertex_tables.get_mut(&label_id).expect("label must exist");
                match table.compact_with_ts_collect(ts) {
                    Ok(removed) => {
                        if !removed.is_empty() {
                            last_compacted_vertices.push((label_id, removed));
                        }
                    }
                    Err(e) => {
                        log::error!("Failed to compact vertex table {}: {}", label_id, e);
                    }
                }
            }
        }

        for &label_id in &vertex_labels {
            self.mark_vertex_modified(label_id);
        }

        let total_vertices_removed: usize = last_compacted_vertices
            .iter()
            .map(|(_, removed)| removed.len())
            .sum();

        log::info!(
            "Compacted vertex tables: {} vertices removed",
            total_vertices_removed
        );

        let edge_keys: Vec<EdgeTableKey>;
        let mut total_edges_removed = 0usize;
        {
            let mut edge_tables = self.persistent.data_store.edge_tables().write();
            edge_keys = edge_tables.keys().copied().collect();

            if config.enable_structure_compaction {
                for &key in &edge_keys {
                    let table = edge_tables.get_mut(&key).expect("edge key must exist");
                    let removed = table.compact_and_freeze(ts, config, crate::storage::edge::CompactionMode::Standard);
                    total_edges_removed += removed;
                }

                log::info!(
                    "Compacted CSR structures: {} edges removed",
                    total_edges_removed
                );
            } else {
                for &key in &edge_keys {
                    let table = edge_tables.get_mut(&key).expect("edge key must exist");
                    table.freeze_csr_only(ts);
                    table.compact_properties(ts);
                }
            }
        }

        for &key in &edge_keys {
            self.mark_edge_modified(key.edge_label);
        }

        match self.gc_index_tombstones(cleanup_ts) {
            Ok(index_gc_stats) if index_gc_stats.total_removed() > 0 => {
                log::info!(
                    "Index GC during compaction: removed {} vertex entries (cleanup_ts={})",
                    index_gc_stats.vertex_entries_removed,
                    cleanup_ts
                );
            }
            Ok(_) => {
                log::debug!("No index tombstones to clean (cleanup_ts={})", cleanup_ts);
            }
            Err(err) => {
                log::warn!("Index GC during compaction failed: {}", err);
            }
        }

        self.persistent.cache_manager.clear_cache();

        match self.trigger_background_freeze() {
            Ok(()) => {
                if let Some(stats) = self.get_freeze_stats() {
                    log::info!(
                        "Background freeze during compaction: {} total freezes, {} edges frozen",
                        stats.freeze_count,
                        stats.total_frozen_edges
                    );
                }
            }
            Err(err) => {
                log::warn!("Background freeze during compaction failed: {}", err);
            }
        }

        if self.persistent.config.merge_config.enable_adaptive_merge {
            let mut edge_tables = self.persistent.data_store.edge_tables().write();
            let mut total_merged = 0usize;

            for table in edge_tables.values_mut() {
                let merged = table.merge_segments_adaptive(
                    ts,
                    self.persistent.config.merge_config.max_segment_age,
                );
                total_merged += merged;
            }

            if total_merged > 0 {
                log::info!(
                    "Adaptive merge during compaction: {} segments merged",
                    total_merged
                );
            }
        }

        log::info!(
            "Compaction completed: {} vertices, {} edges removed (cleanup_ts={})",
            total_vertices_removed,
            total_edges_removed,
            cleanup_ts
        );

        Ok(())
    }
}
