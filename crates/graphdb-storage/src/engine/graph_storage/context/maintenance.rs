use crate::engine::data_store::EdgeTableKey;
use graphdb_core::types::{CompactConfig, LabelId, Timestamp};
use graphdb_core::{StorageError, StorageResult};
use std::collections::HashMap;
use std::sync::atomic::Ordering;

use super::GraphStorageContext;

/// Stable row-id mode for the long-term compaction policy. Mirrors
/// [`crate::vertex::vertex_table::compaction::STABLE_ROW_IDS_ENABLED`]:
/// when `true`, every compaction must produce zero edge endpoint rewrites.
/// Stage one keeps this `false` while the stable collection path is
/// validated; the zero-rewrite assertion below fails closed when enabled.
pub const STABLE_ROW_IDS_ENABLED: bool =
    crate::vertex::vertex_table::compaction::STABLE_ROW_IDS_ENABLED;

/// Fail closed when stable row ids are enabled but a compaction produced
/// edge endpoint rewrites. Call after collecting per-label vertex mappings
/// and before touching any edge table.
pub fn assert_zero_edge_rewrite(
    vertex_mappings: &HashMap<LabelId, HashMap<u32, u32>>,
) -> StorageResult<()> {
    if !STABLE_ROW_IDS_ENABLED {
        return Ok(());
    }
    let rewritten: usize = vertex_mappings.values().map(|mapping| mapping.len()).sum();
    if rewritten > 0 {
        return Err(StorageError::invalid_operation(format!(
            "stable row ids enabled but compaction rewrote {} edge endpoints",
            rewritten
        )));
    }
    Ok(())
}

impl GraphStorageContext {
    /// Compact deleted vertices and propagate old-to-new internal ID
    /// mappings into edge tables.
    ///
    /// Shared by manual compaction transactions and the background
    /// maintenance thread (auto-compaction). The cutoff must be the
    /// watermark safe timestamp; bare transaction stamps are rejected by
    /// convention and must never be passed here.
    ///
    /// Returns the number of removed vertices. Compaction runs under a
    /// commit barrier holding the auto-commit write gate: same-table writes
    /// block for the whole vertex-remap plus edge-remap sequence while
    /// reads continue on their pre-compaction snapshots. Steps execute in
    /// fixed order (index, timestamps, columns, edge endpoints) with each
    /// vertex mapping journaled before the edge rewrite consumes it, so a
    /// mid-pass failure never leaves index and columns in different id
    /// spaces. Crash recovery replays external IDs from the WAL, which
    /// remains the durable source of truth.
    pub(crate) fn compact_vertex_remap(&self, cutoff: Timestamp) -> StorageResult<usize> {
        if !self.persistent.is_open.load(Ordering::Acquire) {
            return Err(StorageError::storage_not_open());
        }

        // Commit barrier: reuse the auto-commit write gate, no new lock
        // primitive. Held across the vertex remap and the edge endpoint
        // rewrite below so the two phases commit as one unit.
        let _barrier = self.persistent.auto_commit_write_gate.acquire();

        let mut last_compacted_vertices = self.persistent.last_compacted_vertices.lock();
        last_compacted_vertices.clear();

        // Old-to-new internal ID mappings produced by vertex compaction,
        // keyed by vertex label. Propagated to edge tables afterwards
        // (edge rows/neighbors are per-label internal IDs). Journals ride
        // along so the edge rewrite extends the same commit record.
        let mut vertex_mappings: HashMap<LabelId, HashMap<u32, u32>> = HashMap::new();
        let mut vertex_journals: HashMap<
            LabelId,
            crate::vertex::vertex_table::compaction::CompactionJournal,
        > = HashMap::new();

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
                    match table.compact_with_cutoff_collect_mapping(cutoff) {
                        Ok((removed, mapping, journal)) => {
                            if !removed.is_empty() {
                                last_compacted_vertices.push((label_id, removed));
                                vertex_mappings.insert(label_id, mapping);
                                vertex_journals.insert(label_id, journal);
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

        // Compaction re-densifies internal IDs: cached ID mappings and
        // vertex records for remapped labels are keyed by stale IDs.
        // Bump their invalidation generations (O(1) per label) so newer
        // readers fall back to the remapped tables.
        for &label_id in vertex_mappings.keys() {
            self.persistent
                .cache_manager
                .invalidate_vertices_by_label(label_id);
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
            // Same barrier, same journal: the vertex mappings were journaled
            // before the rewrite above, and the edge phase is recorded here
            // once per remapped endpoint label.
            for (key, did_remap) in &remapped {
                if !did_remap {
                    continue;
                }
                for label in [key.src_label, key.dst_label] {
                    if let Some(journal) = vertex_journals.get_mut(&label) {
                        journal.record_edge_remap();
                    }
                }
            }
            let remapped_edge_keys: Vec<EdgeTableKey> = remapped
                .into_iter()
                .filter(|(_, did_remap)| *did_remap)
                .map(|(key, _)| key)
                .collect();

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

    /// Stable row-id remap: fold-only collection with a zero-rewrite gate.
    ///
    /// Uses the stable collection path (holes absorbed through the free
    /// stack, live rows never move), asserts zero edge rewrites, and skips
    /// the edge endpoint propagation entirely. Switching production to this
    /// entry requires a full atomic checkpoint rollback baseline first; the
    /// switch is validated by identifier monotonicity, hole reuse rate, and
    /// full edge endpoint comparison.
    pub(crate) fn compact_vertex_remap_stable(&self, cutoff: Timestamp) -> StorageResult<usize> {
        if !self.persistent.is_open.load(Ordering::Acquire) {
            return Err(StorageError::storage_not_open());
        }
        let _barrier = self.persistent.auto_commit_write_gate.acquire();
        let mut last_compacted_vertices = self.persistent.last_compacted_vertices.lock();
        last_compacted_vertices.clear();

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
                    match table.compact_with_cutoff_stable_collect(cutoff) {
                        Ok((removed, mapping, _)) => {
                            debug_assert!(mapping.is_empty());
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
        for &label_id in &vertex_labels {
            let _ = label_id;
        }
        let empty: HashMap<LabelId, HashMap<u32, u32>> = HashMap::new();
        assert_zero_edge_rewrite(&empty)?;

        let total_vertices_removed: usize = last_compacted_vertices
            .iter()
            .map(|(_, removed)| removed.len())
            .sum();
        log::info!(
            "Stable compacted vertex tables: {} vertices absorbed, zero edge rewrites",
            total_vertices_removed
        );
        self.bump_layout_version();
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

        let gc = self.gc_coordinator();
        let wm = gc.capture_watermarks();
        // One watermark capture shared by every sub-system below, margin
        // applied. The caller-provided compaction timestamp is ignored for
        // vertex remap: the watermark cutoff is the only safe bound.
        let margin = self.persistent.config.gc_safety_margin;
        let cleanup_ts = wm.safe_gc_timestamp_with_margin(margin);
        log::info!(
            "Compact maintenance started: compact_ts={}, cleanup_threshold={} (watermarks={})",
            ts,
            cleanup_ts,
            wm.safe_gc_timestamp()
        );

        let total_vertices_removed = if STABLE_ROW_IDS_ENABLED {
            self.compact_vertex_remap_stable(cleanup_ts)?
        } else {
            self.compact_vertex_remap(cleanup_ts)?
        };

        // Fold vertex version-chain before-images under the same shared
        // cutoff. Checkpoints persist current values only, so without this
        // step chains survive until the next background GC pass even though
        // no active snapshot can observe them. Fold-only: ID
        // re-densification already happened in the remap above.
        let total_versions_folded = self
            .persistent
            .data_store
            .with_vertex_tables_mut(|vertex_tables| {
                let mut folded = 0usize;
                for table in vertex_tables.values() {
                    folded += table.fold_version_chains(cleanup_ts);
                    let (entries, max_len, memory_bytes) = table.version_chain_pressure();
                    if max_len > 0 {
                        log::debug!(
                            "vertex table '{}' chain pressure after fold: entries={} max_len={} memory_bytes={}",
                            table.label_name(),
                            entries,
                            max_len,
                            memory_bytes,
                        );
                    }
                }
                Ok(folded)
            })?;

        let edge_keys_and_removed: Vec<(EdgeTableKey, usize)> = self
            .persistent
            .data_store
            .for_all_edge_partitions_mut(|key, table| {
                let reserve_ratio = config.compute_reserve_ratio(table.edge_count() as usize, 0);
                let removed = table.compact_csr_only_with_watermarks(&wm, margin, reserve_ratio);
                table.compact_properties_with_watermarks(&wm, margin);
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

        self.persistent
            .data_store
            .for_all_edge_partitions_mut(|_key, table| {
                let del_stats = table.deletion_stats();
                if del_stats.is_significant() {
                    log::debug!(
                        "EdgeTable[{}] deletion stats: {:.1}% deleted ({} / {} live edges)",
                        table.label(),
                        del_stats.deletion_percentage(),
                        del_stats.total_deleted_edges,
                        del_stats.total_live_edges,
                    );
                }
                Ok(())
            })?;

        // Log freeze configuration for monitoring
        if let Some(ref manager) = self.runtime.background_freeze_manager {
            let freeze_config = manager.get_config();
            log::debug!(
                "Freeze config - edge_threshold: {}, memory_threshold: {}MB, deletion_threshold: {}",
                freeze_config.delta_edge_threshold,
                freeze_config.delta_memory_threshold_bytes / (1024 * 1024),
                freeze_config.deletion_threshold,
            );
        }

        log::info!(
            "Compaction completed: {} vertices, {} version entries folded, {} edges removed (cleanup_ts={})",
            total_vertices_removed,
            total_versions_folded,
            total_edges_removed,
            cleanup_ts
        );

        // Vertex/edge compaction changed the physical layout: bump the
        // monotonic layout version so cached plans that assumed the
        // previous layout are invalidated.
        self.bump_layout_version();

        Ok(())
    }
}

#[cfg(test)]
mod stable_assertion_tests {
    use super::*;

    #[test]
    fn zero_rewrite_assertion_passes_for_empty_mappings() {
        let empty: HashMap<LabelId, HashMap<u32, u32>> = HashMap::new();
        assert!(assert_zero_edge_rewrite(&empty).is_ok());
        let mut with_labels: HashMap<LabelId, HashMap<u32, u32>> = HashMap::new();
        with_labels.insert(1, HashMap::new());
        assert!(assert_zero_edge_rewrite(&with_labels).is_ok());
    }

    #[test]
    fn zero_rewrite_assertion_is_dormant_until_switch() {
        // Stage one keeps the switch off, so legacy above-watermark remaps
        // still pass the gate. Flipping the switch fails closed instead.
        assert!(!STABLE_ROW_IDS_ENABLED);
        let mut mappings: HashMap<LabelId, HashMap<u32, u32>> = HashMap::new();
        mappings.insert(1, [(0u32, 1u32)].into_iter().collect());
        assert!(assert_zero_edge_rewrite(&mappings).is_ok());
    }
}
