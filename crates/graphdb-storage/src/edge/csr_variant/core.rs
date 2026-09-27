use graphdb_core::StorageResult;

use super::super::CsrBase;
use super::super::{EdgeStrategy, FragmentationStats, MutableCsrTrait, Timestamp};
use super::CsrVariant;

impl CsrVariant {
    pub fn from_strategy_with_overflow(
        strategy: EdgeStrategy,
        vertex_capacity: usize,
        edge_capacity: usize,
        overflow_chunk_edges: usize,
    ) -> StorageResult<Self> {
        use super::super::{MutableCsr, SingleMutableCsr};
        match strategy {
            EdgeStrategy::Multiple => Ok(CsrVariant::Multiple(Box::new(
                MutableCsr::with_overflow_chunk_edges(
                    vertex_capacity,
                    edge_capacity,
                    overflow_chunk_edges,
                ),
            ))),
            EdgeStrategy::Single => Ok(CsrVariant::Single(SingleMutableCsr::with_capacity(
                vertex_capacity,
            ))),
            EdgeStrategy::None => Ok(CsrVariant::None { vertex_capacity }),
        }
    }

    /// Clear all edges.
    pub fn clear(&mut self) {
        match self {
            CsrVariant::Multiple(csr) => csr.clear(),
            CsrVariant::Single(csr) => csr.clear(),
            CsrVariant::Pure(csr) => csr.clear(),
            CsrVariant::Bundled(csr) => csr.clear(),
            CsrVariant::Frozen(csr) => csr.clear(),
            CsrVariant::None { .. } => {}
        }
    }

    /// Refresh the hot-path tombstone reuse cutoff. Only the multi-edge
    /// store reuses primary tombstones; single-slot rows overwrite in place
    /// already and hold no overflow, so other variants ignore the hint.
    ///
    /// Freshness follows the single contract on
    /// [`MutableCsr::set_tombstone_reuse_cutoff`](super::super::MutableCsr::set_tombstone_reuse_cutoff):
    /// only watermark-derived bounds refresh, the sentinel disables, and a
    /// stale value only narrows reuse.
    pub fn set_tombstone_reuse_cutoff(&mut self, cutoff: Timestamp) {
        if let CsrVariant::Multiple(csr) = self {
            csr.set_tombstone_reuse_cutoff(cutoff);
        }
    }

    /// Fresh watermark refresh allowing widening. Only call with a freshly
    /// captured global watermark bound.
    pub fn refresh_tombstone_reuse_cutoff(&mut self, fresh: Timestamp) {
        if let CsrVariant::Multiple(csr) = self {
            csr.refresh_tombstone_reuse_cutoff(fresh);
        }
    }

    /// Drop the reuse hint back to the disabled sentinel on every group.
    ///
    /// Stale path of the same contract: no fresh watermark means no reuse.
    pub fn clear_tombstone_reuse_cutoff(&mut self) {
        if let CsrVariant::Multiple(csr) = self {
            csr.clear_tombstone_reuse_cutoff();
        }
    }

    /// Current reuse cutoff for observability and tests. Non-multi-edge
    /// variants never reuse, so they report the disabled sentinel.
    pub fn tombstone_reuse_cutoff(&self) -> Timestamp {
        if let CsrVariant::Multiple(csr) = self {
            csr.tombstone_reuse_cutoff()
        } else {
            Timestamp::MAX
        }
    }

    /// Whether whole-table timestamp reclaim applies to this variant.
    ///
    /// Only forms carrying timestamps reclaim here. Pure and bundled rows
    /// hold no timestamps so their holes compact through the per-row entry
    /// instead. Production frozen reclaim uses the batched rows entry instead
    /// of one call per row.
    pub fn supports_timestamp_reclaim(&self) -> bool {
        matches!(
            self,
            CsrVariant::Multiple(_) | CsrVariant::Single(_) | CsrVariant::Frozen(_)
        )
    }

    /// Whether this variant holds reserved row capacity reported as fragmentation.
    ///
    /// Covers the multi-edge store plus the pure-topology and bundled forms
    /// sharing its primary-plus-overflow layout. Single slots carry no
    /// reserved gaps beyond their tombstone, frozen rows are packed without
    /// gaps, and the placeholder holds no edges.
    pub fn has_reserved_capacity(&self) -> bool {
        matches!(
            self,
            CsrVariant::Multiple(_) | CsrVariant::Pure(_) | CsrVariant::Bundled(_)
        )
    }

    /// Get fragmentation ratio for diagnostics
    ///
    /// Covers every variant holding reserved row capacity: the multi-edge
    /// store plus the pure-topology and bundled forms sharing its
    /// primary-plus-overflow layout. Single-slot, frozen and empty forms
    /// report zero: single slots carry no reserved gaps beyond their
    /// tombstone, frozen rows are packed without gaps, and the placeholder
    /// holds no edges.
    pub fn fragmentation_ratio(&self) -> f32 {
        if !self.has_reserved_capacity() {
            return 0.0;
        }
        match self {
            CsrVariant::Multiple(csr) => csr.fragmentation_ratio(),
            CsrVariant::Pure(csr) => csr.fragmentation_ratio(),
            CsrVariant::Bundled(csr) => csr.fragmentation_ratio(),
            _ => 0.0,
        }
    }

    /// Estimate wasted bytes due to fragmentation.
    ///
    /// Same coverage as the ratio above; other variants report zero.
    pub fn wasted_bytes_estimate(&self) -> usize {
        if !self.has_reserved_capacity() {
            return 0;
        }
        match self {
            CsrVariant::Multiple(csr) => csr.wasted_bytes_estimate(),
            CsrVariant::Pure(csr) => csr.wasted_bytes_estimate(),
            CsrVariant::Bundled(csr) => csr.wasted_bytes_estimate(),
            _ => 0,
        }
    }

    /// Get fragmentation statistics for this CSR variant.
    ///
    /// Returns `Some(stats)` for variants holding reserved row capacity,
    /// `None` for the remaining forms.
    pub fn fragmentation_stats(&self) -> Option<super::super::FragmentationStats> {
        if !self.has_reserved_capacity() {
            return None;
        }
        match self {
            CsrVariant::Multiple(csr) => {
                let stats = csr.get_fragmentation_stats();
                Some(FragmentationStats::with_dead_info(
                    stats.total_capacity,
                    stats.reachable_edges,
                    stats.dead_entries,
                    stats.wasted_capacity,
                ))
            }
            CsrVariant::Pure(csr) => {
                let stats = csr.get_fragmentation_stats();
                Some(FragmentationStats::with_dead_info(
                    stats.total_capacity,
                    stats.reachable_edges,
                    stats.dead_entries,
                    stats.wasted_capacity,
                ))
            }
            CsrVariant::Bundled(csr) => {
                let stats = csr.get_fragmentation_stats();
                Some(FragmentationStats::with_dead_info(
                    stats.total_capacity,
                    stats.reachable_edges,
                    stats.dead_entries,
                    stats.wasted_capacity,
                ))
            }
            _ => None,
        }
    }

    /// Average bytes per edge based on actual memory usage.
    ///
    /// Computed as `used_memory_size() / edge_count()` with no fallback.
    /// Empty tables report zero; a degenerate zero measurement on a
    /// non-empty table reports zero with a debug line. Callers handle zero
    /// explicitly instead of relying on a structural estimate.
    pub fn bytes_per_edge(&self) -> usize {
        super::super::FragmentationStats::measured_bytes_per_edge(
            self.used_memory_size(),
            self.edge_count(),
        )
    }

    /// Whether this variant accepts topology writes without unfreezing.
    ///
    /// Single source of truth for the writable set. Read-only and placeholder
    /// forms report false here and refuse result-returning writes with an
    /// error; counting deletes on those forms report zero at their own
    /// entries.
    pub fn is_writable(&self) -> bool {
        matches!(
            self,
            CsrVariant::Multiple(_)
                | CsrVariant::Single(_)
                | CsrVariant::Pure(_)
                | CsrVariant::Bundled(_)
        )
    }

    /// Whether this variant is a read-only packed view.
    ///
    /// Frozen groups need an explicit unfreeze first. Table write paths check
    /// the group frozen state before reaching the row so counting deletes
    /// never read as a plain miss.
    pub fn is_read_only(&self) -> bool {
        matches!(self, CsrVariant::Frozen(_))
    }

    /// Whether this variant is the empty placeholder.
    pub fn is_empty_placeholder(&self) -> bool {
        matches!(self, CsrVariant::None { .. })
    }

    /// Whether positional row addressing is supported.
    ///
    /// Positions are variant-local. Forms without positional addressing
    /// refuse positional writes instead of falling back to an id scan.
    pub fn supports_positions(&self) -> bool {
        matches!(
            self,
            CsrVariant::Multiple(_)
                | CsrVariant::Single(_)
                | CsrVariant::Pure(_)
                | CsrVariant::Bundled(_)
        )
    }

    /// Whether per-row reclaim with reporting is supported.
    ///
    /// Per-row defragmentation support: writable forms compact their own
    /// holes in place. Frozen single-row reclaim stays rejected and uses the
    /// batched frozen entry. Whole-table timestamp reclaim is a separate
    /// entry with its own exemptions for forms carrying no timestamps.
    pub fn supports_vertex_compact(&self) -> bool {
        matches!(
            self,
            CsrVariant::Multiple(_)
                | CsrVariant::Single(_)
                | CsrVariant::Pure(_)
                | CsrVariant::Bundled(_)
        )
    }

    /// Whether rows promise key order and may use bisection.
    ///
    /// Only single-slot and frozen rows promise order. Other forms expose a
    /// memory-only observation through row sorted state that must never be
    /// cached across restarts.
    pub fn promises_key_order(&self) -> bool {
        matches!(self, CsrVariant::Single(_) | CsrVariant::Frozen(_))
    }

    /// Whether a range scan should bisect this row.
    ///
    /// Central plan selection combining the order promise with the live
    /// sorted observation. Planning state stays memory-only and is rebuilt
    /// on load.
    pub fn should_use_bisection(&self, src_vid: u32) -> bool {
        self.promises_key_order() || self.is_row_sorted(src_vid)
    }
}
