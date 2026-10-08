//! Resource accounting, backpressure and background upkeep.

use super::super::super::{CsrBase, CsrShardSet, MutableCsrTrait};
use super::EdgeStore;
use crate::edge::VertexFragmentation;
use linkrs_core::types::Timestamp;

/// Per-direction read counts for direction-narrowing audits.
///
/// Memory-only observability (never checkpointed): out-leg and in-leg
/// adjacency and point reads increment their own counter. A `Both` table
/// serving traffic on only one leg pays double topology writes and double
/// topology storage for an unused leg.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DirectionUsageSnapshot {
    /// Out-leg reads served since open.
    pub out_reads: u64,
    /// In-leg reads served since open.
    pub in_reads: u64,
}

impl DirectionUsageSnapshot {
    /// Total directional reads observed.
    pub fn total(&self) -> u64 {
        self.out_reads.saturating_add(self.in_reads)
    }

    /// Share of reads served by the incoming leg.
    pub fn in_share(&self) -> Option<f64> {
        let total = self.total();
        if total == 0 {
            None
        } else {
            Some(self.in_reads as f64 / total as f64)
        }
    }
}

/// Recommended narrowing of a dual-direction table to one leg.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectionNarrowingSuggestion {
    /// Direction the table holds now (`Both`).
    pub current: crate::edge::StorageDirection,
    /// Single direction to keep.
    pub target: crate::edge::StorageDirection,
    /// Decision basis with observed counts and the write-amplification saving.
    pub basis: String,
}

/// Outcome of one storage-direction migration.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DirectionMigrationStats {
    /// Topology edge slots dropped with the removed leg.
    pub edges_dropped: u64,
    /// Groups the removed leg held before the drop.
    pub groups_dropped: usize,
}

/// Per-component storage bytes of one edge table. See
/// `EdgeStore::storage_breakdown`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EdgeStorageBreakdown {
    /// Outgoing-leg topology bytes.
    pub out_bytes: usize,
    /// Incoming-leg topology bytes. Zero for `OutOnly` tables.
    pub in_bytes: usize,
    /// MVCC timestamp authority bytes (single copy).
    pub authority_bytes: usize,
    /// Owner-map bytes (single copy).
    pub owner_bytes: usize,
    /// Columnar property bytes, or the schema stub for inline forms.
    pub property_bytes: usize,
    /// Secondary index bytes plus the property-name cache.
    pub index_bytes: usize,
    /// Sum of every component above.
    pub total_bytes: usize,
}

impl EdgeStore {
    /// Per-vertex fragmentation view combining both directions.
    ///
    /// Observation only; the collection trigger consults `vertex_census`
    /// and `reclaimable_count` directly.
    pub fn vertex_fragmentation(&self, vid: u32, cutoff: Timestamp) -> VertexFragmentation {
        let (out_live, out_dead, out_cap) = self.out_csr.vertex_census(vid);
        let (in_live, in_dead, in_cap) = self.in_csr.vertex_census(vid);
        VertexFragmentation {
            vertex: vid,
            live_edges: out_live + in_live,
            dead_entries: out_dead + in_dead,
            capacity: out_cap + in_cap,
            reclaimable: self.out_csr.reclaimable_count(vid, cutoff)
                + self.in_csr.reclaimable_count(vid, cutoff),
        }
    }

    /// Per-component storage breakdown for capacity planning.
    ///
    /// Splits `used_memory_size` into its legs so operators can see the
    /// `Both` topology doubling against the single-copy authority, property
    /// and index shares before choosing a single direction or the bundled
    /// inline form. Sums only, never walks rows; the total equals
    /// `used_memory_size`.
    pub fn storage_breakdown(&self) -> EdgeStorageBreakdown {
        let out_bytes = self.out_csr.used_memory_size();
        let in_bytes = self.in_csr.used_memory_size();
        let authority_bytes = self.mvcc.edge_timestamps.memory_bytes();
        let owner_bytes = self.edge_owner.memory_bytes();
        let property_bytes = self.properties.used_memory_size();
        let cache_bytes = self.property_index_cache.len()
            * (std::mem::size_of::<String>() + std::mem::size_of::<usize>());
        let index_bytes = self
            .property_index
            .as_ref()
            .map(|index| index.memory_usage() as usize)
            .unwrap_or(0)
            + cache_bytes;
        EdgeStorageBreakdown {
            out_bytes,
            in_bytes,
            authority_bytes,
            owner_bytes,
            property_bytes,
            index_bytes,
            total_bytes: out_bytes
                + in_bytes
                + authority_bytes
                + owner_bytes
                + property_bytes
                + index_bytes,
        }
    }

    /// Topology write amplification of the stored direction.
    ///
    /// `Both` pays double topology writes and double topology storage to
    /// serve forward and reverse traversal from local rows; single-direction
    /// tables pay single. The authority, property and index shares stay
    /// single-copy either way. Prefer `OutOnly`/`InOnly` at creation when
    /// the missing direction is never traversed.
    pub fn topology_write_amplification(&self) -> u32 {
        match self.schema.storage_direction() {
            crate::edge::StorageDirection::Both => 2,
            crate::edge::StorageDirection::OutOnly | crate::edge::StorageDirection::InOnly => 1,
        }
    }

    /// Bytes per live edge split into topology, authority, property mapping
    /// and row bookkeeping. Sums only; the denominator is live edges so an
    /// empty table reports zeros instead of dividing by zero.
    pub fn bytes_per_edge_breakdown(&self) -> (f64, f64, f64, f64) {
        let live = self.out_csr.edge_count().max(1) as f64;
        let topo = (self.out_csr.used_memory_size() + self.in_csr.used_memory_size()) as f64 / live;
        let authority = self.mvcc.edge_timestamps.memory_bytes() as f64 / live;
        let (mapping, rows) = self.properties.mapping_row_bytes();
        (topo, authority, mapping as f64 / live, rows as f64 / live)
    }

    /// Suggest narrowing a dual-direction table to one leg.
    ///
    /// Usage governance (no engine change): when a `Both` table has served at
    /// least `min_observations` directional reads and every one of them hit
    /// the same leg, the other leg burns commit and storage bandwidth without
    /// serving any traversal. Returns the single direction to keep with the
    /// observed counts as the basis, or `None` when both legs serve traffic,
    /// the table is already single-direction, or observations are too few to
    /// decide. Counters are memory-only, so a freshly loaded table reports
    /// zero until it serves traffic again.
    pub fn suggest_direction_narrowing(
        &self,
        min_observations: u64,
    ) -> Option<DirectionNarrowingSuggestion> {
        use crate::edge::StorageDirection;
        let current = self.schema.storage_direction();
        if current != StorageDirection::Both {
            return None;
        }
        let snapshot = self.direction_usage_snapshot();
        if snapshot.total() < min_observations {
            return None;
        }
        let basis = format!(
            "direction usage out_reads={} in_reads={} (min_observations={}); Both pays 2x topology writes, single direction pays 1x",
            snapshot.out_reads, snapshot.in_reads, min_observations,
        );
        if snapshot.in_reads == 0 && snapshot.out_reads > 0 {
            return Some(DirectionNarrowingSuggestion {
                current,
                target: StorageDirection::OutOnly,
                basis,
            });
        }
        if snapshot.out_reads == 0 && snapshot.in_reads > 0 {
            return Some(DirectionNarrowingSuggestion {
                current,
                target: StorageDirection::InOnly,
                basis,
            });
        }
        None
    }

    /// Migrate a dual-direction table to one stored leg.
    ///
    /// Usage-layer schema migration (no engine change): `Both` to `OutOnly`
    /// drops the incoming leg, `Both` to `InOnly` drops the outgoing leg.
    /// The remaining leg keeps every logical edge; the dropped leg answered
    /// only reverse traversals, which read as empty afterwards (see
    /// `is_direction_available` and `direction_note`). WAL redo stays
    /// logical and replays onto the remaining leg, so unlike the record-form
    /// switch no WAL fence or mandatory checkpoint gate applies; checkpoint
    /// after the migration to reclaim the dropped leg storage. Widening
    /// (single to `Both`) and single-to-single repurposing are rejected:
    /// the dropped leg cannot be rebuilt from nothing here, reimport or
    /// reload the table instead.
    pub fn migrate_storage_direction(
        &mut self,
        target: crate::edge::StorageDirection,
    ) -> linkrs_core::StorageResult<DirectionMigrationStats> {
        use crate::edge::{EdgeStrategy, StorageDirection};
        if !self.is_open {
            return Err(linkrs_core::StorageError::storage_not_open());
        }
        let current = self.schema.storage_direction();
        if current == target {
            return Ok(DirectionMigrationStats {
                edges_dropped: 0,
                groups_dropped: 0,
            });
        }
        let (drop_out, drop_in) = match (current, target) {
            (StorageDirection::Both, StorageDirection::OutOnly) => (false, true),
            (StorageDirection::Both, StorageDirection::InOnly) => (true, false),
            _ => {
                return Err(linkrs_core::StorageError::invalid_operation(format!(
                    "storage-direction migration supports narrowing Both to OutOnly/InOnly only (current {:?}, target {:?}); widening needs a reimport, see storage_direction/is_direction_available",
                    current, target,
                )));
            }
        };
        if self.pending_add_column.is_some()
            || self.pending_drop_column.is_some()
            || self.pending_rename_column.is_some()
        {
            return Err(linkrs_core::StorageError::invalid_operation(
                "storage-direction migration rejects a pending schema change".to_string(),
            ));
        }
        let record_form = self.schema.record_form;
        let node_group_bits = self.config.node_group_bits;
        let overflow_chunk_edges = self.config.overflow_chunk_edges;
        let mut stats = DirectionMigrationStats {
            edges_dropped: 0,
            groups_dropped: 0,
        };
        if drop_in {
            stats.edges_dropped = self.in_csr.edge_count();
            stats.groups_dropped = self.in_csr.group_count();
            self.schema.ie_strategy = EdgeStrategy::None;
            self.schema.validate_resolved()?;
            self.in_csr = CsrShardSet::new(
                EdgeStrategy::None,
                node_group_bits,
                overflow_chunk_edges,
                record_form,
            )?;
        }
        if drop_out {
            stats.edges_dropped = self.out_csr.edge_count();
            stats.groups_dropped = self.out_csr.group_count();
            self.schema.oe_strategy = EdgeStrategy::None;
            self.schema.validate_resolved()?;
            self.out_csr = CsrShardSet::new(
                EdgeStrategy::None,
                node_group_bits,
                overflow_chunk_edges,
                record_form,
            )?;
        }
        self.segment_stats.clear();
        self.property_column_dirt.clear();
        self.mark_properties_dirty();
        self.out_csr.mark_all_dirty();
        self.in_csr.mark_all_dirty();
        log::info!(
            "edge table '{}' narrowed storage direction {:?} to {:?}: dropped {} edges in {} groups; reverse reads on the dropped leg are empty",
            self.label_name,
            current,
            target,
            stats.edges_dropped,
            stats.groups_dropped,
        );
        Ok(stats)
    }

    pub fn memory_size(&self) -> usize {
        self.used_memory_size()
    }

    pub fn used_memory_size(&self) -> usize {
        let mut total = 0;

        total += self.out_csr.used_memory_size();
        total += self.in_csr.used_memory_size();
        // Sparse segment memory for the visibility authority and the owner
        // map: untouched 1024-id segments stay unallocated, so wide sparse
        // id ranges cost only their pointer table.
        total += self.mvcc.edge_timestamps.memory_bytes();
        total += self.edge_owner.memory_bytes();
        total += self.properties.used_memory_size();

        // Account for property_index_cache
        total += self.property_index_cache.len()
            * (std::mem::size_of::<String>() + std::mem::size_of::<usize>());

        // Account for the edge property index (if enabled)
        if let Some(ref index) = self.property_index {
            total += index.memory_usage() as usize;
        }

        total
    }

    /// Estimate memory usage based on edge count and CSR strategy.
    ///
    /// Write-path fast path: counts plus per-shard sizes only, never a
    /// full-table fragmentation walk. A zero per-edge measurement reports a
    /// zero estimate explicitly; backpressure treats zero as no pressure and
    /// never divides by the estimate.
    pub fn estimate_memory_usage(&self) -> usize {
        let out_edges = self.out_csr.edge_count() as usize;
        let in_edges = self.in_csr.edge_count() as usize;
        let out_bytes_per_edge = self.out_csr.bytes_per_edge();
        let in_bytes_per_edge = self.in_csr.bytes_per_edge();
        out_edges * out_bytes_per_edge + in_edges * in_bytes_per_edge
    }

    /// Record mutable CSR pressure without performing maintenance on the write path.
    pub fn check_and_apply_write_backpressure(&mut self, _current_ts: Timestamp) -> bool {
        if self.config.max_mutable_csr_bytes == 0 {
            return false; // Backpressure disabled
        }

        let mutable_size = self.estimate_memory_usage();

        if mutable_size > self.config.max_mutable_csr_bytes {
            return true;
        }

        false
    }

    pub fn needs_background_maintenance(&self) -> bool {
        self.config.max_mutable_csr_bytes > 0
            && self.estimate_memory_usage() > self.config.max_mutable_csr_bytes
    }

    /// Write-path fast path: bound comes from the per-table pin cache, with
    /// no global watermark capture. Background passes use the watermark
    /// variant below.
    pub fn maybe_run_auto_maintenance(&mut self) -> usize {
        let bound = self.mvcc.min_active_snapshot_ts;
        self.run_auto_maintenance_pass(bound)
    }

    fn run_auto_maintenance_pass(&mut self, bound: Timestamp) -> usize {
        let cfg = self.config.auto_maintenance;
        let mut maintenance_ran = 0;

        // Reclaim edge-property before-images that no active snapshot can
        // observe. Version history is memory-only (checkpoints persist
        // current values), so reclamation never dirties the property file.
        if bound != Timestamp::MAX {
            let removed = self.properties.gc_property_versions(bound);
            if removed > 0 {
                maintenance_ran += 1;
            }
        }

        if cfg.property_compact_ratio > 0.0 && bound != Timestamp::MAX {
            let prop_stats = self.properties.compaction_stats();
            if prop_stats.fragmentation_ratio() >= cfg.property_compact_ratio as f64 {
                self.compact_properties(bound);
                maintenance_ran += 1;
            }
        }

        // Incremental row reclaim on the write path: only rows holding
        // entries eligible at this cutoff are visited, and each pass stops
        // after a bounded row count so small writes never cause large
        // rebuilds. Skipped entirely while no tombstone exists.
        if self.run_vertex_reclaim_pass(bound) {
            maintenance_ran += 1;
        }

        // Lag gauges refresh on every pass, even when no rebuild fires, so
        // idle tables keep observable staleness without a full audit walk.
        // The full drift audit stays on demand (`audit_and_report`): it
        // walks the table and never runs on a timer.
        self.emit_index_lag_metrics();
        // Staleness-driven secondary index rebuild: threshold or age trigger
        // from the table config, reusing the capacity recorded at the last
        // build. Queries fall back to segment scans while lagged, so a
        // rebuild only restores index serving without changing results.
        if self.property_index.is_some() {
            let threshold = cfg.index_rebuild_failure_threshold;
            let max_stale = cfg.index_max_stale_secs;
            match self.rebuild_index_if_needed(threshold, max_stale) {
                Ok(true) => maintenance_ran += 1,
                Ok(false) => {}
                Err(e) => {
                    log::debug!("automatic index rebuild skipped: {}", e);
                    self.maintenance_index_skips = self.maintenance_index_skips.saturating_add(1);
                }
            }
        }

        maintenance_ran
    }

    /// Maintenance skip counters for observability. All three retry on the
    /// next watermark-driven pass; growth without bound points at a stuck
    /// watermark or a permanently failing guard.
    pub fn maintenance_skip_counts(&self) -> (u64, u64, u64) {
        (
            self.maintenance_reclaim_skips,
            self.maintenance_migrate_skips,
            self.maintenance_index_skips,
        )
    }

    /// Background variant: bound comes from one per-pass watermark capture
    /// shared across all tables. Refreshes the hot-path reuse hint from the
    /// same fresh capture before running, then reclaims authority tombstones
    /// whose physical rows are gone in both directions, so the authority map
    /// stays proportional to live edges rather than historical totals. A
    /// disabled sentinel clears reuse instead of running on an expired bound.
    pub fn maybe_run_auto_maintenance_with_watermarks(
        &mut self,
        watermarks: &linkrs_transaction::MvccWatermarks,
        margin: Timestamp,
    ) -> usize {
        let bound = watermarks.safe_gc_timestamp_with_margin(margin);
        if bound != Timestamp::MAX {
            self.out_csr.refresh_tombstone_reuse_cutoff(bound);
            self.in_csr.refresh_tombstone_reuse_cutoff(bound);
        } else {
            self.out_csr.clear_tombstone_reuse_cutoff();
            self.in_csr.clear_tombstone_reuse_cutoff();
        }
        let mut ran = self.run_auto_maintenance_pass(bound);
        match self.reclaim_authority_with_watermarks(watermarks, margin) {
            Ok(reclaimed) => {
                if reclaimed > 0 {
                    ran += 1;
                }
            }
            Err(e) => {
                log::warn!("authority reclaim refused on audit drift: {}", e);
                self.maintenance_reclaim_skips = self.maintenance_reclaim_skips.saturating_add(1);
            }
        }
        // Opt-in record-form migration (background only, never the write
        // path): narrow single-scalar tables move to the recommended inline
        // form. Guard failures leave the table untouched; a successful switch
        // arms the mandatory checkpoint, which the regular flush performs.
        if self.config.auto_migrate_record_form {
            match self.auto_migrate_record_form_if_beneficial() {
                Ok(Some(stats)) => {
                    log::info!(
                        "automatic record-form migration on '{}': {} edges moved",
                        self.label_name,
                        stats.edges_moved
                    );
                    ran += 1;
                }
                Ok(None) => {}
                Err(e) => {
                    log::debug!("automatic record-form migration skipped: {}", e);
                    self.maintenance_migrate_skips =
                        self.maintenance_migrate_skips.saturating_add(1);
                }
            }
        }
        ran
    }

    // ── Edge Property Index ──

    /// Density of one group on one leg: live edges over capacity.
    /// Observation only; the merge-scope selector owns the reclaim decision.
    /// Missing groups report full density so they never trigger compaction.
    pub fn group_density(&self, gid: usize, outgoing: bool) -> f32 {
        let csr = if outgoing {
            &self.out_csr
        } else {
            &self.in_csr
        };
        csr.group_stats(gid)
            .map(|stats| stats.density)
            .unwrap_or(1.0)
    }

    /// Partition insert keys by owner group so batch reservations and future
    /// parallel applies stay group-local. Pure routing helper: no state change.
    pub fn partition_inserts_by_owner(&self, srcs_dsts: &[(u32, u32)]) -> Vec<(u32, Vec<usize>)> {
        use std::collections::BTreeMap;
        let mut by_owner: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
        for (idx, (src, dst)) in srcs_dsts.iter().enumerate() {
            by_owner
                .entry(self.owner_gid_for(*src, *dst))
                .or_default()
                .push(idx);
        }
        by_owner.into_iter().collect()
    }

    /// Committed write count for one owner group, for hotspot observation.
    pub fn group_write_count(&self, gid: u32) -> u64 {
        self.group_write_counts.get(&gid).copied().unwrap_or(0)
    }

    /// Owner groups ordered by committed write volume, most-written first.
    ///
    /// Observation only for hotspot diagnosis and future parallel-apply
    /// scheduling; the write path stays table-serialized for correctness.
    /// `limit` of zero returns every tracked group.
    pub fn hot_groups(&self, limit: usize) -> Vec<(u32, u64)> {
        let mut groups: Vec<(u32, u64)> = self
            .group_write_counts
            .iter()
            .map(|(gid, count)| (*gid, *count))
            .collect();
        groups.sort_unstable_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        if limit > 0 && groups.len() > limit {
            groups.truncate(limit);
        }
        groups
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edge::edge_table::config::EdgeTableConfig;
    use crate::edge::{EdgeSchema, EdgeStrategy, RecordForm, StorageDirection};

    fn both_schema() -> EdgeSchema {
        EdgeSchema {
            label_id: 0,
            label_name: "link".into(),
            src_label: 0,
            dst_label: 0,
            properties: Vec::new(),
            oe_strategy: EdgeStrategy::Multiple,
            ie_strategy: EdgeStrategy::Multiple,
            schema_version: 1,
            record_form: RecordForm::Columnar,
        }
    }

    #[test]
    fn narrowing_needs_minimum_observations() {
        let table = EdgeStore::with_config(both_schema(), EdgeTableConfig::default())
            .expect("table builds");
        assert!(table.suggest_direction_narrowing(10).is_none());
    }

    #[test]
    fn out_only_traffic_suggests_out_only() {
        let table = EdgeStore::with_config(both_schema(), EdgeTableConfig::default())
            .expect("table builds");
        for _ in 0..5 {
            table.observe_direction_read(true, 1);
        }
        let suggestion = table
            .suggest_direction_narrowing(5)
            .expect("out-only traffic suggests");
        assert_eq!(suggestion.current, StorageDirection::Both);
        assert_eq!(suggestion.target, StorageDirection::OutOnly);
        assert!(suggestion.basis.contains("out_reads=5"));
    }

    #[test]
    fn mixed_traffic_suggests_nothing() {
        let table = EdgeStore::with_config(both_schema(), EdgeTableConfig::default())
            .expect("table builds");
        for _ in 0..5 {
            table.observe_direction_read(true, 1);
            table.observe_direction_read(false, 1);
        }
        assert!(table.suggest_direction_narrowing(5).is_none());
    }

    #[test]
    fn narrowing_drops_one_leg_and_serves_the_other() {
        let mut table = EdgeStore::with_config(both_schema(), EdgeTableConfig::default())
            .expect("table builds");
        table.insert_edge(0, 1, 0, &[], 100).expect("insert");
        table.insert_edge(0, 2, 0, &[], 100).expect("insert");
        assert_eq!(table.topology_write_amplification(), 2);
        let stats = table
            .migrate_storage_direction(StorageDirection::OutOnly)
            .expect("narrow to out");
        assert_eq!(stats.edges_dropped, 2);
        assert_eq!(table.storage_direction(), StorageDirection::OutOnly);
        assert_eq!(table.topology_write_amplification(), 1);
        assert_eq!(table.out_edges(0, 200).len(), 2);
        assert!(table.in_edges(1, 200).is_empty());
        assert!(table.direction_note(false).is_some());
        assert!(table.suggest_direction_narrowing(1).is_none());
    }

    #[test]
    fn widening_is_rejected() {
        let mut table = EdgeStore::with_config(both_schema(), EdgeTableConfig::default())
            .expect("table builds");
        table
            .migrate_storage_direction(StorageDirection::OutOnly)
            .expect("narrow");
        assert!(table
            .migrate_storage_direction(StorageDirection::Both)
            .is_err());
        assert_eq!(table.storage_direction(), StorageDirection::OutOnly);
    }

    #[test]
    fn same_direction_migration_is_noop_via_observe() {
        let mut table = EdgeStore::with_config(both_schema(), EdgeTableConfig::default())
            .expect("table builds");
        let stats = table
            .migrate_storage_direction(StorageDirection::Both)
            .expect("noop");
        assert_eq!(stats.edges_dropped, 0);
        assert_eq!(table.storage_direction(), StorageDirection::Both);
    }

    #[test]
    fn direct_observe_drives_suggestion() {
        let table = EdgeStore::with_config(both_schema(), EdgeTableConfig::default())
            .expect("table builds");
        for _ in 0..5 {
            table.observe_direction_read(true, 1);
            table.observe_direction_read(false, 1);
        }
        assert!(table.suggest_direction_narrowing(5).is_none());
    }
}

#[cfg(test)]
mod direction_migration_tests {
    use super::*;
    use crate::edge::edge_table::config::EdgeTableConfig;
    use crate::edge::{EdgeSchema, EdgeStrategy, RecordForm, StorageDirection};

    fn both_schema() -> EdgeSchema {
        EdgeSchema {
            label_id: 0,
            label_name: "link".into(),
            src_label: 0,
            dst_label: 0,
            properties: Vec::new(),
            oe_strategy: EdgeStrategy::Multiple,
            ie_strategy: EdgeStrategy::Multiple,
            schema_version: 1,
            record_form: RecordForm::Columnar,
        }
    }

    fn both_table() -> EdgeStore {
        EdgeStore::with_config(both_schema(), EdgeTableConfig::default())
            .expect("dual-direction table builds")
    }

    #[test]
    fn unused_reverse_leg_suggests_out_only() {
        let mut table = both_table();
        table.insert_edge(0, 1, 0, &[], 100).expect("insert");
        for _ in 0..5 {
            let _ = table.out_edges(0, 200);
        }
        let suggestion = table
            .suggest_direction_narrowing(3)
            .expect("unused in leg suggests narrowing");
        assert_eq!(suggestion.current, StorageDirection::Both);
        assert_eq!(suggestion.target, StorageDirection::OutOnly);
        assert!(suggestion.basis.contains("in_reads=0"));
    }

    #[test]
    fn used_both_legs_suggest_nothing() {
        let mut table = both_table();
        table.insert_edge(0, 1, 0, &[], 100).expect("insert");
        let _ = table.out_edges(0, 200);
        let _ = table.in_edges(1, 200);
        assert!(table.suggest_direction_narrowing(1).is_none());
    }

    #[test]
    fn too_few_observations_suggest_nothing() {
        let mut table = both_table();
        table.insert_edge(0, 1, 0, &[], 100).expect("insert");
        let _ = table.out_edges(0, 200);
        assert!(table.suggest_direction_narrowing(100).is_none());
    }

    #[test]
    fn narrowing_drops_one_leg_and_keeps_writes() {
        let mut table = both_table();
        table.insert_edge(0, 1, 0, &[], 100).expect("insert");
        table.insert_edge(0, 2, 0, &[], 100).expect("insert");
        let stats = table
            .migrate_storage_direction(StorageDirection::OutOnly)
            .expect("narrowing succeeds");
        assert_eq!(stats.edges_dropped, 2);
        assert_eq!(table.storage_direction(), StorageDirection::OutOnly);
        assert_eq!(table.topology_write_amplification(), 1);
        assert_eq!(table.out_edges(0, 200).len(), 2);
        assert!(table.in_edges(1, 200).is_empty());
        assert!(table.direction_note(false).is_some());
        table
            .insert_edge(0, 3, 0, &[], 150)
            .expect("writes continue on the kept leg");
        assert_eq!(table.out_edges(0, 200).len(), 3);
    }

    #[test]
    fn widening_is_rejected() {
        let mut table = both_table();
        table
            .migrate_storage_direction(StorageDirection::OutOnly)
            .expect("narrow first");
        assert!(table
            .migrate_storage_direction(StorageDirection::Both)
            .is_err());
        assert!(table
            .migrate_storage_direction(StorageDirection::InOnly)
            .is_err());
        assert_eq!(table.storage_direction(), StorageDirection::OutOnly);
    }

    #[test]
    fn inline_tables_reject_rank_up_front() {
        use crate::edge::{is_bundled_eligible, RecordFormPreference};
        use crate::types::StoragePropertyDef;
        use linkrs_core::types::DataType;
        use linkrs_core::Value;
        let schema = EdgeSchema {
            properties: vec![StoragePropertyDef {
                name: "weight".into(),
                data_type: DataType::Double,
                nullable: false,
                default_value: Some(Value::Double(0.0)),
            }],
            ..both_schema()
        };
        assert!(is_bundled_eligible(
            &schema.properties,
            schema.oe_strategy,
            schema.ie_strategy
        ));
        let mut table = EdgeStore::with_config(
            schema,
            EdgeTableConfig {
                record_form: RecordFormPreference::Bundled,
                ..Default::default()
            },
        )
        .expect("bundled table builds");
        assert!(table.can_accept_rank(0));
        assert!(!table.can_accept_rank(3));
        let err = table
            .insert_edge(0, 1, 3, &[("weight".into(), Value::Double(1.0))], 100)
            .expect_err("nonzero rank on bundled must fail");
        assert!(err.to_string().contains("columnar record form"));
    }

    #[test]
    fn gate_share_advice_prefers_batch_above_threshold() {
        use crate::engine::graph_storage::context::WriteGateStats;
        let before = WriteGateStats {
            acquisitions: 0,
            wait_nanos: 0,
        };
        let after = WriteGateStats {
            acquisitions: 100,
            wait_nanos: 800_000_000,
        };
        let share = after.share_since(&before, std::time::Duration::from_secs(1), 4);
        assert!((share - 0.20).abs() < 1e-9);
        assert!(WriteGateStats::prefer_batch_commits(share));
        let idle = WriteGateStats {
            acquisitions: 100,
            wait_nanos: 10_000_000,
        };
        let low = idle.share_since(&before, std::time::Duration::from_secs(1), 4);
        assert!(!WriteGateStats::prefer_batch_commits(low));
    }
}
