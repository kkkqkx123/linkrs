use graphdb_core::types::EdgeId;

use super::super::csr_shared::{OverflowChunkSpec, OverflowTable};

#[derive(Debug, Clone, Default)]
pub(crate) struct PureOverflowChunk {
    pub(crate) endpoints: Vec<u32>,
    pub(crate) edge_ids: Vec<u64>,
}

impl PureOverflowChunk {
    pub(crate) fn with_capacity(cap: usize) -> Self {
        Self {
            endpoints: Vec::with_capacity(cap),
            edge_ids: Vec::with_capacity(cap),
        }
    }

    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.endpoints.len()
    }

    #[inline]
    pub(crate) fn is_empty(&self) -> bool {
        self.endpoints.is_empty()
    }

    #[inline]
    pub(crate) fn capacity(&self) -> usize {
        self.endpoints.capacity()
    }

    #[inline]
    pub(crate) fn push(&mut self, endpoint: u32, edge_id: EdgeId) {
        self.endpoints.push(endpoint);
        self.edge_ids.push(edge_id.0);
    }

    #[inline]
    pub(crate) fn remove(&mut self, index: usize) {
        self.endpoints.remove(index);
        self.edge_ids.remove(index);
    }

    #[inline]
    pub(crate) fn endpoint_at(&self, index: usize) -> Option<u32> {
        self.endpoints.get(index).copied()
    }

    #[inline]
    pub(crate) fn edge_id_at(&self, index: usize) -> Option<EdgeId> {
        self.edge_ids.get(index).map(|&v| EdgeId(v))
    }

    pub(crate) fn consolidated(endpoints: &[u32], edge_ids: &[u64]) -> Self {
        Self {
            endpoints: endpoints.to_vec(),
            edge_ids: edge_ids.to_vec(),
        }
    }
}

impl super::super::csr_shared::OverflowChunkSpec for PureOverflowChunk {
    type Slot = (u32, EdgeId);

    fn with_capacity(cap: usize) -> Self {
        PureOverflowChunk::with_capacity(cap)
    }

    #[inline]
    fn len(&self) -> usize {
        self.len()
    }

    #[inline]
    fn capacity(&self) -> usize {
        self.capacity()
    }

    #[inline]
    fn push_slot(&mut self, slot: (u32, EdgeId)) {
        self.push(slot.0, slot.1);
    }
}

/// Pure-CSR overflow storage: the shared overflow table over
/// endpoint/edge-id chunk halves.
pub(crate) type PureOverflowStorage = OverflowTable<PureOverflowChunk>;
