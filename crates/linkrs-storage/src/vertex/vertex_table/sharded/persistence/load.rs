//! Recovery reads: strict open plus incremental delta apply.

use std::path::Path;

use super::super::ShardedVertexTable;
use super::commit_manifest::{
    column_for_isolatable_file, CommitManifest, CorruptionClass, COMMIT_MANIFEST_FILE_NAME,
};
use super::common::now_ms;
use super::sidecar::prune_tampered_sidecars;
use linkrs_core::StorageResult;

/// Shard index plus owning column for a manifest-listed per-column file.
/// Returns `None` for table-critical files or malformed shard prefixes.
fn parse_shard_column(rel: &str) -> Option<(usize, String)> {
    let (shard_part, _) = rel.split_once('/')?;
    let idx: usize = shard_part.strip_prefix("shard_")?.parse().ok()?;
    let col = column_for_isolatable_file(rel)?;
    Some((idx, col))
}

impl ShardedVertexTable {
    /// Open a table from whatever layout and generation its manifest pins.
    ///
    /// Single layout-aware entry: the manifest's shard count, segment
    /// width and redistribution generation build the table, so callers
    /// never hand-construct a layout that disagrees with the checkpoint.
    /// Unknown router versions refuse with a rebuild directive instead of
    /// misrouting persisted rows.
    pub fn open_at<P: AsRef<Path>>(
        label: linkrs_core::types::LabelId,
        label_name: String,
        schema: crate::vertex::VertexSchema,
        path: P,
    ) -> StorageResult<Self> {
        let manifest = Self::read_table_manifest(path.as_ref())?.ok_or_else(|| {
            linkrs_core::StorageError::deserialize_error(format!(
                "missing table manifest at {}: rebuild the table before opening",
                path.as_ref().display()
            ))
        })?;
        if manifest.router_version != super::super::routing::ROUTER_VERSION {
            return Err(linkrs_core::StorageError::deserialize_error(format!(
                "table manifest router version {} differs from binary {}: \
                 rebuild the table with the offline redistribution tool instead of opening it in place",
                manifest.router_version,
                super::super::routing::ROUTER_VERSION,
            )));
        }
        let layout = super::super::routing::ShardLayout {
            num_shards: manifest.num_shards,
            segment_slots_bits: manifest.segment_slots_bits,
            total_segments: manifest.total_segments,
        };
        if !layout.is_consistent() {
            return Err(linkrs_core::StorageError::deserialize_error(format!(
                "table manifest pins an inconsistent shard layout at {}",
                path.as_ref().display()
            )));
        }
        let table = Self::with_layout(label, label_name, schema, layout, manifest.generation);
        table.load(path)?;
        Ok(table)
    }

    pub fn load<P: AsRef<Path>>(&self, path: P) -> StorageResult<()> {
        let path = path.as_ref();
        // Offline pre-flight: reuse the read-only inspection for a
        // diagnostic line before the strict recovery path runs. Read-only;
        // cleanup stays with startup recovery.
        match Self::inspect_commit_health(path) {
            Ok(report) => log::debug!(
                "vertex table '{}' pre-load health: healthy={} epoch={:?} missing={} orphans={}",
                self.label_name,
                report.is_healthy(),
                report.epoch,
                report.missing_files.len(),
                report.orphan_tmp_files.len(),
            ),
            Err(e) => log::debug!(
                "vertex table '{}' pre-load inspection failed: {}",
                self.label_name,
                e
            ),
        }
        // Refuse to mis-decode: persisted global IDs embed the shard count.
        self.check_table_manifest(path)?;
        // Adopt the persisted baseline timestamp so the age signal survives
        // restarts. A missing timestamp refuses the open; a persisted future
        // value (clock skew) adopts the larger of the two with a warning,
        // never moves the signal backward.
        match Self::read_table_manifest(path)?.and_then(|m| m.last_full_flush_ms) {
            Some(persisted) => {
                use std::sync::atomic::Ordering;
                let now = now_ms();
                let adopted = persisted.max(now);
                if persisted > now {
                    log::warn!(
                        "vertex table '{}' baseline timestamp {} is ahead of the current clock {}; \
                         adopting the persisted value",
                        self.label_name,
                        persisted,
                        now,
                    );
                }
                if self.last_full_flush_ms.load(Ordering::Acquire) == 0 {
                    self.last_full_flush_ms.store(adopted, Ordering::Release);
                }
            }
            None => {
                return Err(linkrs_core::StorageError::deserialize_error(format!(
                    "vertex table '{}' manifest at {} has no baseline timestamp: \
                     rebuild the table with the offline redistribution tool instead of opening it in place",
                    self.label_name,
                    path.display(),
                )));
            }
        }
        match Self::read_commit_manifest(path)? {
            Some(manifest) => {
                self.verify_commit_manifest(path, &manifest)?;
                // Manifest-pinned sidecar verification runs before any shard
                // loads: a missing sidecar is simply absent (kept resident),
                // a checksum-mismatched or unparseable sidecar is pruned as
                // a discardable cache so a tampered-but-valid sidecar can
                // never serve stale values. Pruning only deletes derived
                // `.snapshot` files, never authoritative pages, and never
                // fails the open.
                let pruned = prune_tampered_sidecars(path, &manifest.sidecars);
                if pruned > 0 {
                    log::warn!(
                        "vertex table '{}' discarded {} tampered snapshot sidecars; keeping chunks resident",
                        self.label_name,
                        pruned,
                    );
                }
                for (i, shard) in self.shards.iter().enumerate() {
                    let shard_dir = path.join(format!("shard_{}", i));
                    if !shard_dir.exists() {
                        return Err(linkrs_core::StorageError::deserialize_error(format!(
                            "class={} checkpoint epoch {} incomplete: shard directory missing: file={}",
                            CorruptionClass::Fatal.as_str(),
                            manifest.epoch,
                            shard_dir.display(),
                        )));
                    }
                    let mut table = shard.write();
                    table.load(&shard_dir).map_err(|e| {
                        // Shard-scoped damage: the failing shard is known,
                        // so report the isolatable class naming it instead
                        // of a table-wide fatal. Strict open still refuses.
                        linkrs_core::StorageError::deserialize_error(format!(
                            "class={} checkpoint epoch {} shard {} corrupt at file={}: {}",
                            CorruptionClass::Isolatable.as_str(),
                            manifest.epoch,
                            i,
                            shard_dir.display(),
                            e
                        ))
                    })?;
                }
                Ok(())
            }
            None => Err(linkrs_core::StorageError::deserialize_error(format!(
                "class={} vertex table '{}' missing commit manifest at file={}: refusing open without checkpoint pin",
                CorruptionClass::Fatal.as_str(),
                self.label_name,
                path.join(COMMIT_MANIFEST_FILE_NAME).display(),
            ))),
        }
    }

    pub fn apply_delta_pages<P: AsRef<Path>>(&self, path: P) -> StorageResult<()> {
        let path = path.as_ref();
        match Self::read_commit_manifest(path)? {
            Some(manifest) => self.apply_delta_pages_strict(path, &manifest),
            None => Err(linkrs_core::StorageError::deserialize_error(format!(
                "missing commit manifest at {}: refusing delta apply without checkpoint pin",
                path.join(COMMIT_MANIFEST_FILE_NAME).display(),
            ))),
        }
    }

    fn apply_delta_pages_strict(
        &self,
        path: &Path,
        manifest: &CommitManifest,
    ) -> StorageResult<()> {
        self.verify_commit_manifest(path, manifest)?;
        for (i, shard) in self.shards.iter().enumerate() {
            let shard_dir = path.join(format!("shard_{}", i));
            let has_delta = shard_dir.join("columns_pages").exists()
                || shard_dir.join("timestamps.bin").exists()
                || shard_dir.join("id_indexer.bin").exists()
                || shard_dir.join("id_indexer.delta").exists();
            if !(has_delta && shard_dir.exists()) {
                continue;
            }
            let mut table = shard.write();
            if shard_dir.join("columns_pages").exists() {
                table.apply_delta_pages(&shard_dir).map_err(|e| {
                    linkrs_core::StorageError::deserialize_error(format!(
                        "checkpoint epoch {} shard {} delta corrupt at {}: {}",
                        manifest.epoch,
                        i,
                        shard_dir.join("columns_pages").display(),
                        e
                    ))
                })?;
            }
            let ts_path = shard_dir.join("timestamps.bin");
            if ts_path.exists() {
                table.load_timestamps(&ts_path).map_err(|e| {
                    linkrs_core::StorageError::deserialize_error(format!(
                        "checkpoint epoch {} shard {} timestamps corrupt at {}: {}",
                        manifest.epoch,
                        i,
                        ts_path.display(),
                        e
                    ))
                })?;
            }
            Self::load_pk_overlay_strict(&mut table, &shard_dir, manifest, i)?;
        }
        // Manifest-listed column pages missing from disk never reached the
        // per-page loop above; mark their columns unavailable so later reads
        // fail with the column name instead of serving stale pages silently.
        // Table-critical missing files already refused in verification.
        for rel in &manifest.files {
            if path.join(rel).exists() {
                continue;
            }
            let Some((shard_idx, col)) = parse_shard_column(rel) else {
                continue;
            };
            if shard_idx >= self.shards.len() {
                continue;
            }
            let reason = format!("checkpoint file missing: {}", rel);
            log::warn!(
                "column {} in shard {} unavailable: {}",
                col,
                shard_idx,
                reason
            );
            self.shards[shard_idx]
                .read()
                .mark_column_unavailable(&col, reason);
        }
        Ok(())
    }

    /// Primary-key overlay for one shard: a full `id_indexer.bin` replaces
    /// the baseline (post-compaction anchor); otherwise `id_indexer.delta`
    /// applies onto the baseline state.
    fn load_pk_overlay_strict(
        table: &mut crate::vertex::vertex_table::core::VertexTable,
        shard_dir: &Path,
        manifest: &CommitManifest,
        shard_idx: usize,
    ) -> StorageResult<()> {
        let id_path = shard_dir.join("id_indexer.bin");
        if id_path.exists() {
            return table.load_id_indexer(&id_path).map_err(|e| {
                linkrs_core::StorageError::deserialize_error(format!(
                    "checkpoint epoch {} shard {} pk index corrupt at {}: {}",
                    manifest.epoch,
                    shard_idx,
                    id_path.display(),
                    e
                ))
            });
        }
        let delta_path = shard_dir.join("id_indexer.delta");
        if delta_path.exists() {
            table.load_id_indexer_delta(&delta_path).map_err(|e| {
                linkrs_core::StorageError::deserialize_error(format!(
                    "checkpoint epoch {} shard {} pk delta corrupt at {}: {}",
                    manifest.epoch,
                    shard_idx,
                    delta_path.display(),
                    e
                ))
            })?;
        }
        Ok(())
    }
}
