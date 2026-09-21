use graphdb_core::types::EdgeId;

use super::super::{EdgePosition, MutableCsrTrait, Nbr};
use super::{BundledCsr, INVALID_EDGE_ID};

impl BundledCsr {
    pub fn visit_physical<F>(&self, src_vid: u32, f: F)
    where
        F: FnMut(Nbr) -> bool,
    {
        self.topology.visit_physical(src_vid, f)
    }

    /// Borrowed row walk over live entries without allocating.
    ///
    /// Topology half of the paired bundled traversal: values stay in their
    /// columns and must be resolved through `visit_physical_with_values` or
    /// the value-by-key accessors. Walking this alone silently drops values.
    pub fn iter_row(&self, src_vid: u32) -> super::super::pure_csr::PureRowIter<'_> {
        self.topology.iter_row(src_vid)
    }

    /// Borrowed walk over every live entry of the table without allocating.
    ///
    /// Topology only; pair with the value entry when values are needed.
    pub fn iter_all(&self) -> super::super::pure_csr::PureAllIter<'_> {
        self.topology.iter_all()
    }

    pub fn visit_hot<F>(&self, src_vid: u32, f: F)
    where
        F: FnMut(super::super::HotNbr) -> bool,
    {
        self.topology.visit_hot(src_vid, f)
    }

    /// Whether the live endpoints of one row arrive in ascending order.
    pub fn is_row_sorted(&self, src_vid: u32) -> bool {
        self.topology.is_row_sorted(src_vid)
    }

    /// Visit live entries whose endpoint falls in the inclusive range.
    ///
    /// Endpoint-only range by construction: bundled rows carry no rank, so
    /// callers pass endpoint intervals and must not expect rank filtering.
    pub fn visit_threshold<F>(&self, src_vid: u32, lower: Option<u32>, upper: Option<u32>, f: F)
    where
        F: FnMut(Nbr) -> bool,
    {
        self.topology.visit_threshold(src_vid, lower, upper, f)
    }

    /// Fill a caller buffer with the same range content as `visit_threshold`.
    pub fn fill_threshold_into(
        &self,
        src_vid: u32,
        lower: Option<u32>,
        upper: Option<u32>,
        out: &mut Vec<Nbr>,
    ) {
        self.topology
            .fill_threshold_into(src_vid, lower, upper, out)
    }

    /// Visit every physically stored entry of one vertex together with its
    /// inline value (`None` for NULL slots).
    pub fn visit_physical_with_values<F>(&self, src_vid: u32, mut f: F)
    where
        F: FnMut(Nbr, Option<u64>) -> bool,
    {
        self.topology
            .visit_physical_with_position(src_vid, |position, nbr| {
                f(
                    nbr,
                    self.value_at_position(src_vid, position)
                        .and_then(|(raw, valid)| if valid { Some(raw) } else { None }),
                )
            })
    }

    /// Whether any slot in the table holds a valid (non-NULL) value.
    pub fn any_valid_values(&self) -> bool {
        if self.primary_valid.count_ones() > 0 {
            return true;
        }
        self.overflow_values
            .iter()
            .any(|(_, chunks)| chunks.iter().any(|c| c.valid.count_ones() > 0))
    }

    /// Read the raw value and validity of one physical slot.
    pub fn value_at_position(&self, src_vid: u32, position: EdgePosition) -> Option<(u64, bool)> {
        match position {
            EdgePosition::Primary { slot } => {
                let idx = self.primary_index(src_vid, slot)?;
                let valid = self.primary_valid.get(idx).map(|b| *b).unwrap_or(false);
                Some((self.primary_values[idx], valid))
            }
            EdgePosition::Overflow { chunk, slot } => {
                let chunks = self.overflow_values.get(src_vid)?;
                let c = chunks.get(chunk as usize)?;
                let value = *c.values.get(slot as usize)?;
                let valid = c.get_valid(slot as usize);
                Some((value, valid))
            }
        }
    }

    /// Read the value of one edge by id within its source row.
    pub fn value_by_edge_id(&self, src_vid: u32, edge_id: EdgeId) -> Option<(u64, bool)> {
        let (position, _) = self.topology.locate_edge(src_vid, edge_id)?;
        self.value_at_position(src_vid, position)
    }

    /// Read the value of the live edge for one endpoint.
    pub fn value_by_endpoint(&self, src_vid: u32, endpoint: u32) -> Option<(u64, bool)> {
        let mut found = None;
        self.topology
            .visit_physical_with_position(src_vid, |position, nbr| {
                if nbr.endpoint == endpoint && nbr.edge_id != INVALID_EDGE_ID {
                    found = self.value_at_position(src_vid, position);
                    false
                } else {
                    true
                }
            });
        found
    }
}
