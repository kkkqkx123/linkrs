use graphdb_core::{StorageError, StorageResult};

use super::super::{EdgeId, EdgePosition, Nbr, Timestamp, VertexId};
use super::CsrVariant;

impl CsrVariant {
    /// Whether this group stores its single scalar inline.
    pub fn is_bundled(&self) -> bool {
        matches!(self, CsrVariant::Bundled(_))
    }

    /// Insert one edge carrying its inline value.
    ///
    /// Only the bundled form accepts a value; every other form rejects the
    /// call instead of silently dropping the property.
    pub fn insert_edge_with_value(
        &mut self,
        src_vid: u32,
        dst: VertexId,
        edge_id: EdgeId,
        value: Option<u64>,
    ) -> StorageResult<()> {
        match self {
            CsrVariant::Bundled(csr) => csr.insert_edge_with_value(src_vid, dst, edge_id, value),
            _ => Err(StorageError::invalid_operation(
                "inline values require the bundled record form".to_string(),
            )),
        }
    }

    /// Read one edge's inline value within its source row.
    pub fn bundled_value_by_edge_id(&self, src_vid: u32, edge_id: EdgeId) -> Option<(u64, bool)> {
        match self {
            CsrVariant::Bundled(csr) => csr.value_by_edge_id(src_vid, edge_id),
            _ => None,
        }
    }

    /// Overwrite one edge's inline value within its source row.
    pub fn bundled_set_value_by_edge_id(
        &mut self,
        src_vid: u32,
        edge_id: EdgeId,
        value: Option<u64>,
    ) -> bool {
        match self {
            CsrVariant::Bundled(csr) => csr.set_value_by_edge_id(src_vid, edge_id, value),
            _ => false,
        }
    }

    /// Read the inline value of the live edge for one endpoint.
    pub fn bundled_value_by_endpoint(&self, src_vid: u32, endpoint: u32) -> Option<(u64, bool)> {
        match self {
            CsrVariant::Bundled(csr) => csr.value_by_endpoint(src_vid, endpoint),
            _ => None,
        }
    }

    /// Overwrite the inline value of the live edge for one endpoint.
    pub fn bundled_set_value_by_endpoint(
        &mut self,
        src_vid: u32,
        endpoint: u32,
        value: Option<u64>,
    ) -> bool {
        match self {
            CsrVariant::Bundled(csr) => csr.set_value_by_endpoint(src_vid, endpoint, value),
            _ => false,
        }
    }

    /// Revert a deletion restoring the caller's value alongside the topology.
    pub fn bundled_revert_with_value(
        &mut self,
        src_vid: u32,
        position: EdgePosition,
        expected: EdgeId,
        ts: Timestamp,
        value: Option<u64>,
    ) -> bool {
        match self {
            CsrVariant::Bundled(csr) => {
                csr.revert_delete_at_position_with_value(src_vid, position, expected, ts, value)
            }
            _ => false,
        }
    }

    /// Visit every physically stored entry of one vertex with its inline
    /// value (`None` for NULL slots). Non-bundled forms visit with `None`.
    pub fn visit_physical_with_values<F>(&self, src_vid: u32, mut f: F)
    where
        F: FnMut(Nbr, Option<u64>) -> bool,
    {
        match self {
            CsrVariant::Bundled(csr) => csr.visit_physical_with_values(src_vid, f),
            _ => self.visit_physical(src_vid, |nbr| f(nbr, None)),
        }
    }

    /// Whether any slot holds a valid inline value.
    pub fn bundled_has_valid_values(&self) -> bool {
        match self {
            CsrVariant::Bundled(csr) => csr.any_valid_values(),
            _ => false,
        }
    }
}
