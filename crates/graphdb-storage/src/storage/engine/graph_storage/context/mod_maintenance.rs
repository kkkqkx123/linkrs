use crate::core::types::{CompactConfig, LabelId, Timestamp};
use crate::core::{StorageError, StorageResult};
use crate::storage::engine::data_store::EdgeTableKey;
use std::sync::atomic::Ordering;

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

        let cleanup_ts = self
            .persistent
            .version_manager
            .snapshot_tracker()
            .cleanup_threshold();
        log::info!(
            "Compact maintenance started: compact_ts={}, cleanup_threshold={}",
            ts,
            cleanup_ts
        );

        let mut last_compacted_vertices = self.persistent.last_compacted_vertices.lock();
        last_compacted_vertices.clear();

        let vertex_labels = self
            .persistent
            .data_store
            .with_vertex_tables_mut(|vertex_tables| {
                let labels: Vec<LabelId> = vertex_tables.keys().copied().collect();
                for &label_id in &labels {
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
                Ok(labels)
            })?;

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

        let mut total_edges_removed = 0usize;
        let edge_keys = self
            .persistent
            .data_store
            .with_edge_tables_mut(|edge_tables| {
                let keys: Vec<EdgeTableKey> = edge_tables.keys().copied().collect();
                if config.enable_structure_compaction {
                    for &key in &keys {
                        let arc = edge_tables.get_mut(&key).expect("edge key must exist");
                        let mut table = arc.write();
                        let removed = table.compact_and_freeze(
                            ts,
                            config,
                            crate::storage::edge::CompactionMode::Standard,
                        );
                        total_edges_removed += removed;
                    }

                    log::info!(
                        "Compacted CSR structures: {} edges removed",
                        total_edges_removed
                    );
                } else {
                    for &key in &keys {
                        let arc = edge_tables.get_mut(&key).expect("edge key must exist");
                        let mut table = arc.write();
                        table.freeze_csr_only(ts);
                        table.compact_properties(ts);
                    }
                }
                Ok(keys)
            })?;

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

        if let Ok(freed) = self.trigger_segment_eviction() {
            if freed > 0 {
                log::info!("Segment eviction during maintenance freed {} bytes", freed);
            }
        }

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

        self.persistent.data_store.with_edge_tables_mut(|edge_tables| {
            let mut adaptive_merged = 0usize;
            let mut lsm_merged = 0usize;

            for arc in edge_tables.values_mut() {
                let mut table = arc.write();
                if self.persistent.config.merge_config.enable_adaptive_merge {
                    adaptive_merged += table.merge_segments_adaptive(
                        ts,
                        self.persistent.config.merge_config.max_segment_age,
                        self.persistent.config.merge_config.deletion_threshold,
                        self.persistent.config.merge_config.max_segment_size_bytes,
                    );
                }
                if self.persistent.config.merge_config.enable_lsm_tiering {
                    lsm_merged += table.merge_segments_lsm_tiered(ts);
                }

                let stats = table.merge_stats();
                log::debug!(
                    "Merge stats - segments: {}/{}, total_ops: {}, avg_segs_per_op: {:.1}, avg_edges_per_op: {:.0}, avg_time_ms: {:.2}, pressure: {}",
                    stats.current_segment_count,
                    stats.max_segment_count,
                    stats.total_merge_operations,
                    stats.avg_segments_per_merge(),
                    stats.avg_edges_per_merge(),
                    stats.avg_merge_time_ms(),
                    stats.segment_count_pressure()
                );

                let del_stats = table.deletion_stats();
                if del_stats.is_significant() {
                    log::debug!(
                        "EdgeTable[{}] deletion stats: {:.1}% deleted ({} / {} frozen edges)",
                        table.label(),
                        del_stats.deletion_percentage(),
                        del_stats.total_deleted_edges,
                        del_stats.total_frozen_edges,
                    );
                }

                // Debug: Validate segment integrity
                if log::log_enabled!(log::Level::Debug) {
                    let valid_count = table.validate_segment_integrity();
                    let total_segments = table.segment_versions().len();
                    if valid_count != total_segments {
                        log::warn!(
                            "Segment integrity check: {}/{} segments valid",
                            valid_count,
                            total_segments
                        );
                    }
                }
            }

            if adaptive_merged > 0 {
                log::info!(
                    "Adaptive merge during compaction: {} segments merged",
                    adaptive_merged
                );
            }
            if lsm_merged > 0 {
                log::info!(
                    "LSM tiered merge during compaction: {} segments merged",
                    lsm_merged
                );
            }
            Ok(())
        })?;

        // Log freeze configuration for monitoring
        if let Some(ref manager) = self.runtime.background_freeze_manager {
            let freeze_config = manager.get_config();
            log::debug!(
                "Freeze config - strategy: {:?}, edge_threshold: {}, memory_threshold: {}MB",
                freeze_config.strategy,
                freeze_config.delta_edge_threshold,
                freeze_config.delta_memory_threshold_bytes / (1024 * 1024)
            );
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
