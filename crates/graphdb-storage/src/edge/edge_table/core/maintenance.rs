//! Resource accounting, backpressure and background upkeep.

use super::super::super::{CsrBase, MutableCsrTrait};
use super::EdgeStore;
use crate::edge::VertexFragmentation;
use graphdb_core::types::Timestamp;

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

    /// Get mutable CSR memory usage (out_csr + in_csr)
    pub fn mutable_csr_memory_size(&self) -> usize {
        self.out_csr.used_memory_size() + self.in_csr.used_memory_size()
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
                }
            }
        }

        maintenance_ran
    }

    /// Background variant: bound comes from one per-pass watermark capture
    /// shared across all tables. Refreshes the hot-path reuse hint from the
    /// same fresh capture before running, then reclaims authority tombstones
    /// whose physical rows are gone in both directions, so the authority map
    /// stays proportional to live edges rather than historical totals. A
    /// disabled sentinel clears reuse instead of running on an expired bound.
    pub fn maybe_run_auto_maintenance_with_watermarks(
        &mut self,
        watermarks: &graphdb_transaction::MvccWatermarks,
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

    /// Groups below `threshold` density on either stored leg, for observability
    /// and merge-scope selection. Clean groups report full density.
    pub fn sparse_groups_below(&self, threshold: f32) -> Vec<(bool, usize, f32)> {
        let mut sparse = Vec::new();
        if self.schema.has_out() {
            for gid in self.out_csr.existing_group_ids() {
                let density = self.group_density(gid, true);
                if density < threshold {
                    sparse.push((true, gid, density));
                }
            }
        }
        if self.schema.has_in() {
            for gid in self.in_csr.existing_group_ids() {
                let density = self.group_density(gid, false);
                if density < threshold {
                    sparse.push((false, gid, density));
                }
            }
        }
        sparse
    }

    /// Whether a group should compact at the configured merge thresholds.
    /// Multi-region spans use the group threshold, single-region spans use
    /// the region threshold; the decision never changes the live set.
    pub fn should_compact_group(&self, gid: usize, outgoing: bool) -> bool {
        let threshold = self.config.group_merge_min_density;
        self.group_density(gid, outgoing) < threshold
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

    /// Human-readable record-form guidance: locked choice plus evolution cost.
    /// Pure fits topology-only read-heavy types; Bundled fits one stable
    /// scalar; Columnar fits everything else. Breaking the preconditions
    /// needs an explicit migration followed by a mandatory checkpoint.
    pub fn record_form_guidance(&self) -> String {
        let form = self.schema.record_form;
        let properties = self.schema.properties.len();
        match form {
            crate::edge::RecordForm::Pure => format!(
                "table '{}' uses Pure topology (no properties, rank pinned to zero). \
                Adding properties or nonzero ranks needs migration to Bundled or Columnar \
                followed by a checkpoint.",
                self.label_name
            ),
            crate::edge::RecordForm::Bundled => format!(
                "table '{}' uses Bundled inline scalar (one property). \
                Adding a second property, using a non-encodable type, using nonzero ranks, \
                needing MVCC history, changing schema online, bulk importing ranked batches into a non-empty table, or freezing with valid values \
                needs migration to Columnar followed by a checkpoint.",
                self.label_name
            ),
            crate::edge::RecordForm::Columnar => format!(
                "table '{}' uses Columnar storage ({} properties). \
                Tombstone reuse applies to the multi-edge topology only; other forms ignore \
                the reuse cutoff. No migration needed for schema evolution.",
                self.label_name, properties
            ),
        }
    }
}
