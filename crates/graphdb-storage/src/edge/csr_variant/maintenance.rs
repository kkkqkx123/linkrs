use super::super::{EdgeId, Timestamp};
use super::CsrVariant;

impl CsrVariant {
    /// Compact with per-edge removal reporting.
    ///
    /// Whole-table timestamp reclaim: the capability query owns the
    /// exemption list, the match below only serves the supported forms.
    /// Pure and bundled rows hold no timestamps so they report zero; their
    /// holes still compact through the per-row entry. Production frozen
    /// reclaim uses the batched rows entry below instead of one call per row.
    pub fn compact_with_ts_reporting(
        &mut self,
        cutoff: Timestamp,
        reserve_ratio: f32,
        on_edge_removed: &mut dyn FnMut(EdgeId, Timestamp),
    ) -> usize {
        if !self.supports_timestamp_reclaim() {
            return 0;
        }
        match self {
            CsrVariant::Multiple(csr) => {
                csr.compact_with_ts_reporting(cutoff, reserve_ratio, on_edge_removed)
            }
            CsrVariant::Single(csr) => csr.compact_with_ts_reporting(cutoff, on_edge_removed),
            CsrVariant::Frozen(csr) => csr.compact_with_cutoff(cutoff, on_edge_removed),
            _ => 0,
        }
    }

    /// Reclaim a listed row set of a frozen group in one linear pass.
    ///
    /// Production replacement for repeated per-row frozen compactions: one
    /// table-proportional pass instead of one trailing memmove per row.
    /// Only the frozen variant compacts; every other variant reports zero
    /// so callers keep their existing per-row path for mutable groups.
    pub fn compact_frozen_rows_batched(
        &mut self,
        vids: &[u32],
        cutoff: Timestamp,
        on_edge_removed: &mut dyn FnMut(EdgeId, Timestamp),
    ) -> usize {
        match self {
            CsrVariant::Frozen(csr) => csr.compact_rows_batched(vids, cutoff, on_edge_removed),
            _ => 0,
        }
    }

    /// Sort one primary row into key order on the maintenance path.
    ///
    /// Same invalidation as the underlying stores: positions go stale and
    /// the caller relocates through the edge-id key. Frozen, single-slot and
    /// empty forms are already ordered and report false.
    pub fn sort_row(&mut self, src_vid: u32) -> bool {
        match self {
            CsrVariant::Multiple(csr) => csr.sort_row(src_vid),
            CsrVariant::Pure(csr) => csr.sort_row(src_vid),
            CsrVariant::Bundled(csr) => csr.sort_row(src_vid),
            CsrVariant::Single(_) | CsrVariant::Frozen(_) | CsrVariant::None { .. } => false,
        }
    }
}
