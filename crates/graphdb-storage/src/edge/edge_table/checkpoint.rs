//! Incremental checkpoint: per-group topology files plus a manifest.
//!
//! Layout of one edge-table directory, version 3:
//! - `meta.bin`: header section (label ids, schema, next edge id) plus the
//!   authoritative edge timestamps, with the manifest commit tail appended
//!   so metadata and manifest share one atomic unit.
//! - `groups_manifest.bin`: group address width plus out/in group counts.
//! - `out_g{gid}.bin` / `in_g{gid}.bin`: one page-compressed base payload per
//!   group, each self-validated by its row header on load. Rewritten only
//!   for groups carrying delete dirt (base merges) or missing files.
//! - `out_g{gid}.append.bin` / `in_g{gid}.append.bin`: committed append-log
//!   sidecars holding the write-through delta since the group base rewrite.
//!   Insert-only groups checkpoint by persisting the sidecar alone, so
//!   flushed bytes stay proportional to the dirty regions instead of the
//!   group size. The sidecar carries the active manifest and is rejected on
//!   mismatch; it is cumulative across append-only flushes and deleted by
//!   the base merge that absorbs it.
//! - `properties.bin`: property columns plus row visibility.
//!
//! Commit protocol: group bases, sidecars and property payloads are written
//! first (all through atomic shadow files), then the metadata file carrying
//! the manifest tail, and the manifest file is published last as the
//! snapshot commit point. Loading requires the manifest tail embedded in
//! `meta.bin` to equal `groups_manifest.bin`; a mismatch means a torn
//! commit or file damage and the load is rejected. The manifest epoch is
//! the snapshot epoch: shadow (`.tmp`) files written before the manifest
//! commit are discardable uncommitted state reclaimed at startup by the
//! shadow cleanup. Only groups holding uncheckpointed writes are written;
//! clean groups are skipped. Directories holding the old single-file layout
//! (`out_csr.bin` without a manifest), version 1 `meta.bin` without a
//! commit tail, or a pre-version-3 manifest are rejected explicitly, never
//! converted.

use super::core::EdgeStore;
use super::persistence;
use crate::edge::node_group::{decode_append_ops, encode_append_ops, EdgeCheckpointKind, TableShardManifest};
use crate::edge::CsrBase;
use graphdb_core::{StorageError, StorageResult};
use std::path::{Path, PathBuf};

/// Manifest file inside an edge-table directory.
pub const GROUPS_MANIFEST_FILE: &str = "groups_manifest.bin";
/// Legacy single-file topology payloads, rejected when no manifest exists.
pub const LEGACY_OUT_CSR_FILE: &str = "out_csr.bin";
pub const LEGACY_IN_CSR_FILE: &str = "in_csr.bin";

pub fn out_group_file(group: usize) -> String {
    format!("out_g{}.bin", group)
}

pub fn in_group_file(group: usize) -> String {
    format!("in_g{}.bin", group)
}

pub fn out_append_file(group: usize) -> String {
    format!("out_g{}.append.bin", group)
}

pub fn in_append_file(group: usize) -> String {
    format!("in_g{}.append.bin", group)
}

fn out_group_path(dir: &Path, group: usize) -> PathBuf {
    dir.join(out_group_file(group))
}

fn in_group_path(dir: &Path, group: usize) -> PathBuf {
    dir.join(in_group_file(group))
}

fn out_append_path(dir: &Path, group: usize) -> PathBuf {
    dir.join(out_append_file(group))
}

fn in_append_path(dir: &Path, group: usize) -> PathBuf {
    dir.join(in_append_file(group))
}

fn manifest_path(dir: &Path) -> PathBuf {
    dir.join(GROUPS_MANIFEST_FILE)
}

/// File size for checkpoint byte accounting. Metrics must never fail a
/// checkpoint, so a missing file reports zero instead of an error.
fn file_bytes(path: &Path) -> u64 {
    std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0)
}

impl EdgeStore {
    /// Mark property columns dirty. Every operation mutating property state
    /// calls this so checkpoints can skip the property file when clean.
    pub(crate) fn mark_properties_dirty(&mut self) {
        self.properties_dirty = true;
    }

    /// Mark property columns dirty and trace the owning groups.
    ///
    /// Property-only writes leave topology files untouched, so the group
    /// trace records column dirt only and never forces a topology rewrite.
    /// The group trace is a sampled observability caliber: out-of-range
    /// endpoints leave no group trace, and the table-level flag alone
    /// guarantees the final property flush.
    pub(crate) fn mark_properties_dirty_for_edge(&mut self, src: u32, dst: u32) {
        self.properties_dirty = true;
        self.out_csr.mark_column_updated_for(src);
        self.in_csr.mark_column_updated_for(dst);
    }

    /// Flush dirty state incrementally: topology group bases or append
    /// sidecars plus property columns first, metadata carrying the manifest
    /// tail second, manifest last.
    ///
    /// Each group picks its own mode: groups carrying delete dirt rewrite
    /// the base and absorb (then delete) their sidecar; insert-only groups
    /// persist the cumulative append sidecar alone, so small writes never
    /// trigger a whole-group rewrite and flushed bytes stay proportional to
    /// the dirty regions. In-memory append row indexes are dropped after the
    /// flush that persists them; the on-disk sidecar stays cumulative until
    /// a base merge absorbs it. Groups and properties land in shadow files
    /// before the metadata commit so a crash before the manifest publish
    /// leaves only discardable `.tmp` state plus the previous consistent
    /// snapshot. Property statistics are refreshed before the property
    /// payload is serialized so they follow the checkpoint. The checkpoint
    /// kind is Rebalance when any group merged a base, AppendOnly otherwise.
    /// Flushed bytes, elapsed time and authority tombstone totals are
    /// reported to the shared metrics registry when one is set. Returns the
    /// checkpoint kind for engine-side logging.
    pub(crate) fn flush_incremental(
        &mut self,
        dir: &Path,
        page_size: usize,
        level: i32,
    ) -> StorageResult<EdgeCheckpointKind> {
        let started = std::time::Instant::now();
        std::fs::create_dir_all(dir)?;
        crate::compression::cleanup_shadow_files(dir)?;

        let manifest = TableShardManifest {
            group_bits: self.config.node_group_bits,
            out_groups: self.out_csr.group_count() as u32,
            in_groups: self.in_csr.group_count() as u32,
        };
        let dirty_groups =
            self.out_csr.dirty_group_ids().len() + self.in_csr.dirty_group_ids().len();

        let mut flushed_bytes = 0u64;
        let (out_bytes, out_rebalanced) = self.flush_group_set(
            dir,
            page_size,
            level,
            true,
            crate::persistence::section::EDGE_OUT_CSR,
            crate::persistence::section::EDGE_OUT_APPEND,
            &manifest,
        )?;
        flushed_bytes += out_bytes;
        let (in_bytes, in_rebalanced) = self.flush_group_set(
            dir,
            page_size,
            level,
            false,
            crate::persistence::section::EDGE_IN_CSR,
            crate::persistence::section::EDGE_IN_APPEND,
            &manifest,
        )?;
        flushed_bytes += in_bytes;
        let kind = if out_rebalanced || in_rebalanced {
            EdgeCheckpointKind::Rebalance
        } else {
            EdgeCheckpointKind::AppendOnly
        };
        flushed_bytes += self.flush_properties_file(dir, page_size, level)?;
        flushed_bytes += self.flush_metadata_file(dir, page_size, level, &manifest)?;
        self.write_manifest(dir, &manifest)?;
        flushed_bytes += file_bytes(&manifest_path(dir));

        self.properties_dirty = false;
        self.out_csr.clear_all_column_dirty();
        self.in_csr.clear_all_column_dirty();
        self.remove_orphan_group_files(dir);
        log::debug!(
            "EdgeTable[{}] checkpoint kind={:?} dirty_groups={} bytes={}",
            self.label,
            kind,
            dirty_groups,
            flushed_bytes
        );
        if let Some(stats) = &self.stats_manager {
            stats.record_incremental_checkpoint(started.elapsed(), flushed_bytes);
            stats.record_checkpoint_strategy_by_name("incremental");
            let tombstones = self.mvcc.tombstone_stats();
            let active_snapshots: u64 = self.mvcc.active_snapshots.values().sum::<usize>() as u64;
            let saturate = |ts: Option<u64>| ts.map(|v| u32::try_from(v).unwrap_or(u32::MAX));
            stats.record_tombstone_stats(
                tombstones.count as u64,
                tombstones.memory_bytes as u64,
                saturate(tombstones.oldest_delete_ts),
                saturate(tombstones.newest_delete_ts),
                active_snapshots,
            );
        }
        Ok(kind)
    }

    fn flush_metadata_file(
        &self,
        dir: &Path,
        page_size: usize,
        level: i32,
        manifest: &TableShardManifest,
    ) -> StorageResult<u64> {
        // Meta stays a full rewrite: edge timestamps are a single global map
        // with no column sharding yet, so any timestamp change needs the whole
        // table. The header and timestamp sections are serialized separately
        // inside `flush_metadata` as the future per-group split point; this
        // revision keeps both sections in one file. The manifest commit tail
        // is appended so metadata and manifest share one atomic unit: a lone
        // new metadata file without its manifest commit is a torn write.
        // Sharded or delta meta is future work once timestamps split by group.
        let mut meta_payload = Vec::new();
        crate::persistence::write_header_to(
            &mut meta_payload,
            crate::persistence::section::EDGE_META,
        )
        .map_err(|e| StorageError::io_error(format!("Failed to write edge meta header: {}", e)))?;
        persistence::flush_metadata(
            &mut meta_payload,
            self.label,
            self.src_label,
            self.dst_label,
            &self.label_name,
            self.is_open,
            &self.schema,
            self.next_edge_id,
            &self.mvcc.edge_timestamps,
        )?;
        meta_payload.extend_from_slice(&manifest.encode());
        let path = dir.join("meta.bin");
        persistence::write_pages_to_file(&path, &meta_payload, page_size, level, 1)?;
        Ok(file_bytes(&path))
    }

    fn flush_properties_file(
        &mut self,
        dir: &Path,
        page_size: usize,
        level: i32,
    ) -> StorageResult<u64> {
        let path = dir.join("properties.bin");
        if !self.properties_dirty && path.exists() {
            return Ok(0);
        }
        // Column-level follow-up: only dirty columns recompute stats, clean
        // columns keep persisted values. The file itself is still a full
        // rewrite; per-column files are future work once properties split.
        self.properties.refresh_column_stats();
        let mut props_payload = Vec::new();
        persistence::serialize_csr_properties(&self.properties, &mut props_payload)?;
        let edge_count = self.properties.row_count() as u32;
        persistence::write_pages_to_file(&path, &props_payload, page_size, level, edge_count)?;
        self.properties.clear_dirty_columns();
        Ok(file_bytes(&path))
    }

    /// Write one direction group by group, each in its own mode.
    ///
    /// Groups carrying delete dirt rewrite the base file and absorb (then
    /// delete) their append sidecar. Insert-only groups persist the
    /// cumulative append sidecar alone: the on-disk sidecar is read back,
    /// merged with the in-memory log, and rewritten, then the memory row
    /// index is dropped. Clean groups whose base files exist are skipped
    /// and contribute zero bytes. Returns `(bytes, rebalanced_any)`.
    fn flush_group_set(
        &mut self,
        dir: &Path,
        page_size: usize,
        level: i32,
        outgoing: bool,
        section_id: u32,
        append_section_id: u32,
        manifest: &TableShardManifest,
    ) -> StorageResult<(u64, bool)> {
        let group_count = if outgoing {
            self.out_csr.group_count()
        } else {
            self.in_csr.group_count()
        };
        let mut written = 0u64;
        let mut rebalanced = false;
        for gid in 0..group_count {
            let base_path = if outgoing {
                out_group_path(dir, gid)
            } else {
                in_group_path(dir, gid)
            };
            let append_path = if outgoing {
                out_append_path(dir, gid)
            } else {
                in_append_path(dir, gid)
            };
            let shards = if outgoing {
                &self.out_csr
            } else {
                &self.in_csr
            };
            if !shards.needs_checkpoint(gid)
                && !shards.group_has_append_log(gid)
                && base_path.exists()
            {
                continue;
            }
            if shards.group_needs_rebalance(gid) || !base_path.exists() {
                let had_delete_dirt = shards.group_needs_rebalance(gid);
                let variant = shards.group_variant(gid).cloned().ok_or_else(|| {
                    StorageError::deserialize_error(format!("group {} missing on flush", gid))
                })?;
                let mut payload = Vec::new();
                persistence::serialize_csr(&variant, section_id, &mut payload)?;
                persistence::write_pages_to_file(
                    &base_path,
                    &payload,
                    page_size,
                    level,
                    variant.edge_count() as u32,
                )?;
                written += file_bytes(&base_path);
                if append_path.exists() {
                    let _ = std::fs::remove_file(&append_path);
                }
                let shards = if outgoing {
                    &mut self.out_csr
                } else {
                    &mut self.in_csr
                };
                shards.clear_group_dirty(gid);
                shards.clear_group_append_log(gid);
                rebalanced |= had_delete_dirt;
            } else {
                let shards = if outgoing {
                    &self.out_csr
                } else {
                    &self.in_csr
                };
                let mut merged_inserts = Vec::new();
                let mut merged_deletes = Vec::new();
                if append_path.exists() {
                    let (raw, _) = persistence::read_pages_from_file(&append_path)?;
                    let carried = Self::unwrap_append_sidecar(&raw, append_section_id)?;
                    let (prior_inserts, prior_deletes) =
                        decode_append_ops(&carried, manifest)?;
                    merged_inserts = prior_inserts;
                    merged_deletes = prior_deletes;
                }
                let (mem_inserts, mem_deletes) = shards.group_append_ops(gid);
                merged_inserts.extend(mem_inserts);
                merged_deletes.extend(mem_deletes);
                let ops = encode_append_ops(manifest, &merged_inserts, &merged_deletes);
                let mut payload = Vec::new();
                crate::persistence::write_header_to(&mut payload, append_section_id).map_err(
                    |e| StorageError::io_error(format!("Failed to write append header: {}", e)),
                )?;
                payload.extend_from_slice(&(ops.len() as u64).to_le_bytes());
                payload.extend_from_slice(&ops);
                let op_count = merged_inserts.len() + merged_deletes.len();
                persistence::write_pages_to_file(
                    &append_path,
                    &payload,
                    page_size,
                    level,
                    op_count as u32,
                )?;
                written += file_bytes(&append_path);
                let shards = if outgoing {
                    &mut self.out_csr
                } else {
                    &mut self.in_csr
                };
                shards.clear_group_dirty(gid);
                shards.clear_group_append_log(gid);
            }
        }
        Ok((written, rebalanced))
    }

    /// Unwrap one append-sidecar page payload: checks the section header,
    /// length prefix and trailing bytes, returning the append-op bytes.
    fn unwrap_append_sidecar(raw: &[u8], expected_section: u32) -> StorageResult<Vec<u8>> {
        use std::io::Read;
        let mut cursor = &raw[..];
        let mut header_buf = [0u8; crate::persistence::HEADER_SIZE];
        cursor.read_exact(&mut header_buf)?;
        {
            let mut slice = &header_buf[..];
            let (_version, sid) = crate::persistence::read_header(&mut slice)?;
            if sid != expected_section {
                return Err(StorageError::deserialize_error(format!(
                    "unexpected section id in append sidecar: expected {:#06x}, got {:#06x}",
                    expected_section, sid
                )));
            }
        }
        let mut len_bytes = [0u8; 8];
        cursor.read_exact(&mut len_bytes)?;
        let len = u64::from_le_bytes(len_bytes) as usize;
        let mut data = vec![0u8; len];
        cursor.read_exact(&mut data)?;
        if !cursor.is_empty() {
            return Err(StorageError::deserialize_error(
                "unexpected trailing data in append sidecar".to_string(),
            ));
        }
        Ok(data)
    }

    /// Publish the manifest as the snapshot commit point. Must be called
    /// after the metadata file carrying the same manifest tail is durable.
    fn write_manifest(&self, dir: &Path, manifest: &TableShardManifest) -> StorageResult<()> {
        crate::compression::write_shadow_file(manifest_path(dir), &manifest.encode())
    }

    fn remove_orphan_group_files(&self, dir: &Path) {
        let mut gid = self.out_csr.group_count();
        loop {
            let base = out_group_path(dir, gid);
            let append = out_append_path(dir, gid);
            let base_gone = !base.exists() || std::fs::remove_file(&base).is_err();
            if append.exists() {
                let _ = std::fs::remove_file(&append);
            }
            if base_gone {
                break;
            }
            gid += 1;
        }
        let mut gid = self.in_csr.group_count();
        loop {
            let base = in_group_path(dir, gid);
            let append = in_append_path(dir, gid);
            let base_gone = !base.exists() || std::fs::remove_file(&base).is_err();
            if append.exists() {
                let _ = std::fs::remove_file(&append);
            }
            if base_gone {
                break;
            }
            gid += 1;
        }
    }

    /// Load an incremental checkpoint. Directories in the old single-file
    /// layout, version 1 metadata without a commit tail, and pre-version-3
    /// manifests are rejected explicitly, never converted.
    ///
    /// Group bases load first, then append sidecars replay on top; sidecar
    /// version, manifest, section and trailing-byte mismatches fail the
    /// load. Damage detection only: version, section, trailing-byte and
    /// manifest-tail mismatches fail the load. Crash consistency comes from
    /// the commit protocol (group bases, sidecars and properties before
    /// metadata, manifest published last with its tail embedded in
    /// `meta.bin`), covered by the crash-injection tests below instead of by
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
        let manifest_bytes = std::fs::read(&manifest_file)
            .map_err(|e| StorageError::io_error(format!("Failed to read group manifest: {}", e)))?;
        let manifest = TableShardManifest::decode(&manifest_bytes)?;
        if manifest.group_bits != self.config.node_group_bits {
            return Err(StorageError::deserialize_error(format!(
                "group layout mismatch: file uses {} address bits, table uses {}",
                manifest.group_bits, self.config.node_group_bits
            )));
        }

        self.load_metadata_file(dir, &manifest)?;
        self.out_csr.resize_groups(manifest.out_groups as usize)?;
        self.in_csr.resize_groups(manifest.in_groups as usize)?;
        self.load_group_set(dir, true, manifest.out_groups as usize, &manifest)?;
        self.load_group_set(dir, false, manifest.in_groups as usize, &manifest)?;
        self.load_properties_file(dir)?;

        if self.next_edge_id.0 == 0 {
            let max_id = self
                .out_csr
                .iter_all()
                .map(|(_, nbr)| nbr.edge_id.0 + 1)
                .max()
                .unwrap_or(0);
            self.next_edge_id = graphdb_core::types::EdgeId(max_id);
        }
        // Damage detection, not crash recovery: with the manifest-tail commit
        // protocol a torn write fails earlier on the tail mismatch. A nonzero
        // count here means corrupt or regressed files and the load stays
        // fail-closed.
        let (orphan_mappings, orphan_csr_rows) = self.loaded_copy_mismatches();
        let live_orphans = self.live_authority_orphans();
        if orphan_mappings + orphan_csr_rows + live_orphans > 0 {
            return Err(crate::StorageError::db_error(format!(
                "edge table {} loaded with copy mismatches: \
                 orphan property mappings={}, orphan CSR rows={}, \
                 live authority orphans={}",
                self.label_name,
                orphan_mappings,
                orphan_csr_rows,
                live_orphans,
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
        self.is_open = true;
        Ok(())
    }

    fn load_metadata_file(
        &mut self,
        dir: &Path,
        manifest: &TableShardManifest,
    ) -> StorageResult<()> {
        use std::io::Read;
        let meta_path = dir.join("meta.bin");
        let (meta_data, _meta_rows) = persistence::read_pages_from_file(&meta_path)?;
        let mut meta_cursor = &meta_data[..];
        let mut header_buf = [0u8; crate::persistence::HEADER_SIZE];
        meta_cursor.read_exact(&mut header_buf)?;
        {
            let mut slice = &header_buf[..];
            let (_version, sid) = crate::persistence::read_header(&mut slice)?;
            if sid != crate::persistence::section::EDGE_META {
                return Err(StorageError::deserialize_error(format!(
                    "unexpected section id in edge meta: expected {:#06x}, got {:#06x}",
                    crate::persistence::section::EDGE_META,
                    sid
                )));
            }
        }

        let mut version_bytes = [0u8; 4];
        meta_cursor.read_exact(&mut version_bytes)?;
        let version = u32::from_le_bytes(version_bytes);
        if version == 1 {
            return Err(StorageError::deserialize_error(
                "legacy edge meta version 1 without a manifest commit tail is not supported"
                    .to_string(),
            ));
        }
        if version != persistence::EDGE_META_VERSION {
            return Err(StorageError::deserialize_error(format!(
                "unsupported edge meta version: {}",
                version
            )));
        }

        let meta = persistence::load_metadata(&mut meta_cursor)?;
        let embedded = TableShardManifest::decode(meta_cursor).map_err(|_| {
            StorageError::deserialize_error(
                "edge meta missing manifest commit tail: torn write or legacy file".to_string(),
            )
        })?;
        if embedded != *manifest {
            return Err(StorageError::deserialize_error(format!(
                "manifest commit tail mismatch: meta carries {:?}, manifest file holds {:?}",
                embedded, manifest
            )));
        }
        self.label = meta.label;
        self.src_label = meta.src_label;
        self.dst_label = meta.dst_label;
        self.label_name = meta.label_name;
        self.is_open = meta.is_open;
        self.set_schema(meta.schema);
        self.next_edge_id = meta.next_edge_id;
        self.mvcc.edge_timestamps = meta.edge_timestamps;
        self.mvcc.min_active_snapshot_ts = graphdb_core::types::Timestamp::MAX;
        self.mvcc.active_snapshots.clear();
        Ok(())
    }

    fn load_group_set(
        &mut self,
        dir: &Path,
        outgoing: bool,
        count: usize,
        manifest: &TableShardManifest,
    ) -> StorageResult<()> {
        let append_section = if outgoing {
            crate::persistence::section::EDGE_OUT_APPEND
        } else {
            crate::persistence::section::EDGE_IN_APPEND
        };
        for gid in 0..count {
            let path = if outgoing {
                out_group_path(dir, gid)
            } else {
                in_group_path(dir, gid)
            };
            let append_path = if outgoing {
                out_append_path(dir, gid)
            } else {
                in_append_path(dir, gid)
            };
            let expected = if outgoing {
                crate::persistence::section::EDGE_OUT_CSR
            } else {
                crate::persistence::section::EDGE_IN_CSR
            };
            let shards = if outgoing {
                &mut self.out_csr
            } else {
                &mut self.in_csr
            };
            let variant = shards.group_variant_mut(gid).ok_or_else(|| {
                StorageError::deserialize_error(format!("group {} missing on load", gid))
            })?;
            persistence::load_csr(&path, variant, expected)?;
            // Sidecars replay the write-through delta on top of the base;
            // the memory row index stays empty because the base now carries
            // the merged state.
            if append_path.exists() {
                let (raw, _) = persistence::read_pages_from_file(&append_path)?;
                let carried = Self::unwrap_append_sidecar(&raw, append_section)?;
                shards.replay_group_append_log(gid, &carried, manifest)?;
                shards.clear_group_append_log(gid);
            }
            shards.clear_group_dirty(gid);
        }
        Ok(())
    }

    fn load_properties_file(&mut self, dir: &Path) -> StorageResult<()> {
        use crate::edge::property_schema::PropertySchema;
        let props_path = dir.join("properties.bin");
        self.properties = {
            let prop_schemas: Vec<PropertySchema> = self
                .schema
                .properties
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    PropertySchema::new(p.name.clone(), i as i32, p.data_type.clone())
                        .nullable(p.nullable)
                        .with_default_value(p.default_value.clone())
                })
                .collect();
            persistence::load_csr_properties(&props_path, prop_schemas)?
        };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edge::edge_table::config::EdgeTableConfig;
    use crate::edge::{EdgeSchema, EdgeStrategy};
    use crate::types::StoragePropertyDef;
    use graphdb_core::Value;

    fn make_table() -> EdgeStore {
        let schema = EdgeSchema {
            label_id: 0,
            label_name: "knows".to_string(),
            src_label: 0,
            dst_label: 0,
            properties: vec![StoragePropertyDef {
                name: "weight".to_string(),
                data_type: graphdb_core::types::DataType::Double,
                nullable: false,
                default_value: Some(Value::Double(0.0)),
            }],
            oe_strategy: EdgeStrategy::Multiple,
            ie_strategy: EdgeStrategy::Multiple,
            schema_version: 1,
        };
        EdgeStore::with_config(schema, EdgeTableConfig::default()).unwrap()
    }

    #[test]
    fn clean_groups_are_skipped_on_flush() {
        let mut table = make_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("flush should succeed");
        let group_zero = dir.path().join(out_group_file(0));
        assert!(group_zero.exists());
        let stamp = group_zero.metadata().unwrap().modified().unwrap();

        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("second flush should succeed");
        assert_eq!(group_zero.metadata().unwrap().modified().unwrap(), stamp);
    }

    #[test]
    fn dirty_group_roundtrip_preserves_cross_group_edges() {
        let mut table = make_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        table.insert_edge(5000, 6000, 0, &[], 100).unwrap();
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("flush should succeed");
        assert!(dir.path().join(out_group_file(1)).exists());

        let mut loaded = make_table();
        loaded.load(dir.path()).expect("load should succeed");
        assert!(loaded.has_edge(0, 1, 0, 200));
        assert!(loaded.has_edge(5000, 6000, 0, 200));
        assert_eq!(loaded.edge_count(), 2);
        assert!(loaded.out_csr.dirty_group_ids().is_empty());
        assert!(loaded.in_csr.dirty_group_ids().is_empty());
    }

    #[test]
    fn legacy_layout_without_manifest_is_rejected() {
        let mut table = make_table();
        table.insert_edge(0, 1, 0, &[], 100).unwrap();
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        std::fs::create_dir_all(dir.path()).unwrap();
        let mut payload = Vec::new();
        let variant = table.out_csr.group_variant(0).unwrap().clone();
        persistence::serialize_csr(
            &variant,
            crate::persistence::section::EDGE_OUT_CSR,
            &mut payload,
        )
        .unwrap();
        persistence::write_pages_to_file(
            &dir.path().join(LEGACY_OUT_CSR_FILE),
            &payload,
            crate::compression::DEFAULT_PAGE_SIZE,
            3,
            1,
        )
        .unwrap();

        let mut loaded = make_table();
        let err = loaded
            .load(dir.path())
            .expect_err("legacy layout must be rejected");
        assert!(err.to_string().contains("legacy single-file"));
    }

    #[test]
    fn properties_file_skipped_when_clean() {
        let mut table = make_table();
        table.insert_edge(0, 1, 0, &[], 100).unwrap();
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("flush should succeed");
        let props = dir.path().join("properties.bin");
        let stamp = props.metadata().unwrap().modified().unwrap();
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("second flush should succeed");
        assert_eq!(props.metadata().unwrap().modified().unwrap(), stamp);
    }

    #[test]
    fn flush_records_incremental_checkpoint_metrics() {
        use graphdb_metrics::{MetricType, StatsManager};

        let mut table = make_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        let stats = std::sync::Arc::new(StatsManager::new());
        table.set_stats_manager(stats.clone());
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("flush should succeed");
        let bytes = stats
            .get_value(MetricType::CheckpointIncrementalBytesFlushed)
            .unwrap_or(0);
        assert!(bytes > 0, "flushed bytes should be recorded");
        assert_eq!(
            stats.get_value(MetricType::CheckpointStrategyIncremental),
            Some(1)
        );
    }

    #[test]
    fn flush_without_metrics_registry_behaves_the_same() {
        let mut table = make_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("flush without a registry should succeed");
        let mut loaded = make_table();
        loaded.load(dir.path()).expect("load should succeed");
        assert!(loaded.has_edge(0, 1, 0, 200));
    }

    #[test]
    fn unpublished_column_is_dropped_on_reload() {
        let mut table = make_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        // Fill the physical column but never publish: a crash here must be
        // equivalent to aborting the staged change.
        table
            .prepare_add_property("score".to_string(), graphdb_core::DataType::Int, true, None)
            .unwrap();
        table.fill_pending_add_property().unwrap();
        assert!(table.properties.has_property("score"));
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("flush should succeed");

        let mut loaded = make_table();
        loaded.load(dir.path()).expect("load should succeed");
        assert!(!loaded.properties.has_property("score"));
        assert!(!loaded.schema.properties.iter().any(|p| p.name == "score"));
        assert!(loaded.has_edge(0, 1, 0, 200));
        assert!(loaded.pending_add_column().is_none());
    }

    #[test]
    fn published_column_survives_reload_with_stats() {
        let mut table = make_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(3.0))], 100)
            .unwrap();
        table
            .prepare_add_property(
                "score".to_string(),
                graphdb_core::DataType::Int,
                true,
                Some(Value::Int(7)),
            )
            .unwrap();
        table.fill_pending_add_property().unwrap();
        table.publish_pending_add_property().unwrap();
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("flush should succeed");

        // A fresh table only knows the published schema when loading.
        let schema = EdgeSchema {
            label_id: 0,
            label_name: "knows".to_string(),
            src_label: 0,
            dst_label: 0,
            properties: vec![
                StoragePropertyDef::new(
                    "weight".to_string(),
                    graphdb_core::types::DataType::Double,
                ),
                StoragePropertyDef::new("score".to_string(), graphdb_core::types::DataType::Int),
            ],
            oe_strategy: EdgeStrategy::Multiple,
            ie_strategy: EdgeStrategy::Multiple,
            schema_version: 1,
        };
        let mut loaded =
            EdgeStore::with_config(schema, EdgeTableConfig::default()).expect("table builds");
        loaded.load(dir.path()).expect("load should succeed");
        assert!(loaded.properties.has_property("score"));
        let snapshot = loaded
            .column_stats_snapshot("weight")
            .expect("flushed stats should be queryable");
        assert_eq!(snapshot.row_count, 1);
        assert_eq!(snapshot.min_value, Some(Value::Double(3.0)));
        assert_eq!(snapshot.max_value, Some(Value::Double(3.0)));
    }

    #[test]
    fn encoded_values_survive_reload_with_encoding() {
        let mut table = make_table();
        for i in 0..20 {
            table
                .insert_edge(
                    i,
                    i + 100,
                    0,
                    &[("weight".to_string(), Value::Double(i as f64))],
                    100,
                )
                .unwrap();
        }
        let encoded = table.encode_property_columns();
        assert!(encoded > 0);
        let before = table.properties.column_encoding_type("weight");
        assert!(before.is_some_and(|enc| enc != crate::encoding::EncodingType::None));
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("flush should succeed");

        let mut loaded = make_table();
        loaded.load(dir.path()).expect("load should succeed");
        assert_eq!(loaded.properties.column_encoding_type("weight"), before);
        let record = loaded.get_edge(3, 103, 0, 200).expect("edge survives");
        assert!(record
            .properties
            .iter()
            .any(|(k, v)| k == "weight" && *v == Value::Double(3.0)));
        let snapshot = loaded
            .column_stats_snapshot("weight")
            .expect("flushed stats should be queryable");
        assert_eq!(snapshot.row_count, 20);
        assert!(snapshot.null_count.is_some());
    }

    #[test]
    fn legacy_properties_version_is_rejected() {
        let mut table = make_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        let mut payload = table.properties.dump();
        assert!(!payload.is_empty());
        payload[0] = 2;
        let mut reloaded = make_table();
        assert!(reloaded.properties.load(&payload).is_err());
    }

    #[test]
    fn stable_column_ids_survive_drop_and_reload() {
        let mut table = make_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        table
            .add_property("score".to_string(), graphdb_core::DataType::Int, true)
            .expect("add score should succeed");
        let score_id = table
            .properties
            .get_property_id("score")
            .expect("score id should exist");
        table
            .remove_property("weight")
            .expect("drop should succeed");
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("flush should succeed");

        let schema = EdgeSchema {
            label_id: 0,
            label_name: "knows".to_string(),
            src_label: 0,
            dst_label: 0,
            properties: vec![StoragePropertyDef::new(
                "score".to_string(),
                graphdb_core::types::DataType::Int,
            )],
            oe_strategy: EdgeStrategy::Multiple,
            ie_strategy: EdgeStrategy::Multiple,
            schema_version: 1,
        };
        let mut loaded =
            EdgeStore::with_config(schema, EdgeTableConfig::default()).expect("table builds");
        loaded.load(dir.path()).expect("load should succeed");
        assert_eq!(loaded.properties.get_property_id("score"), Some(score_id));
        loaded
            .add_property("extra".to_string(), graphdb_core::DataType::Int, true)
            .expect("add extra should succeed");
        let extra_id = loaded
            .properties
            .get_property_id("extra")
            .expect("extra id should exist");
        assert!(extra_id != score_id);
        assert!(extra_id > score_id);
    }

    #[test]
    fn insert_only_flush_reports_append_only() {
        use crate::edge::EdgeCheckpointKind;

        let mut table = make_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        let kind = table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("flush should succeed");
        assert_eq!(kind, EdgeCheckpointKind::AppendOnly);
    }

    #[test]
    fn delete_flush_reports_rebalance() {
        use crate::edge::EdgeCheckpointKind;

        let mut table = make_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        table
            .insert_edge(0, 2, 0, &[("weight".to_string(), Value::Double(2.0))], 100)
            .unwrap();
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("flush should succeed");
        table.delete_edge(0, 1, 0, 200).unwrap();
        let kind = table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("flush should succeed");
        assert_eq!(kind, EdgeCheckpointKind::Rebalance);
    }

    #[test]
    fn property_only_update_skips_topology_rewrite() {
        use crate::edge::EdgeCheckpointKind;

        let mut table = make_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("flush should succeed");
        let group_zero = dir.path().join(out_group_file(0));
        let stamp = group_zero.metadata().unwrap().modified().unwrap();

        table
            .update_edge_property(0, 1, 0, "weight", &Value::Double(9.0), 200)
            .expect("property update should succeed");
        assert!(!table.out_csr.sampled_column_dirty_group_ids().is_empty());
        let kind = table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("flush should succeed");
        assert_eq!(kind, EdgeCheckpointKind::AppendOnly);
        assert_eq!(group_zero.metadata().unwrap().modified().unwrap(), stamp);
    }

    #[test]
    fn remap_forces_rebalance_checkpoint() {
        use crate::edge::EdgeCheckpointKind;
        use std::collections::HashMap;

        let mut table = make_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        table.insert_edge(5000, 6000, 0, &[], 100).unwrap();
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("flush should succeed");
        assert!(table.out_csr.dirty_group_ids().is_empty());

        let src_mapping: HashMap<u32, u32> = [(5000u32, 2u32)].into_iter().collect();
        let dst_mapping: HashMap<u32, u32> = [(6000u32, 3u32)].into_iter().collect();
        table
            .remap_vertex_ids(Some(&src_mapping), Some(&dst_mapping))
            .expect("remap should succeed");
        assert!(!table.out_csr.dirty_group_ids().is_empty());
        assert!(table.has_edge(2, 3, 0, 200));
        let kind = table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("flush should succeed");
        assert_eq!(kind, EdgeCheckpointKind::Rebalance);
    }

    #[test]
    fn flush_reports_tombstone_totals_to_registry() {
        use graphdb_metrics::{MetricType, StatsManager};

        let mut table = make_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        table
            .insert_edge(0, 2, 0, &[("weight".to_string(), Value::Double(2.0))], 100)
            .unwrap();
        table.delete_edge(0, 1, 0, 200).unwrap();
        let stats = std::sync::Arc::new(StatsManager::new());
        table.set_stats_manager(stats.clone());
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("flush should succeed");
        assert_eq!(stats.get_value(MetricType::TombstoneCount), Some(1));
        assert!(
            stats
                .get_value(MetricType::TombstoneMemoryBytes)
                .unwrap_or(0)
                > 0
        );
    }

    #[test]
    fn successful_flush_loads_consistent_triple() {
        let mut table = make_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        table
            .insert_edge(0, 2, 0, &[("weight".to_string(), Value::Double(2.0))], 110)
            .unwrap();
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("flush should succeed");

        let mut loaded = make_table();
        loaded.load(dir.path()).expect("load should succeed");
        assert!(loaded.has_edge(0, 1, 0, 200));
        assert!(loaded.has_edge(0, 2, 0, 200));
        let record = loaded.get_edge(0, 1, 0, 200).expect("edge survives");
        assert!(record
            .properties
            .iter()
            .any(|(k, v)| k == "weight" && *v == Value::Double(1.0)));
        assert_eq!(
            loaded.mvcc.creation_ts_of(graphdb_core::types::EdgeId(0)),
            Some(100)
        );
        assert_eq!(
            loaded.mvcc.creation_ts_of(graphdb_core::types::EdgeId(1)),
            Some(110)
        );
        assert_eq!(loaded.edge_count(), 2);
    }

    #[test]
    fn torn_manifest_tail_is_rejected_not_mixed() {
        use crate::edge::node_group::TableShardManifest;
        let mut table = make_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("flush should succeed");

        // Simulate a crash between the metadata write and the manifest
        // publish: metadata carries the new tail while the manifest file
        // still holds the previous snapshot. The load must fail closed on
        // the tail mismatch instead of presenting mixed topology plus
        // timestamps.
        let manifest_path = dir.path().join(GROUPS_MANIFEST_FILE);
        let bytes = std::fs::read(&manifest_path).expect("manifest readable");
        let mut manifest = TableShardManifest::decode(&bytes).expect("manifest decodes");
        manifest.out_groups += 10;
        crate::compression::write_shadow_file(&manifest_path, &manifest.encode())
            .expect("torn manifest writable");

        let mut loaded = make_table();
        let err = loaded
            .load(dir.path())
            .expect_err("torn commit must be rejected");
        assert!(err.to_string().contains("mismatch"));
    }

    #[test]
    fn legacy_meta_without_tail_is_rejected() {
        let mut table = make_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("flush should succeed");

        // Strip the manifest commit tail and downgrade the version to mimic
        // a version 1 file: the loader must reject it explicitly.
        let meta_path = dir.path().join("meta.bin");
        let (mut payload, _) =
            persistence::read_pages_from_file(&meta_path).expect("meta readable");
        assert!(payload.len() > 20);
        payload.truncate(payload.len() - 16);
        let header_len = crate::persistence::HEADER_SIZE;
        payload[header_len..header_len + 4].copy_from_slice(&1u32.to_le_bytes());
        persistence::write_pages_to_file(
            &meta_path,
            &payload,
            crate::compression::DEFAULT_PAGE_SIZE,
            3,
            1,
        )
        .expect("legacy meta writable");

        let mut loaded = make_table();
        let err = loaded
            .load(dir.path())
            .expect_err("legacy meta must be rejected");
        assert!(err.to_string().contains("legacy"));
    }

    #[test]
    fn crash_before_manifest_publish_never_loads_mixed_snapshot() {
        let mut table = make_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("first flush should succeed");
        let old_manifest = std::fs::read(dir.path().join(GROUPS_MANIFEST_FILE))
            .expect("old manifest readable");

        table
            .insert_edge(5000, 6000, 0, &[("weight".to_string(), Value::Double(2.0))], 110)
            .unwrap();
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("second flush should succeed");

        // Crash between the second metadata write and its manifest publish:
        // restore the old manifest file while the new metadata tail stays.
        // Loading must fail on the mismatch rather than mix new timestamps
        // with old topology.
        std::fs::write(dir.path().join(GROUPS_MANIFEST_FILE), &old_manifest)
            .expect("manifest restore works");
        let mut loaded = make_table();
        let err = loaded
            .load(dir.path())
            .expect_err("torn second commit must be rejected");
        assert!(err.to_string().contains("mismatch"));

        // Restoring the matching new manifest is out of scope for the loader;
        // a clean retry of the whole flush from live memory publishes both.
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("retry flush should succeed");
        let mut reloaded = make_table();
        reloaded.load(dir.path()).expect("load should succeed");
        assert!(reloaded.has_edge(0, 1, 0, 200));
        assert!(reloaded.has_edge(5000, 6000, 0, 200));
    }

    #[test]
    fn append_only_flush_skips_base_rewrite() {
        use crate::edge::EdgeCheckpointKind;

        let mut table = make_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        let kind = table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("first flush should succeed");
        assert_eq!(kind, EdgeCheckpointKind::AppendOnly);
        // First flush of a new group writes the base; no sidecar exists yet.
        let base_path = dir.path().join(out_group_file(0));
        assert!(base_path.exists());
        assert!(!dir.path().join(out_append_file(0)).exists());
        let stamp = base_path.metadata().unwrap().modified().unwrap();

        table
            .insert_edge(0, 2, 0, &[("weight".to_string(), Value::Double(2.0))], 110)
            .unwrap();
        let kind = table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("second flush should succeed");
        assert_eq!(kind, EdgeCheckpointKind::AppendOnly);
        // Insert-only groups persist the sidecar alone: the base is untouched.
        assert_eq!(base_path.metadata().unwrap().modified().unwrap(), stamp);
        assert!(dir.path().join(out_append_file(0)).exists());

        let mut loaded = make_table();
        loaded.load(dir.path()).expect("load should succeed");
        assert!(loaded.has_edge(0, 1, 0, 200));
        assert!(loaded.has_edge(0, 2, 0, 200));
        assert_eq!(loaded.edge_count(), 2);
    }

    #[test]
    fn cumulative_sidecars_survive_two_append_flushes() {
        let mut table = make_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("first flush should succeed");
        // Two insert-only batches with a flush each: the second sidecar must
        // accumulate the first, never overwrite it.
        table
            .insert_edge(0, 2, 0, &[("weight".to_string(), Value::Double(2.0))], 110)
            .unwrap();
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("second flush should succeed");
        table
            .insert_edge(0, 3, 0, &[("weight".to_string(), Value::Double(3.0))], 120)
            .unwrap();
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("third flush should succeed");

        let mut loaded = make_table();
        loaded.load(dir.path()).expect("load should succeed");
        assert!(loaded.has_edge(0, 1, 0, 200));
        assert!(loaded.has_edge(0, 2, 0, 200));
        assert!(loaded.has_edge(0, 3, 0, 200));
        assert_eq!(loaded.edge_count(), 3);
    }

    #[test]
    fn delete_flush_rewrites_base_and_drops_sidecar() {
        use crate::edge::EdgeCheckpointKind;

        let mut table = make_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        table
            .insert_edge(0, 2, 0, &[("weight".to_string(), Value::Double(2.0))], 100)
            .unwrap();
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("first flush should succeed");
        table
            .insert_edge(0, 3, 0, &[("weight".to_string(), Value::Double(3.0))], 110)
            .unwrap();
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("append flush should succeed");
        assert!(dir.path().join(out_append_file(0)).exists());

        table.delete_edge(0, 1, 0, 200).unwrap();
        let kind = table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("delete flush should succeed");
        assert_eq!(kind, EdgeCheckpointKind::Rebalance);
        // The base merge absorbs the sidecar: no append file remains.
        assert!(!dir.path().join(out_append_file(0)).exists());

        let mut loaded = make_table();
        loaded.load(dir.path()).expect("load should succeed");
        assert!(!loaded.has_edge(0, 1, 0, 250));
        assert!(loaded.has_edge(0, 2, 0, 250));
        assert!(loaded.has_edge(0, 3, 0, 250));
        assert_eq!(loaded.edge_count(), 2);
    }

    #[test]
    fn small_write_sidecar_stays_proportional_to_dirty_scale() {
        let mut table = make_table();
        for i in 0..200u32 {
            table
                .insert_edge(i, i + 1000, 0, &[], 100)
                .unwrap();
        }
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("baseline flush should succeed");
        let base_len = std::fs::metadata(dir.path().join(out_group_file(0)))
            .expect("base readable")
            .len();

        table.insert_edge(0, 2001, 0, &[], 110).unwrap();
        table.insert_edge(1, 2002, 0, &[], 110).unwrap();
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("small flush should succeed");
        let sidecar_len = std::fs::metadata(dir.path().join(out_append_file(0)))
            .expect("sidecar readable")
            .len();
        // Two fresh edges persist as a small delta, not a group rewrite.
        assert!(
            (sidecar_len as f64) < (base_len as f64) / 4.0,
            "sidecar {} must stay far below base {}",
            sidecar_len,
            base_len
        );
        // Region dirt is cleared by the flush that persists it.
        assert!(table.out_csr.dirty_region_ids(0).is_empty());
    }

    #[test]
    fn torn_sidecar_is_rejected_not_replayed() {
        let mut table = make_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("first flush should succeed");
        table
            .insert_edge(0, 2, 0, &[("weight".to_string(), Value::Double(2.0))], 110)
            .unwrap();
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("append flush should succeed");
        let sidecar = dir.path().join(out_append_file(0));
        assert!(sidecar.exists());
        // Corrupt the sidecar payload (valid pages, broken ops section).
        let (mut raw, _) =
            persistence::read_pages_from_file(&sidecar).expect("sidecar readable");
        assert!(raw.len() > 32);
        raw.truncate(raw.len() - 4);
        persistence::write_pages_to_file(
            &sidecar,
            &raw,
            crate::compression::DEFAULT_PAGE_SIZE,
            3,
            1,
        )
        .expect("torn sidecar writable");

        let mut loaded = make_table();
        assert!(
            loaded.load(dir.path()).is_err(),
            "torn sidecar must fail the load"
        );
    }

    #[test]
    fn pre_v3_manifest_is_rejected() {
        let mut table = make_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("flush should succeed");
        // Hand-craft a version 2 manifest: same body, old version.
        let mut legacy = 2u32.to_le_bytes().to_vec();
        let current = std::fs::read(dir.path().join(GROUPS_MANIFEST_FILE))
            .expect("manifest readable");
        legacy.extend_from_slice(&current[4..]);
        std::fs::write(dir.path().join(GROUPS_MANIFEST_FILE), &legacy)
            .expect("legacy manifest writable");

        let mut loaded = make_table();
        let err = loaded
            .load(dir.path())
            .expect_err("pre-v3 manifest must be rejected");
        assert!(err.to_string().contains("unsupported group manifest version"));
    }
}
