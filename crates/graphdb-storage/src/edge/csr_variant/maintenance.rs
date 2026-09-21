use super::super::{EdgeId, Timestamp};
use super::CsrVariant;

impl CsrVariant {
    /// Compact with per-edge removal reporting.
    ///
    /// Both retained strategies report reclaimed tombstones; the placeholder
    /// keeps the no-op semantics. Frozen groups reclaim tombstones in place
    /// with no reserve; mapped views need a snapshot-file rebuild and stay
    /// no-op here.
    pub fn compact_with_ts_reporting(
        &mut self,
        cutoff: Timestamp,
        reserve_ratio: f32,
        on_edge_removed: &mut dyn FnMut(EdgeId, Timestamp),
    ) -> usize {
        match self {
            CsrVariant::Multiple(csr) => {
                csr.compact_with_ts_reporting(cutoff, reserve_ratio, on_edge_removed)
            }
            CsrVariant::Single(csr) => csr.compact_with_ts_reporting(cutoff, on_edge_removed),
            CsrVariant::Pure(_) | CsrVariant::Bundled(_) => 0,
            CsrVariant::Frozen(csr) => csr.compact_with_cutoff(cutoff, on_edge_removed),
            CsrVariant::Mapped(_) => 0,
            CsrVariant::None { .. } => 0,
        }
    }

    /// Sort one primary row into key order on the maintenance path.
    ///
    /// Same invalidation as the underlying stores: positions go stale and
    /// the caller relocates through the edge-id key. Frozen, mapped,
    /// single-slot and empty forms are already ordered and report false.
    pub fn sort_row(&mut self, src_vid: u32) -> bool {
        match self {
            CsrVariant::Multiple(csr) => csr.sort_row(src_vid),
            CsrVariant::Pure(csr) => csr.sort_row(src_vid),
            CsrVariant::Bundled(csr) => csr.sort_row(src_vid),
            CsrVariant::Single(_)
            | CsrVariant::Frozen(_)
            | CsrVariant::Mapped(_)
            | CsrVariant::None { .. } => false,
        }
    }
}
