//! Incremental flush orchestration and checkpoint metrics.

use super::super::core::EdgeStore;
use super::layout::{file_bytes, manifest_path};
use super::topology::GroupFlush;
use crate::edge::node_group::{EdgeCheckpointKind, TableShardManifest};
use crate::edge::CsrBase;
use graphdb_core::StorageResult;
use std::path::Path;

impl EdgeStore {
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
        flushed_bytes += self.flush_form_profile(dir, page_size, level)?;
        let (out_bytes, out_rebalanced) = self.flush_group_set(&GroupFlush {
            dir,
            page_size,
            level,
            outgoing: true,
            section_id: crate::persistence::section::EDGE_OUT_CSR,
            append_section_id: crate::persistence::section::EDGE_OUT_APPEND,
            manifest: &manifest,
        })?;
        flushed_bytes += out_bytes;
        let (in_bytes, in_rebalanced) = self.flush_group_set(&GroupFlush {
            dir,
            page_size,
            level,
            outgoing: false,
            section_id: crate::persistence::section::EDGE_IN_CSR,
            append_section_id: crate::persistence::section::EDGE_IN_APPEND,
            manifest: &manifest,
        })?;
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
        // A checkpoint re-establishes the base: any pre-checkpoint switch is
        // now durable and the mandatory-checkpoint fence lifts.
        self.migration_pending_checkpoint = false;
        let orphan_files = self.remove_orphan_group_files(dir);
        let encoding_saved: usize = self
            .topology_encoding_report()
            .iter()
            .map(|(_, _, plain, encoded)| plain.saturating_sub(*encoded))
            .sum();
        log::debug!(
            "EdgeTable[{}] checkpoint kind={:?} dirty_groups={} bytes={} live_edges={} encoding_saved={} orphan_files={}",
            self.label,
            kind,
            dirty_groups,
            flushed_bytes,
            self.out_csr.edge_count() + self.in_csr.edge_count(),
            encoding_saved,
            orphan_files
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
        let _ = super::super::wal::truncate(dir);
        Ok(kind)
    }
}
