//! Topology group persistence: base payloads plus append-log sidecars.

use super::super::core::EdgeStore;
use super::layout::{in_append_path, in_group_path, out_append_path, out_group_path};
use super::snapshot_cache::{backfill_snapshot_file, sync_snapshot_file};
use crate::edge::node_group::{decode_append_ops, encode_append_ops, TableShardManifest};
use crate::edge::edge_table::checkpoint::snapshot::{
    snapshot_path_for, write_snapshot_file, MappedFrozen,
};
use crate::edge::{CsrBase, CsrVariant};
use graphdb_core::{StorageError, StorageResult};
use std::path::Path;

/// One-direction checkpoint context for group flushes: destination layout,
/// compression knobs, section ids and the shard manifest used for append-op
/// encoding.
pub(crate) struct GroupFlush<'a> {
    pub dir: &'a Path,
    pub page_size: usize,
    pub level: i32,
    pub outgoing: bool,
    pub section_id: u32,
    pub append_section_id: u32,
    pub manifest: &'a TableShardManifest,
}

impl EdgeStore {
    /// Write one direction group by group, each in its own mode.
    ///
    /// Groups carrying delete dirt rewrite the base file and absorb (then
    /// delete) their append sidecar. Insert-only groups persist the
    /// cumulative append sidecar alone: the on-disk sidecar is read back,
    /// merged with the in-memory log, and rewritten, then the memory row
    /// index is dropped. When the merged op count reaches the configured
    /// per-group append bound, the group rewrites the base instead so the
    /// sidecar stays bounded; the bound-triggered rewrite never marks the
    /// checkpoint as rebalanced. Clean groups whose base files exist are
    /// skipped and contribute zero bytes. Returns `(bytes, rebalanced_any)`.
    pub(crate) fn flush_group_set(&mut self, flush: &GroupFlush<'_>) -> StorageResult<(u64, bool)> {
        let dir = flush.dir;
        let page_size = flush.page_size;
        let level = flush.level;
        let outgoing = flush.outgoing;
        let section_id = flush.section_id;
        let append_section_id = flush.append_section_id;
        let manifest = flush.manifest;
        let existing: Vec<usize> = if outgoing {
            self.out_csr.existing_group_ids()
        } else {
            self.in_csr.existing_group_ids()
        };
        let mut written = 0u64;
        let mut rebalanced = false;
        let mut dump_scratch = crate::edge::mutable_csr::persistence::CsrDumpScratch::new();
        for gid in existing {
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
                backfill_snapshot_file(
                    &base_path,
                    shards.group_variant(gid).ok_or_else(|| {
                        StorageError::deserialize_error(format!("group {} missing on flush", gid))
                    })?,
                )?;
                continue;
            }
            if shards.group_needs_rebalance(gid) || !base_path.exists() {
                let had_delete_dirt = shards.group_needs_rebalance(gid);
                let variant = shards.group_variant(gid).ok_or_else(|| {
                    StorageError::deserialize_error(format!("group {} missing on flush", gid))
                })?;
                let mut payload = Vec::new();
                super::super::persistence::serialize_csr_with_scratch(
                    variant,
                    section_id,
                    &mut payload,
                    &mut dump_scratch,
                )?;
                super::super::persistence::write_pages_to_file(
                    &base_path,
                    &payload,
                    page_size,
                    level,
                    variant.edge_count() as u32,
                )?;
                written += super::layout::file_bytes(&base_path);
                if append_path.exists() {
                    let _ = std::fs::remove_file(&append_path);
                }
                sync_snapshot_file(&base_path, variant)?;
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
                    let (raw, _) = super::super::persistence::read_pages_from_file(&append_path)?;
                    let carried = Self::unwrap_append_sidecar(&raw, append_section_id)?;
                    let (prior_inserts, prior_deletes) = decode_append_ops(&carried, manifest)?;
                    merged_inserts = prior_inserts;
                    merged_deletes = prior_deletes;
                }
                let (mem_inserts, mem_deletes) = shards.group_append_ops(gid);
                merged_inserts.extend(mem_inserts);
                merged_deletes.extend(mem_deletes);
                let op_count = merged_inserts.len() + merged_deletes.len();
                let bound = self.config.max_append_ops_per_group;
                if bound != 0 && op_count >= bound {
                    let variant = shards.group_variant(gid).ok_or_else(|| {
                        StorageError::deserialize_error(format!("group {} missing on flush", gid))
                    })?;
                    let mut payload = Vec::new();
                    super::super::persistence::serialize_csr_with_scratch(
                        variant,
                        section_id,
                        &mut payload,
                        &mut dump_scratch,
                    )?;
                    super::super::persistence::write_pages_to_file(
                        &base_path,
                        &payload,
                        page_size,
                        level,
                        variant.edge_count() as u32,
                    )?;
                    written += super::layout::file_bytes(&base_path);
                    if append_path.exists() {
                        let _ = std::fs::remove_file(&append_path);
                    }
                    sync_snapshot_file(&base_path, variant)?;
                    let shards = if outgoing {
                        &mut self.out_csr
                    } else {
                        &mut self.in_csr
                    };
                    shards.clear_group_dirty(gid);
                    shards.clear_group_append_log(gid);
                } else {
                    let ops = encode_append_ops(manifest, &merged_inserts, &merged_deletes);
                    let mut payload = Vec::new();
                    crate::persistence::write_header_to(&mut payload, append_section_id).map_err(
                        |e| StorageError::io_error(format!("Failed to write append header: {}", e)),
                    )?;
                    payload.extend_from_slice(&(ops.len() as u64).to_le_bytes());
                    payload.extend_from_slice(&ops);
                    super::super::persistence::write_pages_to_file(
                        &append_path,
                        &payload,
                        page_size,
                        level,
                        op_count as u32,
                    )?;
                    written += super::layout::file_bytes(&append_path);
                    let shards = if outgoing {
                        &mut self.out_csr
                    } else {
                        &mut self.in_csr
                    };
                    shards.clear_group_dirty(gid);
                    shards.clear_group_append_log(gid);
                }
            }
        }
        Ok((written, rebalanced))
    }

    /// Unwrap one append-sidecar page payload: checks the section header,
    /// length prefix and trailing bytes, returning the append-op bytes.
    pub(crate) fn unwrap_append_sidecar(
        raw: &[u8],
        expected_section: u32,
    ) -> StorageResult<Vec<u8>> {
        use std::io::Read;
        let mut cursor = raw;
        let mut header_buf = [0u8; crate::persistence::HEADER_SIZE];
        cursor.read_exact(&mut header_buf)?;
        {
            let mut slice = &header_buf[..];
            let sid = crate::persistence::read_header(&mut slice)?;
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

    pub(crate) fn load_group_set(
        &mut self,
        dir: &Path,
        outgoing: bool,
        listed: &[u32],
        manifest: &TableShardManifest,
    ) -> StorageResult<()> {
        let append_section = if outgoing {
            crate::persistence::section::EDGE_OUT_APPEND
        } else {
            crate::persistence::section::EDGE_IN_APPEND
        };
        for gid_u32 in listed {
            let gid = *gid_u32 as usize;
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
            // Snapshot cache branch: the base file holds the authoritative
            // heap bytes (mapped groups dump under the shared frozen tag),
            // while the snapshot file is a derived view. A validated mapping
            // skips the authoritative decode and yields the mapped form;
            // anything else falls through to the heap frozen form below, so
            // the mapped identity never roundtrips through the base payload.
            // Pending append deltas rule the view out, since a read-only
            // view cannot absorb the write-through delta. A regenerated
            // snapshot file is a cache: rebuilding it must never fail the load.
            let snapshot = snapshot_path_for(&path);
            if !append_path.exists() {
                match MappedFrozen::open_with_intent(&snapshot, self.config.memory_intent) {
                    Ok(mapped) => {
                        *variant = CsrVariant::Mapped(Box::new(mapped));
                        shards.clear_group_dirty(gid);
                        continue;
                    }
                    Err(error) => {
                        log::debug!(
                            "snapshot cache miss for group {} ({}), falling back to authority: {}",
                            gid,
                            snapshot.display(),
                            error,
                        );
                        if let Some(stats) = &self.stats_manager {
                            stats.add_value(graphdb_metrics::MetricType::SnapshotFallbackCount);
                        }
                    }
                }
            } else {
                log::debug!(
                    "snapshot cache bypassed for group {} with pending append delta",
                    gid,
                );
                if let Some(stats) = &self.stats_manager {
                    stats.add_value(graphdb_metrics::MetricType::SnapshotFallbackCount);
                }
            }
            super::super::persistence::load_csr(&path, variant, expected)?;
            if let CsrVariant::Frozen(csr) = &*variant {
                if let Err(error) = write_snapshot_file(csr, &snapshot) {
                    log::warn!("snapshot cache rebuild failed for group {}: {}", gid, error,);
                    if let Some(stats) = &self.stats_manager {
                        stats.add_value(graphdb_metrics::MetricType::SnapshotFallbackCount);
                    }
                }
            }
            // Sidecars replay the write-through delta on top of the base;
            // the memory row index stays empty because the base now carries
            // the merged state.
            if append_path.exists() {
                let (raw, _) = super::super::persistence::read_pages_from_file(&append_path)?;
                let carried = Self::unwrap_append_sidecar(&raw, append_section)?;
                shards.replay_group_append_log(gid, &carried, manifest)?;
                shards.clear_group_append_log(gid);
            }
            shards.clear_group_dirty(gid);
        }
        Ok(())
    }
}
