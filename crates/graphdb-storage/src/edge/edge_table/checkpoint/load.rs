//! Incremental load orchestration with torn-commit recovery.

use super::super::core::EdgeStore;
use super::layout::{
    manifest_path, LEGACY_IN_CSR_FILE, LEGACY_OUT_CSR_FILE, LEGACY_PROPERTIES_FILE,
};
use crate::edge::node_group::TableShardManifest;
use graphdb_core::{StorageError, StorageResult};
use std::path::Path;

impl EdgeStore {
    /// Load an incremental checkpoint. Directories in the old single-file
    /// layout, version 1 metadata without a commit tail, version 2 metadata
    /// with global timestamps, the legacy global `properties.bin` and
    /// pre-version-5 manifests are rejected explicitly, never converted.
    /// Damage detection only: version, section and trailing-byte mismatches
    /// fail the load. Crash consistency comes from
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
            if dir.join(LEGACY_OUT_CSR_FILE).exists() || dir.join(LEGACY_IN_CSR_FILE).exists() {
                return Err(StorageError::deserialize_error(
                    "legacy single-file edge layout without a group manifest is not supported"
                        .to_string(),
                ));
            }
            return Err(StorageError::io_error(format!(
                "missing group manifest: {}",
                manifest_file.display()
            )));
        }
        if dir.join(LEGACY_PROPERTIES_FILE).exists() {
            return Err(StorageError::deserialize_error(
                "legacy global properties.bin without per-group shards is not supported"
                    .to_string(),
            ));
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
        self.rebuild_owner_map();

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
        if orphan_mappings + orphan_csr_rows + live_orphans > 0 {
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
