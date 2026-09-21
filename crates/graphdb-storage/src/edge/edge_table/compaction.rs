//! Compaction: per-region CSR rebuilds and property slot reclamation.
//!
//! All cutoffs derive from one per-pass `MvccWatermarks` capture plus margin;
//! eligibility uses `Visibility::is_gc_eligible`. Reclaim passes skip groups
//! without delete history; flush-time rebuilds touch only dirty regions at a
//! density-tiered scope (row, region, group). Whole-table and whole-group
//! fragmentation ratios stay observation metrics; only group-scope merges
//! consult the caller fragmentation gate.

use super::core::EdgeStore;
use super::stats::DeletionStats;
use crate::edge::csr_trait::{CsrBase, MutableCsrTrait};
use graphdb_core::types::Timestamp;
use graphdb_core::StorageResult;

/// Upper bound of rows reclaimed in one write-path maintenance pass, so a
/// small write never triggers a large rebuild pause. Recommended range
/// 16..=64 from the reclaim pause versus throughput tradeoff; tune against
/// checkpoint flushed bytes per live edge, see `docs/plan/csr_baseline_notes.md`.
pub(crate) const MAX_VERTEX_RECLAIM_PER_PASS: usize = 32;

impl EdgeStore {
    /// Whether any group is fragmented enough to justify a rebuild.
    ///
    /// Scans each group with its own capacity/live ratio; the whole-table
    /// ratio stays an observation metric and never triggers collection.
    /// A fragmented clean group still triggers, so a dirty group cannot
    /// drag clean groups into the rebuild set.
    pub fn has_fragmented_group(&self, threshold: f32) -> bool {
        for gid in self.out_csr.existing_group_ids() {
            if self
                .out_csr
                .group_variant(gid)
                .is_some_and(|variant| variant.fragmentation_ratio() >= threshold)
            {
                return true;
            }
        }
        for gid in self.in_csr.existing_group_ids() {
            if self
                .in_csr
                .group_variant(gid)
                .is_some_and(|variant| variant.fragmentation_ratio() >= threshold)
            {
                return true;
            }
        }
        false
    }

    /// Compact every group, sharing one pass cutoff across all sub-systems.
    /// Deletions are promoted into the tombstone layer. Explicit maintenance
    /// only; the write path uses the bounded reclaim pass below.
    pub fn compact_csr_only_with_watermarks(
        &mut self,
        watermarks: &graphdb_transaction::MvccWatermarks,
        margin: Timestamp,
        reserve_ratio: f32,
    ) -> usize {
        let cutoff = watermarks.safe_gc_timestamp_with_margin(margin);
        // The explicit pass refreshes the reuse hint from its own fresh
        // capture; a disabled sentinel propagates as disabled.
        if cutoff != Timestamp::MAX {
            self.out_csr.refresh_tombstone_reuse_cutoff(cutoff);
            self.in_csr.refresh_tombstone_reuse_cutoff(cutoff);
        } else {
            self.out_csr.clear_tombstone_reuse_cutoff();
            self.in_csr.clear_tombstone_reuse_cutoff();
        }
        let mut removed_edges = std::collections::HashSet::new();
        for gid in self.out_csr.existing_group_ids() {
            self.out_csr.compact_group_with_reporting(
                gid,
                cutoff,
                reserve_ratio,
                &mut |edge_id, delete_ts| {
                    removed_edges.insert(edge_id);
                    self.mvcc.record_deletion(edge_id, delete_ts);
                },
            );
        }
        for gid in self.in_csr.existing_group_ids() {
            self.in_csr.compact_group_with_reporting(
                gid,
                cutoff,
                reserve_ratio,
                &mut |edge_id, delete_ts| {
                    removed_edges.insert(edge_id);
                    self.mvcc.record_deletion(edge_id, delete_ts);
                },
            );
        }
        let drift = self.audit_copy_drift();
        if !drift.is_empty() {
            log::warn!("compact_csr_only drift after rebuild: {}", drift.join("; "));
        }
        removed_edges.len()
    }

    /// Reclaim rows holding entries eligible at `bound`, visiting at most
    /// `max_vertices` rows.
    ///
    /// Groups without delete history are skipped without per-row
    /// inspection, so the pause stays proportional to the dirty groups
    /// rather than the size of the table. Each visited row pays a single
    /// fused dead/reclaimable probe instead of a census plus an eligibility
    /// walk.
    pub fn compact_reclaimable_vertices(&mut self, bound: Timestamp, max_vertices: usize) -> usize {
        if bound == Timestamp::MAX || max_vertices == 0 {
            return 0;
        }
        let mut gids: Vec<usize> = self.out_csr.existing_group_ids();
        for gid in self.in_csr.existing_group_ids() {
            if !gids.contains(&gid) {
                gids.push(gid);
            }
        }
        gids.sort_unstable();
        let group_bits = self.out_csr.group_bits();
        let mut removed_edges = std::collections::HashSet::new();
        let mut visited = 0usize;
        for gid in gids {
            if visited >= max_vertices {
                break;
            }
            let out_scan = self.out_csr.group_needs_reclaim_scan(gid);
            let in_scan = self.in_csr.group_needs_reclaim_scan(gid);
            if !out_scan && !in_scan {
                continue;
            }
            let rows = crate::edge::node_group::group_size(group_bits);
            let base = crate::edge::node_group::group_base(gid, group_bits);
            let mut group_done = true;
            let mut any_dead = false;
            for local in 0..rows {
                if visited >= max_vertices {
                    group_done = false;
                    break;
                }
                let vid = base.saturating_add(local as u32);
                let (out_dead, out_reclaimable) = if out_scan {
                    self.out_csr.vertex_reclaim_probe(vid, bound)
                } else {
                    (0, 0)
                };
                let (in_dead, in_reclaimable) = if in_scan {
                    self.in_csr.vertex_reclaim_probe(vid, bound)
                } else {
                    (0, 0)
                };
                any_dead |= out_dead + in_dead > 0;
                if out_reclaimable + in_reclaimable == 0 {
                    continue;
                }
                visited += 1;
                self.out_csr.compact_vertex_with_reporting(
                    vid,
                    bound,
                    &mut |edge_id, delete_ts| {
                        removed_edges.insert(edge_id);
                        self.mvcc.record_deletion(edge_id, delete_ts);
                    },
                );
                self.in_csr
                    .compact_vertex_with_reporting(vid, bound, &mut |edge_id, delete_ts| {
                        removed_edges.insert(edge_id);
                        self.mvcc.record_deletion(edge_id, delete_ts);
                    });
            }
            // Keep the hint while any tombstone physically remains, even when
            // the current bound covers nothing yet. Clearing early would drop
            // tail groups that a later watermark could still reclaim.
            if group_done && !any_dead {
                if out_scan {
                    self.out_csr.clear_reclaim_hint(gid);
                }
                if in_scan {
                    self.in_csr.clear_reclaim_hint(gid);
                }
            }
        }
        removed_edges.len()
    }

    /// Write-path incremental reclaim pass.
    ///
    /// Gated by tombstone count and watermark movement: below the count
    /// threshold with an unchanged watermark there is nothing newly
    /// reclaimable, so the commit skips the group scan entirely. A watermark
    /// advance or heap growth past the last pass re-arms it, and a full heap
    /// always scans. Insert-only workloads therefore pay no scan cost.
    ///
    /// This pass never refreshes the hot-path reuse hint: `bound` may come
    /// from the table-local pin cache, which can sit above the global safe
    /// point. Only watermark captures refresh the hint (see
    /// [`Self::maybe_run_auto_maintenance_with_watermarks`],
    /// [`Self::compact_csr_only_with_watermarks`] and
    /// [`Self::maybe_compact_for_flush_with_watermarks`]); otherwise a stale
    /// local bound would widen reuse past what reclaim has proven.
    pub(crate) fn run_vertex_reclaim_pass(&mut self, bound: Timestamp) -> bool {
        if bound == Timestamp::MAX {
            return false;
        }
        let threshold = self.config.auto_maintenance.reclaim_tombstone_threshold;
        let tombstones = self.mvcc.total_tombstone_count();
        if tombstones == 0
            || (tombstones < threshold
                && bound == self.last_reclaim_bound
                && tombstones <= self.last_reclaim_tombstones)
        {
            return false;
        }
        let reclaimed = self.compact_reclaimable_vertices(bound, MAX_VERTEX_RECLAIM_PER_PASS) > 0;
        self.last_reclaim_bound = bound;
        self.last_reclaim_tombstones = tombstones;
        reclaimed
    }

    /// Rebuild dense dirty regions before flush, sharing the pass cutoff.
    /// Clean regions are skipped, so a small dirty set never triggers a
    /// large rebuild pause.
    ///
    /// Scope is tiered per region: rows holding reclaimable entries are the
    /// only trigger, density only widens the scope from row to region to
    /// group, and larger scopes require higher density. Row gaps kept at the
    /// packed density target never trigger a rebuild on their own; only
    /// tombstone dirt does.
    pub fn maybe_compact_for_flush_with_watermarks(
        &mut self,
        watermarks: &graphdb_transaction::MvccWatermarks,
        margin: Timestamp,
        threshold: f32,
    ) {
        // Row-gap reserve derived from the packed-row density target, so
        // rebuilt rows keep everyday-write gaps instead of packing full.
        const RESERVE_RATIO: f32 = 1.0 - crate::edge::mutable_csr::PACKED_CSR_DENSITY;
        let cutoff = watermarks.safe_gc_timestamp_with_margin(margin);
        // The flush entry also refreshes the reuse hint from its own fresh
        // capture: flush-time compactions may run without a preceding
        // write-path reclaim pass. A disabled sentinel clears reuse instead
        // of running on an expired bound.
        if cutoff != Timestamp::MAX {
            self.out_csr.refresh_tombstone_reuse_cutoff(cutoff);
            self.in_csr.refresh_tombstone_reuse_cutoff(cutoff);
        } else {
            self.out_csr.clear_tombstone_reuse_cutoff();
            self.in_csr.clear_tombstone_reuse_cutoff();
        }
        // Group-scope merges stay behind the caller fragmentation gate so a
        // small flush never triggers an unbounded rebuild; row and region
        // scopes trigger on per-row reclaimable counts alone.
        let group_merge_allowed = |ratio: f32| ratio >= threshold;
        for outgoing in [true, false] {
            let existing: Vec<usize> = if outgoing {
                self.out_csr.existing_group_ids()
            } else {
                self.in_csr.existing_group_ids()
            };
            let group_size = if outgoing {
                self.out_csr.group_size()
            } else {
                self.in_csr.group_size()
            };
            let regions = crate::edge::node_group::regions_per_group(group_size);
            for gid in existing {
                for rid in 0..regions {
                    let shards = if outgoing {
                        &self.out_csr
                    } else {
                        &self.in_csr
                    };
                    let scope = shards.select_merge_scope(
                        gid,
                        rid,
                        cutoff,
                        self.config.region_merge_min_density,
                        self.config.group_merge_min_density,
                    );
                    match scope {
                        None => continue,
                        Some(crate::edge::node_group::RegionMergeScope::Group) => {
                            let ratio = if outgoing {
                                self.out_csr
                                    .group_variant(gid)
                                    .map(|variant| {
                                        variant
                                            .fragmentation_stats()
                                            .map_or(0.0, |stats| stats.fragmentation_ratio())
                                    })
                                    .unwrap_or(0.0)
                            } else {
                                self.in_csr
                                    .group_variant(gid)
                                    .map(|variant| {
                                        variant
                                            .fragmentation_stats()
                                            .map_or(0.0, |stats| stats.fragmentation_ratio())
                                    })
                                    .unwrap_or(0.0)
                            };
                            if !group_merge_allowed(ratio) {
                                let shards = if outgoing {
                                    &mut self.out_csr
                                } else {
                                    &mut self.in_csr
                                };
                                shards.compact_region_with_reporting(
                                    gid,
                                    rid,
                                    cutoff,
                                    &mut |edge_id, delete_ts| {
                                        self.mvcc.record_deletion(edge_id, delete_ts)
                                    },
                                );
                                continue;
                            }
                            let shards = if outgoing {
                                &mut self.out_csr
                            } else {
                                &mut self.in_csr
                            };
                            shards.compact_group_with_reporting(
                                gid,
                                cutoff,
                                RESERVE_RATIO,
                                &mut |edge_id, delete_ts| {
                                    self.mvcc.record_deletion(edge_id, delete_ts)
                                },
                            );
                            break;
                        }
                        Some(crate::edge::node_group::RegionMergeScope::Region) => {
                            let shards = if outgoing {
                                &mut self.out_csr
                            } else {
                                &mut self.in_csr
                            };
                            shards.compact_region_with_reporting(
                                gid,
                                rid,
                                cutoff,
                                &mut |edge_id, delete_ts| {
                                    self.mvcc.record_deletion(edge_id, delete_ts)
                                },
                            );
                        }
                        Some(crate::edge::node_group::RegionMergeScope::Row) => {
                            let (start, end) =
                                crate::edge::node_group::region_local_range(rid, group_size);
                            let group_bits = if outgoing {
                                self.out_csr.group_bits()
                            } else {
                                self.in_csr.group_bits()
                            };
                            let base = crate::edge::node_group::group_base(gid, group_bits);
                            for local in start..end {
                                let vid = base.saturating_add(local);
                                let needs = if outgoing {
                                    self.out_csr.vertex_needs_compact(vid, cutoff)
                                } else {
                                    self.in_csr.vertex_needs_compact(vid, cutoff)
                                };
                                if !needs {
                                    continue;
                                }
                                if outgoing {
                                    self.out_csr.compact_vertex_with_reporting(
                                        vid,
                                        cutoff,
                                        &mut |edge_id, delete_ts| {
                                            self.mvcc.record_deletion(edge_id, delete_ts)
                                        },
                                    );
                                } else {
                                    self.in_csr.compact_vertex_with_reporting(
                                        vid,
                                        cutoff,
                                        &mut |edge_id, delete_ts| {
                                            self.mvcc.record_deletion(edge_id, delete_ts)
                                        },
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// Compact properties by reclaiming slots of unreferenced dead records.
    ///
    /// Live rows keep their positions, so edge-to-row mappings never need
    /// remapping.
    pub fn compact_properties_with_watermarks(
        &mut self,
        watermarks: &graphdb_transaction::MvccWatermarks,
        margin: Timestamp,
    ) {
        let bound = watermarks.safe_gc_timestamp_with_margin(margin);
        self.compact_properties(bound);
    }

    /// Reclaim authority tombstones below a global watermark.
    ///
    /// Only entries whose topology rows are gone in both directions and whose
    /// property row is gone are removed, and only when the deletion timestamp
    /// is below the watermark-derived cutoff. The cutoff comes from the
    /// shared watermark capture, never from the table-local pin. A nonzero
    /// cross-copy audit fails closed so dangling references never lose
    /// their authority record.
    pub fn reclaim_authority_with_watermarks(
        &mut self,
        watermarks: &graphdb_transaction::MvccWatermarks,
        margin: Timestamp,
    ) -> StorageResult<usize> {
        use graphdb_core::StorageError;
        let cutoff = watermarks.safe_gc_timestamp_with_margin(margin);
        if cutoff == Timestamp::MAX {
            return Ok(0);
        }
        let (orphan_mappings, orphan_csr_rows, live_orphans) = self.copy_audit();
        if orphan_mappings + orphan_csr_rows + live_orphans > 0 {
            return Err(StorageError::data_corruption(format!(
                "reclaim_authority: audit nonzero (mappings={}, csr_rows={}, live_orphans={}), refusing reclaim",
                orphan_mappings, orphan_csr_rows, live_orphans
            )));
        }
        let mut live_topology = std::collections::HashSet::new();
        for (_, nbr) in self.out_csr.iter_all().chain(self.in_csr.iter_all()) {
            live_topology.insert(nbr.edge_id);
        }
        let properties = &self.properties;
        Ok(self.mvcc.reclaim_below(cutoff, |edge_id| {
            !live_topology.contains(&edge_id) && !properties.contains_edge(edge_id)
        }))
    }

    pub fn compact_properties(&mut self, bound: Timestamp) {
        let mut valid_edge_ids = std::collections::HashSet::new();
        for (edge_id, _pos) in self.properties.edge_mappings() {
            // Authoritative visibility, not the tombstone table alone: a
            // tombstone reclaimed by an earlier GC round must not resurrect
            // its edge as live here.
            if self.mvcc.is_edge_visible(edge_id, bound) {
                valid_edge_ids.insert(edge_id);
            }
        }

        let reclaimed = self.properties.reclaim_slots(&valid_edge_ids, bound);
        if reclaimed > 0 {
            self.mark_properties_dirty();
            log::debug!("Property slot reclaim recycled {} row(s)", reclaimed);
        }
    }

    /// Get deletion statistics for the sharded table.
    pub fn deletion_stats(&self) -> DeletionStats {
        let stats = self.mvcc.tombstone_stats();
        DeletionStats {
            total_live_edges: self.out_csr.edge_count(),
            total_deleted_edges: stats.count as u64,
        }
    }
}
