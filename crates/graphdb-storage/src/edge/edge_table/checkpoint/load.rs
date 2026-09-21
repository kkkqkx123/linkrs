//! Incremental load orchestration with torn-commit recovery.

use super::super::core::EdgeStore;
use super::layout::manifest_path;
use crate::edge::node_group::TableShardManifest;
use graphdb_core::{StorageError, StorageResult};
use std::path::Path;

impl EdgeStore {
    /// Load an incremental checkpoint. Damage detection only: section and
    /// trailing-byte mismatches fail the load. Crash consistency comes from
    /// the commit protocol (group bases, sidecars and per-group shards before
    /// metadata, manifest published last with its tail embedded in
    /// `meta.bin`); a torn manifest file falls back to the embedded tail,
    /// covered by the crash-injection tests below instead of by
    /// the orphan audit. The orphan audit stays as file-damage detection: a
    /// nonzero mismatch means torn or corrupt files, not a recoverable crash
    /// window.
    pub(crate) fn load_incremental(&mut self, dir: &Path) -> StorageResult<()> {
        let manifest_file = manifest_path(dir);
        if !manifest_file.exists() {
            return Err(StorageError::io_error(format!(
                "missing group manifest: {}",
                manifest_file.display()
            )));
        }
        let manifest_bytes = std::fs::read(&manifest_file)
            .map_err(|e| StorageError::io_error(format!("Failed to read group manifest: {}", e)))?;
        let file_manifest = TableShardManifest::decode(&manifest_bytes)?;
        if file_manifest.group_bits != self.config.node_group_bits {
            return Err(StorageError::deserialize_error(format!(
                "group layout mismatch: file uses {} address bits, table uses {}",
                file_manifest.group_bits, self.config.node_group_bits
            )));
        }

        let embedded = self.load_metadata_file(dir, &file_manifest)?;
        let manifest = if embedded != file_manifest {
            if embedded.group_bits != self.config.node_group_bits {
                return Err(StorageError::deserialize_error(format!(
                    "group layout mismatch: file uses {} address bits, table uses {}",
                    embedded.group_bits, self.config.node_group_bits
                )));
            }
            log::debug!(
                "EdgeTable[{}] torn manifest file falls back to meta tail: file={:?} tail={:?}",
                self.label,
                file_manifest,
                embedded,
            );
            embedded
        } else {
            file_manifest
        };
        // Load replaces the table contents: drain topology rows first so the
        // non-empty guard on group reset only rejects mistaken resets, never
        // an intentional reload. Authority and property rows are rebuilt from
        // the shards below.
        for gid in self.out_csr.existing_group_ids() {
            if let Some(variant) = self.out_csr.group_variant_mut(gid) {
                variant.clear();
            }
        }
        for gid in self.in_csr.existing_group_ids() {
            if let Some(variant) = self.in_csr.group_variant_mut(gid) {
                variant.clear();
            }
        }
        self.out_csr.set_groups(&manifest.out_groups)?;
        self.in_csr.set_groups(&manifest.in_groups)?;
        self.load_group_set(dir, true, &manifest.out_groups, &manifest)?;
        self.load_group_set(dir, false, &manifest.in_groups, &manifest)?;
        self.load_timestamp_shards(dir, &manifest)?;
        self.load_property_shards(dir, &manifest)?;
        self.load_segment_stats(dir)?;
        let owner_stats = self.rebuild_owner_map_with_stats();
        if let Some(stats) = &self.stats_manager {
            stats.add_value_with_amount(
                graphdb_metrics::MetricType::EdgeRelocatedOrphans,
                owner_stats.relocated_orphans as u64,
            );
        }
        if owner_stats.relocated_orphans > 0 {
            log::info!(
                "EdgeTable[{}] rebuilt owner map: mapped={}, relocated_orphans={} to group {}",
                self.label_name,
                owner_stats.mapped,
                owner_stats.relocated_orphans,
                owner_stats.fallback_group,
            );
        } else {
            log::debug!(
                "EdgeTable[{}] rebuilt owner map: mapped={}",
                self.label_name,
                owner_stats.mapped,
            );
        }

        if self.next_edge_id.0 == 0 {
            let max_id = self
                .out_csr
                .iter_all()
                .map(|(_, nbr)| nbr.edge_id.0 + 1)
                .max()
                .unwrap_or(0);
            self.next_edge_id = graphdb_core::types::EdgeId(max_id);
        }
        // Damage detection for true corruption: torn manifest files already
        // recovered above via the embedded tail. A nonzero count here means
        // corrupt or regressed files and the load stays fail-closed.
        let (orphan_mappings, orphan_csr_rows, live_orphans) = self.copy_audit();
        if let Some(stats) = &self.stats_manager {
            stats.add_value_with_amount(
                graphdb_metrics::MetricType::EdgeOrphanMappings,
                orphan_mappings as u64,
            );
            stats.add_value_with_amount(
                graphdb_metrics::MetricType::EdgeOrphanRows,
                orphan_csr_rows as u64,
            );
            stats.add_value_with_amount(
                graphdb_metrics::MetricType::EdgeLiveAuthorityOrphans,
                live_orphans as u64,
            );
        }
        if orphan_mappings + orphan_csr_rows + live_orphans > 0 {
            // Storage-side refusal: corrupt or regressed checkpoint files fail
            // the open. This is never an import discard — import drops count
            // accepted/dropped rows per file, while this path refuses the
            // whole load with a storage error.
            log::warn!(
                "EdgeTable[{}] refuses checkpoint load with copy mismatches: \
                 orphan property mappings={}, orphan CSR rows={}, \
                 live authority orphans={}",
                self.label_name,
                orphan_mappings,
                orphan_csr_rows,
                live_orphans,
            );
            return Err(crate::StorageError::db_error(format!(
                "edge table {} loaded with copy mismatches: \
                 orphan property mappings={}, orphan CSR rows={}, \
                 live authority orphans={}",
                self.label_name, orphan_mappings, orphan_csr_rows, live_orphans,
            )));
        }
        self.properties_dirty = false;
        self.out_csr.clear_all_dirty();
        self.in_csr.clear_all_dirty();
        // Reloaded state is a fresh checkpoint base: no switch is pending.
        self.migration_pending_checkpoint = false;
        // Pending staged schema changes are memory-only: a reload after any
        // crash is equivalent to aborting them, because the property store is
        // rebuilt from the published schema below.
        self.pending_add_column = None;
        self.pending_drop_column = None;
        self.pending_rename_column = None;
        self.wal_dir = None;
        let wal_ops = super::super::wal::read_ops(dir)?;
        for op in wal_ops {
            self.replay_one_wal_op(op)?;
        }
        let (orphan_mappings, orphan_csr_rows, live_orphans) = self.copy_audit();
        if orphan_mappings + orphan_csr_rows + live_orphans > 0 {
            // Storage-side refusal after WAL replay, same division as the
            // load audit above: replay damage refuses the open, never counts
            // as an import discard.
            log::warn!(
                "EdgeTable[{}] refuses WAL replay with copy mismatches: \
                 orphan property mappings={}, orphan CSR rows={}, \
                 live authority orphans={}",
                self.label_name,
                orphan_mappings,
                orphan_csr_rows,
                live_orphans,
            );
            return Err(crate::StorageError::db_error(format!(
                "edge table {} replayed WAL with copy mismatches: \
                 orphan property mappings={}, orphan CSR rows={}, \
                 live authority orphans={}",
                self.label_name, orphan_mappings, orphan_csr_rows, live_orphans,
            )));
        }
        self.wal_dir = Some(dir.to_path_buf());
        self.is_open = true;
        Ok(())
    }
}
