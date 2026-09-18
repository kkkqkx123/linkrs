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
        // Authority records share the tombstone per-record estimate so live
        // and deleted entries use one caliber including hash overhead.
        total +=
            super::super::stats::TombstoneStats::estimate_memory(self.mvcc.edge_timestamps.len());
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

        maintenance_ran
    }

    /// Background variant: bound comes from one per-pass watermark capture
    /// shared across all tables. Also reclaims authority tombstones whose
    /// physical rows are gone in both directions, so the authority map stays
    /// proportional to live edges rather than historical totals.
    pub fn maybe_run_auto_maintenance_with_watermarks(
        &mut self,
        watermarks: &graphdb_transaction::MvccWatermarks,
        margin: Timestamp,
    ) -> usize {
        let bound = watermarks.safe_gc_timestamp_with_margin(margin);
        let mut ran = self.run_auto_maintenance_pass(bound);
        if self.reclaim_authority_with_watermarks(watermarks, margin) > 0 {
            ran += 1;
        }
        ran
    }

    // ── Edge Property Index ──
}
