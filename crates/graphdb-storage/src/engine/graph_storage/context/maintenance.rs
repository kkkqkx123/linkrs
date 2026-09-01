use crate::engine::data_store::EdgeTableKey;
use graphdb_core::types::{CompactConfig, LabelId, Timestamp};
use graphdb_core::{StorageError, StorageResult};
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use super::GraphStorageContext;

impl GraphStorageContext {
    /// Compact deleted vertices and propagate old-to-new internal ID
    /// mappings into edge tables and cold snapshots.
    ///
    /// Shared by manual compaction transactions and the background
    /// maintenance thread (auto-compaction). Deletes at or before `ts` are
    /// reclaimed; callers must pass a safe timestamp (e.g. the snapshot
    /// tracker cleanup threshold) to preserve time-travel visibility.
    ///
    /// Returns the number of removed vertices. Compaction is an in-memory
    /// re-layout: it writes no WAL entries, and crash recovery replays
    /// external IDs from the WAL, so no persistence ordering constraint
    /// applies here.
    pub(crate) fn compact_vertex_remap(&self, ts: Timestamp) -> StorageResult<usize> {
        if !self.persistent.is_open.load(Ordering::Acquire) {
            return Err(StorageError::storage_not_open());
        }

        let mut last_compacted_vertices = self.persistent.last_compacted_vertices.lock();
        last_compacted_vertices.clear();

        // Old-to-new internal ID mappings produced by vertex compaction,
        // keyed by vertex label. Propagated to edge tables and cold
        // snapshots afterwards (edge rows/neighbors are per-label internal
        // IDs).
        let mut vertex_mappings: HashMap<LabelId, HashMap<u32, u32>> = HashMap::new();

        let vertex_labels = self
            .persistent
            .data_store
            .with_vertex_tables_mut(|vertex_tables| {
                let labels: Vec<LabelId> = vertex_tables.keys().copied().collect();
                for &label_id in &labels {
                    let table = vertex_tables.get(&label_id).ok_or_else(|| {
                        StorageError::label_not_found(format!(
                            "label {label_id} not found during compaction"
                        ))
                    })?;
                    match table.compact_with_ts_collect_mapping(ts) {
                        Ok((removed, mapping)) => {
                            if !removed.is_empty() {
                                last_compacted_vertices.push((label_id, removed));
                                vertex_mappings.insert(label_id, mapping);
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

        // Propagate compaction ID remaps into every edge table referencing a
        // compacted vertex label before CSR structures are rebuilt below.
        if !vertex_mappings.is_empty() {
            // Edge types whose endpoint tags are unspecified resolve against
            // any vertex table (wildcard label 0); merge all compacted
            // mappings for them. Overlapping old IDs across labels are
            // resolved arbitrarily — mirroring the wildcard lookup's own
            // ambiguity.
            let wildcard_mapping: HashMap<u32, u32> = vertex_mappings
                .values()
                .flat_map(|m| m.iter().map(|(&k, &v)| (k, v)))
                .collect();
            let mapping_for = |label: LabelId| -> Option<&HashMap<u32, u32>> {
                if let Some(m) = vertex_mappings.get(&label) {
                    return Some(m);
                }
                if label == 0 && !wildcard_mapping.is_empty() {
                    return Some(&wildcard_mapping);
                }
                None
            };

            let remapped: Vec<(EdgeTableKey, bool)> = self
                .persistent
                .data_store
                .for_all_edge_partitions_mut(|key, table| {
                    let src_mapping = mapping_for(key.src_label);
                    let dst_mapping = mapping_for(key.dst_label);
                    if src_mapping.is_none() && dst_mapping.is_none() {
                        return Ok((key, false));
                    }
                    table.remap_vertex_ids(src_mapping, dst_mapping)?;
                    Ok((key, true))
                })?;
            let remapped_edge_keys: Vec<EdgeTableKey> = remapped
                .into_iter()
                .filter(|(_, did_remap)| *did_remap)
                .map(|(key, _)| key)
                .collect();

            // Cold snapshots hold CSR rows/neighbors in the same internal ID
            // spaces; remap in memory and rewrite the backing .lkcs file so
            // queries stay consistent across reloads. The file is a
            // rebuildable cache, so a persist failure only degrades to a
            // stale file (logged, not fatal).
            let mut cold_snapshots = self.cold_snapshots().write();
            for snapshots in cold_snapshots.values_mut() {
                for snapshot in snapshots.iter_mut() {
                    let schema = snapshot.schema();
                    let src_label = schema.src_label;
                    let dst_label = schema.dst_label;
                    let src_mapping = mapping_for(src_label);
                    let dst_mapping = mapping_for(dst_label);
                    if src_mapping.is_none() && dst_mapping.is_none() {
                        continue;
                    }
                    Arc::make_mut(snapshot).remap_vertex_ids(src_mapping, dst_mapping)?;
                    if let Err(e) = snapshot.persist() {
                        log::warn!(
                            "Failed to persist cold snapshot (label={}) after remap: {}",
                            src_label,
                            e
                        );
                    }
                }
            }
            drop(cold_snapshots);

            log::info!(
                "Propagated vertex compaction remap to {} edge table(s), {} compacted label(s)",
                remapped_edge_keys.len(),
                vertex_mappings.len()
            );
        }

        let total_vertices_removed: usize = last_compacted_vertices
            .iter()
            .map(|(_, removed)| removed.len())
            .sum();

        log::info!(
            "Compacted vertex tables: {} vertices removed",
            total_vertices_removed
        );

        Ok(total_vertices_removed)
    }

    pub(crate) fn compact_maintenance(
        &self,
        config: &CompactConfig,
        ts: Timestamp,
    ) -> StorageResult<()> {
        if !self.persistent.is_open.load(Ordering::Acquire) {
            return Err(StorageError::storage_not_open());
        }

        let gc = crate::engine::gc_coordinator::GcCoordinator::new(
            self.persistent.version_manager.clone(),
        );
        let wm = gc.capture_watermarks();
        let cleanup_ts = wm.safe_gc_timestamp();
        log::info!(
            "Compact maintenance started: compact_ts={}, cleanup_threshold={} (watermarks={})",
            ts,
            cleanup_ts,
            wm.safe_gc_timestamp()
        );

        let total_vertices_removed = self.compact_vertex_remap(ts)?;

        let edge_keys_and_removed: Vec<(EdgeTableKey, usize)> = self
            .persistent
            .data_store
            .for_all_edge_partitions_mut(|key, table| {
                let removed = if config.enable_structure_compaction {
                    table.compact_and_freeze(ts, config)
                } else {
                    table.freeze_csr_only(ts);
                    table.compact_properties(ts);
                    0
                };
                Ok((key, removed))
            })?;

        let total_edges_removed: usize = edge_keys_and_removed.iter().map(|(_, r)| r).sum();
        let edge_keys: Vec<EdgeTableKey> =
            edge_keys_and_removed.into_iter().map(|(k, _)| k).collect();

        if config.enable_structure_compaction {
            log::info!(
                "Compacted CSR structures: {} edges removed",
                total_edges_removed
            );
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

        let (adaptive_merged, lsm_merged) = self
            .persistent
            .data_store
            .for_all_edge_partitions_mut(|_key, table| {
                let mut adaptive_here = 0;
                let mut lsm_here = 0;

                if self.persistent.config.merge_config.enable_adaptive_merge {
                    adaptive_here += table.merge_segments_adaptive(
                        ts,
                        self.persistent.config.merge_config.max_segment_age,
                        self.persistent.config.merge_config.deletion_threshold,
                        self.persistent.config.merge_config.max_segment_size_bytes,
                    );
                }
                if self.persistent.config.merge_config.enable_lsm_tiering {
                    lsm_here += table.merge_segments_lsm_tiered(ts);
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
                Ok((adaptive_here, lsm_here))
            })?
            .into_iter()
            .fold((0, 0), |acc, res| (acc.0 + res.0, acc.1 + res.1));

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

        // Vertex/edge compaction, segment merges, and eviction all changed
        // the physical layout: bump the monotonic layout version so cached
        // plans that assumed the previous layout are invalidated.
        self.bump_layout_version();

        Ok(())
    }
}
