//! Incremental checkpoint: per-group topology, timestamp and property shards
//! plus segment statistics and a manifest.
//!
//! Layout of one edge-table directory, version 5:
//! - `meta.bin`: header section only (label ids, schema, next edge id), with
//!   the manifest commit tail appended so metadata and manifest share one
//!   atomic unit.
//! - `groups_manifest.bin`: address width plus existing out/in group id lists.
//!   Missing groups read as empty and never produce files.
//! - `out_g{gid}.bin` / `in_g{gid}.bin`: one page-compressed base payload per
//!   existing group, each self-validated by its row header on load. Topology
//!   columns persist through the integer column path with per-column
//!   bit-packing or run-length encoding and a plain fallback. Rewritten
//!   only for groups carrying delete dirt (base merges) or missing files.
//! - `out_g{gid}.append.bin` / `in_g{gid}.append.bin`: committed append-log
//!   sidecars holding the write-through delta since the group base rewrite.
//!   Insert-only groups checkpoint by persisting the sidecar alone, so
//!   flushed bytes stay proportional to the dirty regions instead of the
//!   group size. The sidecar carries only the address width and is rejected
//!   on mismatch; it is cumulative across append-only flushes and deleted by
//!   the base merge that absorbs it.
//! - `ts_g{gid}.bin`: authoritative timestamps for the owning group's edges,
//!   falling with the same dirt as the group. Small timestamp writes rewrite
//!   only dirty owners, never the whole table.
//! - `props_g{gid}.bin`: property rows for the owning group's edges, falling
//!   with the same dirt as the group. Small property writes rewrite only
//!   dirty owners, never the whole table.
//! - `segment_stats.bin`: per-group segment statistics collected at the
//!   checkpoint for scan pruning, persisted alongside the shards and
//!   restored on load.
//!
//! Commit protocol: group bases, sidecars, timestamp shards, property shards
//! and the segment-statistics snapshot are written first (all through atomic
//! shadow files), then the metadata file carrying the manifest tail, and the
//! manifest file is published last as the snapshot commit point. Loading trusts
//! the manifest tail embedded in `meta.bin` when it differs from
//! `groups_manifest.bin`: metadata is published before the manifest, so a torn
//! commit holds new metadata plus the previous manifest file, and the embedded
//! tail already describes the durable group files. A missing tail still means
//! file damage and the load is rejected. The manifest
//! epoch is the snapshot epoch: shadow (`.tmp`) files written before the
//! manifest commit are discardable uncommitted state reclaimed at startup by
//! the shadow cleanup. Only groups holding uncheckpointed writes are written;
//! clean groups are skipped, so flushed bytes stay proportional to dirty
//! groups rather than the table size. Directories holding the old single-file
//! layout (`out_csr.bin` without a manifest), version 1 `meta.bin` without a
//! commit tail, version 2 `meta.bin` with global timestamps, the legacy
//! global `properties.bin`, or a pre-version-5 manifest are rejected
//! explicitly, never converted.

use super::core::EdgeStore;
use super::persistence;
use crate::edge::node_group::{
    decode_append_ops, encode_append_ops, EdgeCheckpointKind, TableShardManifest,
};
use crate::edge::CsrBase;
use graphdb_core::{StorageError, StorageResult};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Manifest file inside an edge-table directory.
pub const GROUPS_MANIFEST_FILE: &str = "groups_manifest.bin";
/// Segment-statistics snapshot inside an edge-table directory.
pub const SEGMENT_STATS_FILE: &str = "segment_stats.bin";
/// Legacy single-file topology payloads, rejected when no manifest exists.
pub const LEGACY_OUT_CSR_FILE: &str = "out_csr.bin";
pub const LEGACY_IN_CSR_FILE: &str = "in_csr.bin";
/// Legacy global property file, rejected when present.
pub const LEGACY_PROPERTIES_FILE: &str = "properties.bin";

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

pub fn ts_group_file(group: u32) -> String {
    format!("ts_g{}.bin", group)
}

pub fn props_group_file(group: u32) -> String {
    format!("props_g{}.bin", group)
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

fn ts_group_path(dir: &Path, group: u32) -> PathBuf {
    dir.join(ts_group_file(group))
}

fn props_group_path(dir: &Path, group: u32) -> PathBuf {
    dir.join(props_group_file(group))
}

fn manifest_path(dir: &Path) -> PathBuf {
    dir.join(GROUPS_MANIFEST_FILE)
}

fn segment_stats_path(dir: &Path) -> PathBuf {
    dir.join(SEGMENT_STATS_FILE)
}

/// Parse `"<prefix>{gid}.bin"` or `"<prefix>{gid}.append.bin"` into the gid.
/// Returns `None` for foreign files so orphan cleanup never deletes them.
/// The match is exact: a shared numeric prefix with a foreign suffix (for
/// example `out_g3_evil.bin`) is third-party and skipped.
fn parse_group_file(name: &str, prefix: &str) -> Option<u32> {
    let rest = name.strip_prefix(prefix)?;
    for suffix in [".append.bin", ".bin"] {
        if let Some(digits) = rest.strip_suffix(suffix) {
            if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
                return digits.parse::<u32>().ok();
            }
            return None;
        }
    }
    None
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
    /// The trace is precise: every write marks its owning group, and the
    /// table-level flag remains only as a correctness insurance.
    pub(crate) fn mark_properties_dirty_for_edge(&mut self, src: u32, dst: u32) {
        self.properties_dirty = true;
        self.out_csr.mark_column_updated_for(src);
        self.in_csr.mark_column_updated_for(dst);
    }

    /// Flush dirty state incrementally: topology group bases or append
    /// sidecars, per-group timestamp shards and per-group property shards
    /// first, metadata carrying the manifest tail second, manifest last.
    ///
    /// Each group picks its own mode: groups carrying delete dirt rewrite
    /// the base and absorb (then delete) their sidecar; insert-only groups
    /// persist the cumulative append sidecar alone, so small writes never
    /// trigger a whole-group rewrite and flushed bytes stay proportional to
    /// the dirty regions. Timestamp and property shards fall with the same
    /// dirt as their owner group, so small writes rewrite only dirty owners
    /// rather than global files. In-memory append row indexes are dropped
    /// after the flush that persists them; the on-disk sidecar stays
    /// cumulative until a base merge absorbs it. Groups and shards land in
    /// shadow files before the metadata commit so a crash before the manifest
    /// publish leaves only discardable `.tmp` state plus the previous
    /// consistent snapshot. Property statistics are refreshed before shards
    /// are serialized so they follow the checkpoint. The checkpoint kind is
    /// Rebalance when any group merged a base, AppendOnly otherwise.
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
            out_groups: self
                .out_csr
                .existing_group_ids()
                .into_iter()
                .map(|gid| gid as u32)
                .collect(),
            in_groups: self
                .in_csr
                .existing_group_ids()
                .into_iter()
                .map(|gid| gid as u32)
                .collect(),
        };
        let dirty_groups =
            self.out_csr.dirty_group_ids().len() + self.in_csr.dirty_group_ids().len();

        let mut flushed_bytes = 0u64;
        flushed_bytes += self.flush_timestamp_shards(dir, page_size, level)?;
        flushed_bytes += self.flush_property_shards(dir, page_size, level)?;
        self.refresh_segment_stats();
        flushed_bytes += self.flush_segment_stats(dir, page_size, level)?;
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
        flushed_bytes += self.flush_metadata_file(dir, page_size, level, &manifest)?;
        self.write_manifest(dir, &manifest)?;
        flushed_bytes += file_bytes(&manifest_path(dir));

        self.properties_dirty = false;
        self.out_csr.clear_all_column_dirty();
        self.in_csr.clear_all_column_dirty();
        self.remove_orphan_group_files(dir);
        let encoding_saved: usize = self
            .topology_encoding_report()
            .iter()
            .map(|(_, _, plain, encoded)| plain.saturating_sub(*encoded))
            .sum();
        log::debug!(
            "EdgeTable[{}] checkpoint kind={:?} dirty_groups={} bytes={} live_edges={} encoding_saved={}",
            self.label,
            kind,
            dirty_groups,
            flushed_bytes,
            self.out_csr.edge_count() + self.in_csr.edge_count(),
            encoding_saved
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
            // Single-caliber fragmentation observability: the trigger gate in
            // the compaction module consumes the same ratio, so the panel and
            // the trigger can never disagree on the value.
            let mut total_capacity = 0usize;
            let mut wasted_capacity = 0usize;
            for shard in [&self.out_csr, &self.in_csr] {
                if let Some(fragment) = shard.fragmentation_stats() {
                    total_capacity += fragment.total_capacity;
                    wasted_capacity += fragment.wasted_capacity;
                }
            }
            let ratio = if total_capacity == 0 {
                0.0
            } else {
                wasted_capacity as f32 / total_capacity as f32
            };
            stats.record_fragmentation_stats(
                ratio,
                (self.out_csr.wasted_bytes_estimate() + self.in_csr.wasted_bytes_estimate()) as u64,
            );
        }
        self.wal_dir = Some(dir.to_path_buf());
        let _ = super::wal::truncate(dir);
        Ok(kind)
    }

    fn flush_metadata_file(
        &self,
        dir: &Path,
        page_size: usize,
        level: i32,
        manifest: &TableShardManifest,
    ) -> StorageResult<u64> {
        // Header-only rewrite: timestamps live in per-group shards falling
        // with the same dirt as their owner, so any timestamp change needs
        // only dirty owners' shards. The manifest commit tail is appended so
        // metadata and manifest share one atomic unit: a lone new metadata
        // file without its manifest commit is a torn write.
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
        )?;
        meta_payload.extend_from_slice(&manifest.encode());
        let path = dir.join("meta.bin");
        persistence::write_pages_to_file(&path, &meta_payload, page_size, level, 1)?;
        Ok(file_bytes(&path))
    }

    /// Owner groups holding uncheckpointed timestamp writes: the owner
    /// direction's topology dirt. Timestamps change only on insert and
    /// delete, both of which dirty the owner topology group.
    fn timestamp_dirty_owners(&self) -> Vec<u32> {
        if self.schema.oe_strategy != crate::edge::EdgeStrategy::None {
            self.out_csr
                .dirty_group_ids()
                .into_iter()
                .map(|gid| gid as u32)
                .collect()
        } else if self.schema.ie_strategy != crate::edge::EdgeStrategy::None {
            self.in_csr
                .dirty_group_ids()
                .into_iter()
                .map(|gid| gid as u32)
                .collect()
        } else {
            Vec::new()
        }
    }

    /// Owner groups whose timestamp or property shards must be rewritten.
    /// Topology dirt always covers inserts and deletes; precise column
    /// traces cover property-only writes. When the table flag reports
    /// property dirt but no group trace exists, every owner rewrites as a
    /// correctness insurance that the regular path never needs. Each
    /// insurance rewrite increments the fallback counter for observability.
    fn property_dirty_owners(&mut self) -> Vec<u32> {
        let (dirty, column_traced, existing) =
            if self.schema.oe_strategy != crate::edge::EdgeStrategy::None {
                (
                    self.out_csr.dirty_group_ids(),
                    self.out_csr.column_dirty_group_ids(),
                    self.out_csr.existing_group_ids(),
                )
            } else if self.schema.ie_strategy != crate::edge::EdgeStrategy::None {
                (
                    self.in_csr.dirty_group_ids(),
                    self.in_csr.column_dirty_group_ids(),
                    self.in_csr.existing_group_ids(),
                )
            } else {
                return Vec::new();
            };
        let mut set: HashSet<u32> = HashSet::new();
        for gid in dirty.into_iter().chain(column_traced.into_iter()) {
            set.insert(gid as u32);
        }
        if set.is_empty() && self.properties_dirty {
            for gid in existing {
                set.insert(gid as u32);
            }
            self.property_fallback_rewrites += 1;
        }
        let mut out: Vec<u32> = set.into_iter().collect();
        out.sort_unstable();
        out
    }

    /// Group authority entries of the wanted shards by owning shard, falling
    /// back to the smallest existing owner when the recorded owner is gone
    /// (reclaimed groups whose tombstone survives). Missing owners read as
    /// empty shards. Grouping memory follows the wanted set rather than the
    /// table size.
    fn grouped_timestamps(
        &self,
        wanted: &HashSet<u32>,
    ) -> HashMap<
        u32,
        Vec<(
            graphdb_core::types::EdgeId,
            crate::edge::edge_table::mvcc::EdgeTimestamps,
        )>,
    > {
        let owners = self.owner_group_ids();
        let fallback = owners.first().copied();
        let live: HashSet<u32> = owners.into_iter().collect();
        let mut grouped: HashMap<
            u32,
            Vec<(
                graphdb_core::types::EdgeId,
                crate::edge::edge_table::mvcc::EdgeTimestamps,
            )>,
        > = HashMap::new();
        let mut fallback_hits = 0usize;
        for (edge_id, ts) in self.mvcc.edge_timestamps.iter() {
            let (gid, fell_back) =
                Self::resolve_owner_gid(&edge_id, &self.edge_owner, &live, fallback);
            fallback_hits += usize::from(fell_back);
            if wanted.contains(&gid) {
                grouped.entry(gid).or_default().push((edge_id, *ts));
            }
        }
        if fallback_hits > 0 {
            log::debug!(
                "grouped_timestamps: {} authority entries fell back to smallest owner",
                fallback_hits
            );
        }
        for entries in grouped.values_mut() {
            entries.sort_by_key(|(edge_id, _)| edge_id.0);
        }
        grouped
    }

    fn flush_timestamp_shards(
        &self,
        dir: &Path,
        page_size: usize,
        level: i32,
    ) -> StorageResult<u64> {
        let mut dirty_set: HashSet<u32> = self.timestamp_dirty_owners().into_iter().collect();
        // A fresh checkpoint directory holds no shard files yet. Dirty
        // tracking alone would skip every owner after a preceding save
        // cleared the flags, leaving CSR bases without authority shards.
        // Missing files always join the write set so each checkpoint is a
        // complete snapshot while repeated flushes to the same data dir
        // stay incremental.
        for gid in self.owner_group_ids() {
            if !ts_group_path(dir, gid).exists() {
                dirty_set.insert(gid);
            }
        }
        if dirty_set.is_empty() {
            return Ok(0);
        }
        let mut dirty: Vec<u32> = dirty_set.iter().copied().collect();
        dirty.sort_unstable();
        let grouped = self.grouped_timestamps(&dirty_set);
        let mut written = 0u64;
        for gid in dirty {
            let entries = grouped.get(&gid).cloned().unwrap_or_default();
            let path = ts_group_path(dir, gid);
            if entries.is_empty() {
                if path.exists() {
                    let _ = std::fs::remove_file(&path);
                }
                continue;
            }
            let mut payload = Vec::new();
            persistence::serialize_timestamp_shard(
                &entries,
                crate::persistence::section::EDGE_TS_SHARD,
                &mut payload,
            )?;
            persistence::write_pages_to_file(
                &path,
                &payload,
                page_size,
                level,
                entries.len() as u32,
            )?;
            written += file_bytes(&path);
        }
        Ok(written)
    }

    fn flush_property_shards(
        &mut self,
        dir: &Path,
        page_size: usize,
        level: i32,
    ) -> StorageResult<u64> {
        use crate::edge::property_schema::PropertySchema;
        let mut dirty_set: HashSet<u32> = if self.properties_dirty {
            self.property_dirty_owners().into_iter().collect()
        } else {
            HashSet::new()
        };
        // Same fresh-directory completeness rule as timestamp shards:
        // a new checkpoint must carry property rows even when the
        // preceding save already cleared the dirty flags.
        for gid in self.owner_group_ids() {
            if !props_group_path(dir, gid).exists() {
                dirty_set.insert(gid);
            }
        }
        if dirty_set.is_empty() {
            return Ok(0);
        }
        let mut dirty: Vec<u32> = dirty_set.into_iter().collect();
        dirty.sort_unstable();
        self.properties.refresh_column_stats();
        let owners = self.owner_group_ids();
        let fallback = owners.first().copied();
        let live: HashSet<u32> = owners.into_iter().collect();
        // Group dirty owners only: the single owner-map pass files each row
        // into its dirty shard slot, so grouping memory follows the dirty set
        // rather than the table size. Clean groups reuse their files untouched.
        let wanted: HashSet<u32> = dirty.iter().copied().collect();
        let mut by_owner: HashMap<u32, Vec<graphdb_core::types::EdgeId>> = HashMap::new();
        let mut fallback_hits = 0usize;
        for edge_id in self.properties.edge_ids() {
            let (gid, fell_back) =
                Self::resolve_owner_gid(&edge_id, &self.edge_owner, &live, fallback);
            fallback_hits += usize::from(fell_back);
            if wanted.contains(&gid) {
                by_owner.entry(gid).or_default().push(edge_id);
            }
        }
        if fallback_hits > 0 {
            log::debug!(
                "flush_property_shards: {} property rows fell back to smallest owner",
                fallback_hits
            );
        }
        let schema: Vec<PropertySchema> = self.properties.property_schema().to_vec();
        let dirty_columns: Vec<String> = self.properties.dirty_column_names();
        let topology_dirty: HashSet<u32> = self
            .out_csr
            .dirty_group_ids()
            .into_iter()
            .map(|gid| gid as u32)
            .chain(
                self.in_csr
                    .dirty_group_ids()
                    .into_iter()
                    .map(|gid| gid as u32),
            )
            .collect();
        let mut written = 0u64;
        for gid in dirty {
            let path = props_group_path(dir, gid);
            let edges = by_owner.get(&gid).cloned().unwrap_or_default();
            if edges.is_empty() {
                if path.exists() {
                    let _ = std::fs::remove_file(&path);
                }
                continue;
            }
            if !topology_dirty.contains(&gid) && path.exists() && !dirty_columns.is_empty() {
                let incremental = Self::flush_property_shard_incremental(
                    &mut self.properties,
                    &path,
                    &schema,
                    &edges,
                    &dirty_columns,
                )
                .unwrap_or(None);
                if let Some(bytes) = incremental {
                    persistence::write_pages_to_file(
                        &path,
                        &bytes,
                        page_size,
                        level,
                        edges.len() as u32,
                    )?;
                    written += file_bytes(&path);
                    continue;
                }
            }
            let mut shard = crate::edge::CsrWithProperties::new(schema.clone());
            for edge_id in edges {
                if let Some((create_ts, delete_ts, values)) = self.properties.export_row(edge_id) {
                    let _ = shard.import_row(edge_id, create_ts, delete_ts, &values);
                }
            }
            for column in schema.iter().map(|s| s.name.clone()).collect::<Vec<_>>() {
                if let Some(enc) = self.properties.column_encoding_type(&column) {
                    if enc != crate::encoding::EncodingType::None {
                        let _ = shard.apply_encoding_to_column(&column, enc, 255);
                    }
                }
            }
            shard.refresh_column_stats();
            let mut payload = Vec::new();
            persistence::serialize_property_shard(
                &shard,
                crate::persistence::section::EDGE_PROPS_SHARD,
                &mut payload,
            )?;
            persistence::write_pages_to_file(
                &path,
                &payload,
                page_size,
                level,
                shard.row_count() as u32,
            )?;
            written += file_bytes(&path);
        }
        self.properties.clear_dirty_columns();
        Ok(written)
    }

    /// Incremental property-shard rewrite for property-only dirt.
    ///
    /// Loads the last flushed shard, patches only `dirty_columns` from the
    /// live property store, re-encodes only those columns and refreshes only
    /// their statistics. Clean columns reuse the last flushed bytes without
    /// re-export or re-encode, so checkpoint work follows dirty columns
    /// rather than the full row. Returns `Ok(None)` when the row set or
    /// schema changed, signalling the caller to take the full rewrite path
    /// rather than risking a missed write.
    fn flush_property_shard_incremental(
        live: &mut crate::edge::CsrWithProperties,
        path: &Path,
        schema: &[crate::edge::property_schema::PropertySchema],
        edges: &[graphdb_core::types::EdgeId],
        dirty_columns: &[String],
    ) -> StorageResult<Option<Vec<u8>>> {
        use std::io::Read as _;
        let Ok((raw, _)) = persistence::read_pages_from_file(path) else {
            return Ok(None);
        };
        let mut cursor = &raw[..];
        let mut header_buf = [0u8; crate::persistence::HEADER_SIZE];
        if cursor.read_exact(&mut header_buf).is_err() {
            return Ok(None);
        }
        {
            let mut slice = &header_buf[..];
            let Ok((_version, sid)) = crate::persistence::read_header(&mut slice) else {
                return Ok(None);
            };
            if sid != crate::persistence::section::EDGE_PROPS_SHARD {
                return Ok(None);
            }
        }
        let mut len_bytes = [0u8; 8];
        if cursor.read_exact(&mut len_bytes).is_err() {
            return Ok(None);
        }
        let len = u64::from_le_bytes(len_bytes) as usize;
        let mut data = vec![0u8; len];
        if cursor.read_exact(&mut data).is_err() || !cursor.is_empty() {
            return Ok(None);
        }
        let mut shard = crate::edge::CsrWithProperties::new(schema.to_vec());
        if shard.load(&data).is_err() {
            return Ok(None);
        }
        let shard_edges: HashSet<graphdb_core::types::EdgeId> = shard.edge_ids().collect();
        let live_set: HashSet<graphdb_core::types::EdgeId> = edges.iter().copied().collect();
        if shard_edges != live_set {
            return Ok(None);
        }
        let shard_cols: HashSet<String> = shard
            .property_schema()
            .iter()
            .map(|s| s.name.clone())
            .collect();
        for name in dirty_columns {
            if !shard_cols.contains(name) {
                return Ok(None);
            }
        }
        for edge_id in edges {
            let Some((_, _, values)) = live.export_row(*edge_id) else {
                return Ok(None);
            };
            let value_map: HashMap<&String, &Option<graphdb_core::Value>> =
                values.iter().map(|(name, value)| (name, value)).collect();
            for name in dirty_columns {
                let value = value_map.get(name).and_then(|cell| (*cell).clone());
                if shard
                    .set_property_for_edge(
                        *edge_id,
                        name,
                        value,
                        graphdb_core::types::MAX_TIMESTAMP,
                    )
                    .is_err()
                {
                    return Ok(None);
                }
            }
        }
        for name in dirty_columns {
            if let Some(enc) = live.column_encoding_type(name) {
                if enc != crate::encoding::EncodingType::None {
                    let _ = shard.apply_encoding_to_column(name, enc, 255);
                }
            }
            shard.refresh_column_stats_for(name);
        }
        let mut payload = Vec::new();
        persistence::serialize_property_shard(
            &shard,
            crate::persistence::section::EDGE_PROPS_SHARD,
            &mut payload,
        )?;
        Ok(Some(payload))
    }

    /// Persist the checkpoint-collected segment statistics snapshot.
    ///
    /// Written on every checkpoint after the collection step so scans prune
    /// from the same epoch as the topology and property shards. The snapshot
    /// shares the manifest commit point: it lands in a shadow file before
    /// the metadata commit, and a crash before the manifest publish leaves
    /// only the previous consistent snapshot.
    fn flush_segment_stats(&self, dir: &Path, page_size: usize, level: i32) -> StorageResult<u64> {
        use super::stats::encode_segment_snapshot;
        let snapshot = encode_segment_snapshot(&self.segment_stats);
        let mut payload = Vec::new();
        crate::persistence::write_header_to(
            &mut payload,
            crate::persistence::section::EDGE_SEGMENT_STATS,
        )
        .map_err(|e| {
            StorageError::io_error(format!("Failed to write segment stats header: {}", e))
        })?;
        payload.extend_from_slice(&(snapshot.len() as u64).to_le_bytes());
        payload.extend_from_slice(&snapshot);
        let path = segment_stats_path(dir);
        persistence::write_pages_to_file(
            &path,
            &payload,
            page_size,
            level,
            self.segment_stats.len() as u32,
        )?;
        Ok(file_bytes(&path))
    }

    /// Load the segment-statistics snapshot, failing closed on section,
    /// version or trailing mismatches. A missing snapshot means an older
    /// layout and the load is rejected, never rebuilt silently.
    fn load_segment_stats(&mut self, dir: &Path) -> StorageResult<()> {
        use super::stats::decode_segment_snapshot;
        use std::io::Read as _;
        let path = segment_stats_path(dir);
        let (raw, _) = persistence::read_pages_from_file(&path).map_err(|e| {
            StorageError::deserialize_error(format!(
                "missing segment statistics snapshot at {}: {}",
                path.display(),
                e
            ))
        })?;
        let mut cursor = &raw[..];
        let mut header_buf = [0u8; crate::persistence::HEADER_SIZE];
        cursor.read_exact(&mut header_buf)?;
        {
            let mut slice = &header_buf[..];
            let (_version, sid) = crate::persistence::read_header(&mut slice)?;
            if sid != crate::persistence::section::EDGE_SEGMENT_STATS {
                return Err(StorageError::deserialize_error(format!(
                    "unexpected section id in segment stats: expected {:#06x}, got {:#06x}",
                    crate::persistence::section::EDGE_SEGMENT_STATS,
                    sid
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
                "unexpected trailing data in segment stats".to_string(),
            ));
        }
        let stats = decode_segment_snapshot(&data)?;
        self.restore_segment_stats(stats);
        Ok(())
    }

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
                continue;
            }
            if shards.group_needs_rebalance(gid) || !base_path.exists() {
                let had_delete_dirt = shards.group_needs_rebalance(gid);
                let variant = shards.group_variant(gid).ok_or_else(|| {
                    StorageError::deserialize_error(format!("group {} missing on flush", gid))
                })?;
                let mut payload = Vec::new();
                persistence::serialize_csr_with_scratch(
                    variant,
                    section_id,
                    &mut payload,
                    &mut dump_scratch,
                )?;
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
                    persistence::serialize_csr_with_scratch(
                        variant,
                        section_id,
                        &mut payload,
                        &mut dump_scratch,
                    )?;
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
                } else {
                    let ops = encode_append_ops(manifest, &merged_inserts, &merged_deletes);
                    let mut payload = Vec::new();
                    crate::persistence::write_header_to(&mut payload, append_section_id).map_err(
                        |e| StorageError::io_error(format!("Failed to write append header: {}", e)),
                    )?;
                    payload.extend_from_slice(&(ops.len() as u64).to_le_bytes());
                    payload.extend_from_slice(&ops);
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
        let out_existing: HashSet<u32> = self
            .out_csr
            .existing_group_ids()
            .into_iter()
            .map(|gid| gid as u32)
            .collect();
        let in_existing: HashSet<u32> = self
            .in_csr
            .existing_group_ids()
            .into_iter()
            .map(|gid| gid as u32)
            .collect();
        let owner_existing: HashSet<u32> =
            if self.schema.oe_strategy != crate::edge::EdgeStrategy::None {
                out_existing.clone()
            } else {
                in_existing.clone()
            };
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                let path = entry.path();
                if name.starts_with("out_g") && name.ends_with(".bin") && !name.contains(".append")
                {
                    if let Some(gid) = parse_group_file(&name, "out_g") {
                        if !out_existing.contains(&gid) {
                            let _ = std::fs::remove_file(&path);
                        }
                    }
                } else if name.starts_with("in_g")
                    && name.ends_with(".bin")
                    && !name.contains(".append")
                {
                    if let Some(gid) = parse_group_file(&name, "in_g") {
                        if !in_existing.contains(&gid) {
                            let _ = std::fs::remove_file(&path);
                        }
                    }
                } else if name.starts_with("out_g") && name.contains(".append") {
                    if let Some(gid) = parse_group_file(&name, "out_g") {
                        if !out_existing.contains(&gid) {
                            let _ = std::fs::remove_file(&path);
                        }
                    }
                } else if name.starts_with("in_g") && name.contains(".append") {
                    if let Some(gid) = parse_group_file(&name, "in_g") {
                        if !in_existing.contains(&gid) {
                            let _ = std::fs::remove_file(&path);
                        }
                    }
                } else if name.starts_with("ts_g") && name.ends_with(".bin") {
                    if let Some(gid) = parse_group_file(&name, "ts_g") {
                        if !owner_existing.contains(&gid) {
                            let _ = std::fs::remove_file(&path);
                        }
                    }
                } else if name.starts_with("props_g") && name.ends_with(".bin") {
                    if let Some(gid) = parse_group_file(&name, "props_g") {
                        if !owner_existing.contains(&gid) {
                            let _ = std::fs::remove_file(&path);
                        }
                    }
                } else if name == LEGACY_PROPERTIES_FILE {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
    }

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
        let wal_ops = super::wal::read_ops(dir)?;
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

    fn load_metadata_file(
        &mut self,
        dir: &Path,
        _file_manifest: &TableShardManifest,
    ) -> StorageResult<TableShardManifest> {
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
        if version == 2 {
            return Err(StorageError::deserialize_error(
                "legacy edge meta version 2 with global timestamps is not supported".to_string(),
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
        self.label = meta.label;
        self.src_label = meta.src_label;
        self.dst_label = meta.dst_label;
        self.label_name = meta.label_name;
        self.is_open = meta.is_open;
        self.set_schema(meta.schema);
        self.next_edge_id = meta.next_edge_id;
        self.mvcc.edge_timestamps.clear();
        self.mvcc.min_active_snapshot_ts = graphdb_core::types::Timestamp::MAX;
        self.mvcc.active_snapshots.clear();
        Ok(embedded)
    }

    fn load_group_set(
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

    fn owner_list_for_load(&self, manifest: &TableShardManifest) -> Vec<u32> {
        if self.schema.oe_strategy != crate::edge::EdgeStrategy::None {
            manifest.out_groups.clone()
        } else {
            manifest.in_groups.clone()
        }
    }

    fn load_timestamp_shards(
        &mut self,
        dir: &Path,
        manifest: &TableShardManifest,
    ) -> StorageResult<()> {
        self.mvcc.edge_timestamps.clear();
        let owners = self.owner_list_for_load(manifest);
        for gid in &owners {
            let path = ts_group_path(dir, *gid);
            if !path.exists() {
                continue;
            }
            let entries = persistence::load_timestamp_shard(
                &path,
                crate::persistence::section::EDGE_TS_SHARD,
            )?;
            for (edge_id, ts) in entries {
                if let Some(prev) = self.mvcc.edge_timestamps.get(&edge_id) {
                    if prev.create_ts != ts.create_ts || prev.delete_ts != ts.delete_ts {
                        return Err(StorageError::deserialize_error(format!(
                            "duplicate timestamp shard entry for edge {:?}",
                            edge_id
                        )));
                    }
                    continue;
                }
                self.mvcc.edge_timestamps.insert(edge_id, ts);
                self.edge_owner.or_insert(edge_id, *gid);
            }
        }
        Ok(())
    }

    fn load_property_shards(
        &mut self,
        dir: &Path,
        manifest: &TableShardManifest,
    ) -> StorageResult<()> {
        use crate::edge::property_schema::PropertySchema;
        use std::io::Read as _;
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
        self.properties = crate::edge::CsrWithProperties::new(prop_schemas.clone());
        let owners = self.owner_list_for_load(manifest);
        let mut encodings: HashMap<String, crate::encoding::EncodingType> = HashMap::new();
        let mut prop_ids: HashMap<String, i32> = HashMap::new();
        for gid in &owners {
            let path = props_group_path(dir, *gid);
            if !path.exists() {
                continue;
            }
            let (raw, _) = persistence::read_pages_from_file(&path)?;
            let mut cursor = &raw[..];
            let mut header_buf = [0u8; crate::persistence::HEADER_SIZE];
            cursor.read_exact(&mut header_buf)?;
            {
                let mut slice = &header_buf[..];
                let (_version, sid) = crate::persistence::read_header(&mut slice)?;
                if sid != crate::persistence::section::EDGE_PROPS_SHARD {
                    return Err(StorageError::deserialize_error(format!(
                        "unexpected section id in props shard: expected {:#06x}, got {:#06x}",
                        crate::persistence::section::EDGE_PROPS_SHARD,
                        sid
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
                    "unexpected trailing data in props shard".to_string(),
                ));
            }
            let mut shard = crate::edge::CsrWithProperties::new(prop_schemas.clone());
            shard.load(&data)?;
            for column in shard
                .property_schema()
                .iter()
                .map(|s| s.name.clone())
                .collect::<Vec<_>>()
            {
                if let Some(enc) = shard.column_encoding_type(&column) {
                    if enc != crate::encoding::EncodingType::None {
                        encodings.entry(column.clone()).or_insert(enc);
                    }
                }
                if let Some(id) = shard.prop_id_of(&column) {
                    prop_ids.entry(column).or_insert(id);
                }
            }
            for edge_id in shard.edge_ids().collect::<Vec<_>>() {
                if let Some((create_ts, delete_ts, values)) = shard.export_row(edge_id) {
                    // Cross-shard repeats must agree byte for byte: a repeat
                    // with different stamps or values is damage and fails the
                    // load instead of silently letting the first shard win.
                    if let Some((prev_create, prev_delete, prev_values)) =
                        self.properties.export_row(edge_id)
                    {
                        if prev_create != create_ts
                            || prev_delete != delete_ts
                            || prev_values != values
                        {
                            return Err(StorageError::deserialize_error(format!(
                                "duplicate property shard entry for edge {:?}",
                                edge_id
                            )));
                        }
                        continue;
                    }
                    self.properties
                        .import_row(edge_id, create_ts, delete_ts, &values)?;
                    self.edge_owner.or_insert(edge_id, *gid);
                }
            }
        }
        for (column, enc) in encodings {
            if self.properties.has_property(&column) {
                let _ = self.properties.apply_encoding_to_column(&column, enc, 255);
            }
        }
        self.properties.restore_prop_ids(&prop_ids);
        self.properties.refresh_column_stats();
        self.properties.clear_dirty_columns();
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
    use std::io::Write as _;

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
        let props = dir.path().join(props_group_file(0));
        assert!(props.exists());
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
        assert!(!table.out_csr.column_dirty_group_ids().is_empty());
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
    fn torn_manifest_tail_recovers_new_snapshot() {
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
        // publish: the manifest file carries stale state while the metadata
        // tail already describes the durable groups. The load recovers the
        // new snapshot via the tail instead of mixing or rejecting.
        let manifest_path = dir.path().join(GROUPS_MANIFEST_FILE);
        let bytes = std::fs::read(&manifest_path).expect("manifest readable");
        let mut manifest = TableShardManifest::decode(&bytes).expect("manifest decodes");
        manifest.out_groups.push(9999);
        crate::compression::write_shadow_file(&manifest_path, &manifest.encode())
            .expect("torn manifest writable");

        let mut loaded = make_table();
        loaded
            .load(dir.path())
            .expect("torn commit recovers via tail");
        assert!(loaded.has_edge(0, 1, 0, 200));
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
    fn crash_before_manifest_publish_recovers_new_snapshot() {
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
        let old_manifest =
            std::fs::read(dir.path().join(GROUPS_MANIFEST_FILE)).expect("old manifest readable");

        table
            .insert_edge(
                5000,
                6000,
                0,
                &[("weight".to_string(), Value::Double(2.0))],
                110,
            )
            .unwrap();
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("second flush should succeed");

        // Crash between the second metadata write and its manifest publish:
        // restore the old manifest file while the new metadata tail stays.
        // Loading recovers the new snapshot via the tail with topology,
        // properties and timestamps consistent.
        std::fs::write(dir.path().join(GROUPS_MANIFEST_FILE), &old_manifest)
            .expect("manifest restore works");
        let mut loaded = make_table();
        loaded
            .load(dir.path())
            .expect("torn second commit recovers new snapshot");
        assert!(loaded.has_edge(0, 1, 0, 200));
        assert!(loaded.has_edge(5000, 6000, 0, 200));

        // A clean retry of the whole flush from live memory still publishes
        // both files and stays loadable.
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
            table.insert_edge(i, i + 1000, 0, &[], 100).unwrap();
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
        let (mut raw, _) = persistence::read_pages_from_file(&sidecar).expect("sidecar readable");
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
    fn pre_v4_manifest_is_rejected() {
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
        // Hand-craft a version 3 manifest: same body, old version.
        let mut legacy = 3u32.to_le_bytes().to_vec();
        let current =
            std::fs::read(dir.path().join(GROUPS_MANIFEST_FILE)).expect("manifest readable");
        legacy.extend_from_slice(&current[4..]);
        std::fs::write(dir.path().join(GROUPS_MANIFEST_FILE), &legacy)
            .expect("legacy manifest writable");

        let mut loaded = make_table();
        let err = loaded
            .load(dir.path())
            .expect_err("pre-v4 manifest must be rejected");
        assert!(err
            .to_string()
            .contains("unsupported group manifest version"));
    }

    #[test]
    fn legacy_global_properties_file_is_rejected() {
        let mut table = make_table();
        table.insert_edge(0, 1, 0, &[], 100).unwrap();
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("flush should succeed");
        std::fs::write(dir.path().join(LEGACY_PROPERTIES_FILE), b"legacy")
            .expect("legacy props writable");
        let mut loaded = make_table();
        let err = loaded
            .load(dir.path())
            .expect_err("legacy properties must be rejected");
        assert!(err.to_string().contains("legacy global properties"));
    }

    #[test]
    fn legacy_meta_v2_is_rejected() {
        let mut table = make_table();
        table.insert_edge(0, 1, 0, &[], 100).unwrap();
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("flush should succeed");
        let meta_path = dir.path().join("meta.bin");
        let (mut payload, _) =
            persistence::read_pages_from_file(&meta_path).expect("meta readable");
        let header_len = crate::persistence::HEADER_SIZE;
        payload[header_len..header_len + 4].copy_from_slice(&2u32.to_le_bytes());
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
            .expect_err("legacy meta v2 must be rejected");
        assert!(err.to_string().contains("version 2"));
    }

    #[test]
    fn sparse_endpoints_produce_no_hole_files() {
        let mut table = make_table();
        table.insert_edge(0, 1, 0, &[], 100).unwrap();
        table.insert_edge(9000, 9001, 0, &[], 100).unwrap();
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("flush should succeed");
        assert!(dir.path().join(out_group_file(0)).exists());
        assert!(dir.path().join(out_group_file(2)).exists());
        assert!(!dir.path().join(out_group_file(1)).exists());
        assert!(!dir.path().join(ts_group_file(1)).exists());
        assert!(!dir.path().join(props_group_file(1)).exists());
        assert!(dir.path().join(ts_group_file(0)).exists());
        assert!(dir.path().join(ts_group_file(2)).exists());
        assert!(dir.path().join(props_group_file(0)).exists());
        assert!(dir.path().join(props_group_file(2)).exists());

        let mut loaded = make_table();
        loaded.load(dir.path()).expect("load should succeed");
        assert!(loaded.has_edge(0, 1, 0, 200));
        assert!(loaded.has_edge(9000, 9001, 0, 200));
        assert!(loaded.out_edges(5000, 200).is_empty());
        assert_eq!(loaded.edge_count(), 2);
    }

    #[test]
    fn small_timestamp_write_stays_proportional_to_dirty_owners() {
        let mut table = make_table();
        for i in 0..100u32 {
            table.insert_edge(i, i + 1000, 0, &[], 100).unwrap();
        }
        for i in 5000..5100u32 {
            table.insert_edge(i, i + 1000, 0, &[], 100).unwrap();
        }
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("baseline flush should succeed");
        let baseline_ts: u64 = dir
            .path()
            .read_dir()
            .expect("read dir")
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("ts_g"))
            .map(|entry| entry.metadata().map(|meta| meta.len()).unwrap_or(0))
            .sum();
        assert!(baseline_ts > 0);

        table.insert_edge(0, 2001, 0, &[], 110).unwrap();
        table.insert_edge(1, 2002, 0, &[], 110).unwrap();
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("small flush should succeed");
        let small_ts = std::fs::metadata(dir.path().join(ts_group_file(0)))
            .expect("dirty ts shard readable")
            .len();
        assert!(
            (small_ts as f64) < (baseline_ts as f64),
            "dirty ts shard {} must stay below baseline total {}",
            small_ts,
            baseline_ts
        );
        let mut loaded = make_table();
        loaded.load(dir.path()).expect("load should succeed");
        assert!(loaded.has_edge(0, 2001, 0, 200));
        assert_eq!(
            loaded.mvcc.creation_ts_of(graphdb_core::types::EdgeId(200)),
            Some(110)
        );
    }

    #[test]
    fn small_property_write_stays_proportional_to_dirty_owners() {
        let mut table = make_table();
        for i in 0..100u32 {
            table
                .insert_edge(
                    i,
                    i + 1000,
                    0,
                    &[("weight".to_string(), Value::Double(1.0))],
                    100,
                )
                .unwrap();
        }
        for i in 5000..5100u32 {
            table
                .insert_edge(
                    i,
                    i + 1000,
                    0,
                    &[("weight".to_string(), Value::Double(2.0))],
                    100,
                )
                .unwrap();
        }
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("baseline flush should succeed");
        let baseline_props: u64 = dir
            .path()
            .read_dir()
            .expect("read dir")
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("props_g"))
            .map(|entry| entry.metadata().map(|meta| meta.len()).unwrap_or(0))
            .sum();
        assert!(baseline_props > 0);

        table
            .update_edge_property(0, 1000, 0, "weight", &Value::Double(9.0), 200)
            .expect("property update should succeed");
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("small flush should succeed");
        let small_props = std::fs::metadata(dir.path().join(props_group_file(0)))
            .expect("dirty props shard readable")
            .len();
        assert!(
            (small_props as f64) < (baseline_props as f64),
            "dirty props shard {} must stay below baseline total {}",
            small_props,
            baseline_props
        );
        let mut loaded = make_table();
        loaded.load(dir.path()).expect("load should succeed");
        let record = loaded.get_edge(0, 1000, 0, 200).expect("edge survives");
        assert!(record
            .properties
            .iter()
            .any(|(k, v)| k == "weight" && *v == Value::Double(9.0)));
    }

    #[test]
    fn wal_recovers_committed_unflushed_writes_idempotently() {
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
            .expect("baseline flush should succeed");

        // Committed after the checkpoint, never flushed: redo log owns them.
        table
            .insert_edge(2, 3, 0, &[("weight".to_string(), Value::Double(2.0))], 200)
            .unwrap();
        assert!(table.delete_edge(0, 1, 0, 210).unwrap());
        drop(table);

        let mut recovered = make_table();
        recovered.load(dir.path()).expect("load replays the log");
        assert!(!recovered.has_edge(0, 1, 0, 300));
        assert!(recovered.has_edge(2, 3, 0, 300));

        // Log survives until the next checkpoint: a repeated replay of the
        // same ops (insert then delete) must land in the identical state.
        let mut second = make_table();
        second.load(dir.path()).expect("second load replays again");
        assert_eq!(second.edge_count(), recovered.edge_count());
        assert!(!second.has_edge(0, 1, 0, 300));
        assert!(second.has_edge(2, 3, 0, 300));
        let (mappings, rows, live_orphans) = second.copy_audit();
        assert_eq!((mappings, rows, live_orphans), (0, 0, 0));

        // Checkpoint truncates the log: afterwards recovery needs no replay.
        second
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .unwrap();
        assert!(!crate::edge::edge_table::wal::wal_path(dir.path()).exists());
    }

    #[test]
    fn torn_edge_wal_tail_rejects_load() {
        let mut table = make_table();
        table.insert_edge(0, 1, 0, &[], 100).unwrap();
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .unwrap();
        table.insert_edge(2, 3, 0, &[], 200).unwrap();
        // Simulate a torn tail: an entry header claiming more bytes than the
        // file holds after the last durable commit.
        std::fs::OpenOptions::new()
            .append(true)
            .open(crate::edge::edge_table::wal::wal_path(dir.path()))
            .expect("wal exists after the second commit")
            .write_all(&64u64.to_le_bytes())
            .expect("append torn header");
        drop(table);

        let mut recovered = make_table();
        let err = recovered
            .load(dir.path())
            .expect_err("torn edge WAL tail must fail closed");
        assert!(err.to_string().contains("edge WAL"));
    }

    #[test]
    fn reshard_roundtrip_preserves_snapshot() {
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
            .expect("baseline flush should succeed");
        let stats = table.reshard(9).expect("reshard should succeed");
        assert_eq!(stats.old_bits, 12);
        assert_eq!(stats.new_bits, 9);
        assert_eq!(stats.edges, 2);
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("post-reshard flush should succeed");

        let mut loaded = EdgeStore::with_config(
            crate::edge::EdgeSchema {
                label_id: 0,
                label_name: "knows".to_string(),
                src_label: 0,
                dst_label: 0,
                properties: vec![crate::types::StoragePropertyDef {
                    name: "weight".to_string(),
                    data_type: graphdb_core::types::DataType::Double,
                    nullable: false,
                    default_value: Some(Value::Double(0.0)),
                }],
                oe_strategy: crate::edge::EdgeStrategy::Multiple,
                ie_strategy: crate::edge::EdgeStrategy::Multiple,
                schema_version: 1,
            },
            EdgeTableConfig {
                node_group_bits: 9,
                ..EdgeTableConfig::default()
            },
        )
        .expect("table builds");
        loaded.load(dir.path()).expect("load should succeed");
        assert!(loaded.has_edge(0, 1, 0, 200));
        assert!(loaded.has_edge(5000, 6000, 0, 200));
        let record = loaded.get_edge(0, 1, 0, 200).expect("edge survives");
        assert!(record
            .properties
            .iter()
            .any(|(k, v)| k == "weight" && *v == Value::Double(1.0)));
        assert_eq!(loaded.edge_count(), 2);
    }

    fn make_bounded_table(bound: usize) -> EdgeStore {
        let schema = EdgeSchema {
            label_id: 0,
            label_name: "knows".to_string(),
            src_label: 0,
            dst_label: 0,
            properties: vec![],
            oe_strategy: EdgeStrategy::Multiple,
            ie_strategy: EdgeStrategy::Multiple,
            schema_version: 1,
        };
        let config = EdgeTableConfig {
            max_append_ops_per_group: bound,
            ..EdgeTableConfig::default()
        };
        EdgeStore::with_config(schema, config).unwrap()
    }

    #[test]
    fn append_bound_forces_base_merge() {
        let mut table = make_bounded_table(4);
        table.insert_edge(0, 1, 0, &[], 100).unwrap();
        table.insert_edge(0, 2, 0, &[], 100).unwrap();
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("baseline flush should succeed");
        for i in 3..8u32 {
            table.insert_edge(0, i, 0, &[], 110).unwrap();
        }
        let before = table.edge_count();
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("bound flush should succeed");
        assert!(dir.path().join(out_group_file(0)).exists());
        assert!(!dir.path().join(out_append_file(0)).exists());
        assert_eq!(table.edge_count(), before);
        let mut loaded = make_bounded_table(4);
        loaded.load(dir.path()).expect("load should succeed");
        assert_eq!(loaded.edge_count(), before);
        assert!(loaded.has_edge(0, 1, 0, 200));
        assert!(loaded.has_edge(0, 7, 0, 200));
    }

    #[test]
    fn under_bound_stays_sidecar() {
        let mut table = make_bounded_table(16);
        table.insert_edge(0, 1, 0, &[], 100).unwrap();
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("baseline flush should succeed");
        let base_path = dir.path().join(out_group_file(0));
        assert!(base_path.exists());
        let stamp = base_path.metadata().unwrap().modified().unwrap();
        table.insert_edge(0, 2, 0, &[], 110).unwrap();
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("small flush should succeed");
        assert!(dir.path().join(out_append_file(0)).exists());
        assert_eq!(base_path.metadata().unwrap().modified().unwrap(), stamp);
        let mut loaded = make_bounded_table(16);
        loaded.load(dir.path()).expect("load should succeed");
        assert_eq!(loaded.edge_count(), 2);
    }
}
