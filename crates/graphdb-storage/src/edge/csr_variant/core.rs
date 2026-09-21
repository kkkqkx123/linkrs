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
    ///
    /// Mapped views hold their payload off heap and cannot be emptied, so
    /// clearing one swaps the whole variant to the empty placeholder at the
    /// same vertex capacity. Group containers keep the group slot and must
    /// treat the post-clear variant as a placeholder for every later branch.
    pub fn clear(&mut self) {
        // Clearing drops the snapshot view: the mapping is outside the heap
        // and cannot be emptied, so the group falls back to the placeholder.
        if let CsrVariant::Mapped(csr) = self {
            let vertex_capacity = csr.vertex_capacity();
            *self = CsrVariant::None { vertex_capacity };
            return;
        }
        match self {
            CsrVariant::Multiple(csr) => csr.clear(),
            CsrVariant::Single(csr) => csr.clear(),
            CsrVariant::Pure(csr) => csr.clear(),
            CsrVariant::Bundled(csr) => csr.clear(),
            CsrVariant::Frozen(csr) => csr.clear(),
            CsrVariant::Mapped(_) => unreachable!("mapped view replaced above"),
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

    /// Get fragmentation ratio for diagnostics
    ///
    /// Covers every variant holding reserved row capacity: the multi-edge
    /// store plus the pure-topology and bundled forms sharing its
    /// primary-plus-overflow layout. Single-slot, frozen, mapped and empty
    /// forms report zero: single slots carry no reserved gaps beyond their
    /// tombstone, frozen and mapped rows are packed without gaps, and the
    /// placeholder holds no edges.
    pub fn fragmentation_ratio(&self) -> f32 {
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
}
