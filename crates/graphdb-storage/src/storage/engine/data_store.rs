use std::collections::HashMap;

use parking_lot::RwLock;

use crate::core::types::LabelId;
use crate::storage::edge::EdgeTable;
use crate::storage::vertex::VertexTable;

#[derive(Hash, Eq, PartialEq, Clone, Copy, Debug)]
pub struct EdgeTableKey {
    pub src_label: LabelId,
    pub dst_label: LabelId,
    pub edge_label: LabelId,
}

impl EdgeTableKey {
    pub fn new(src_label: LabelId, dst_label: LabelId, edge_label: LabelId) -> Self {
        Self {
            src_label,
            dst_label,
            edge_label,
        }
    }
}

impl From<(LabelId, LabelId, LabelId)> for EdgeTableKey {
    fn from((src_label, dst_label, edge_label): (LabelId, LabelId, LabelId)) -> Self {
        Self {
            src_label,
            dst_label,
            edge_label,
        }
    }
}

pub struct GraphDataStore {
    vertex_tables: RwLock<HashMap<LabelId, VertexTable>>,
    edge_tables: RwLock<HashMap<EdgeTableKey, EdgeTable>>,
    vertex_label_names: RwLock<HashMap<String, LabelId>>,
    edge_label_names: RwLock<HashMap<String, LabelId>>,
    vertex_label_counter: RwLock<LabelId>,
    edge_label_counter: RwLock<LabelId>,
    /// Reverse index: edge_label -> list of EdgeTableKeys
    /// Enables O(1) lookup of all tables for a given edge label
    /// Significantly improves performance of edge property operations
    edge_label_index: RwLock<HashMap<LabelId, Vec<EdgeTableKey>>>,
}

impl GraphDataStore {
    pub fn new() -> Self {
        Self {
            vertex_tables: RwLock::new(HashMap::new()),
            edge_tables: RwLock::new(HashMap::new()),
            vertex_label_names: RwLock::new(HashMap::new()),
            edge_label_names: RwLock::new(HashMap::new()),
            vertex_label_counter: RwLock::new(0),
            edge_label_counter: RwLock::new(0),
            edge_label_index: RwLock::new(HashMap::new()),
        }
    }

    pub(crate) fn vertex_tables(&self) -> &RwLock<HashMap<LabelId, VertexTable>> {
        &self.vertex_tables
    }

    pub(crate) fn edge_tables(&self) -> &RwLock<HashMap<EdgeTableKey, EdgeTable>> {
        &self.edge_tables
    }

    pub(crate) fn vertex_label_names(&self) -> &RwLock<HashMap<String, LabelId>> {
        &self.vertex_label_names
    }

    pub(crate) fn edge_label_names(&self) -> &RwLock<HashMap<String, LabelId>> {
        &self.edge_label_names
    }

    pub(crate) fn vertex_label_counter(&self) -> &RwLock<LabelId> {
        &self.vertex_label_counter
    }

    pub(crate) fn edge_label_counter(&self) -> &RwLock<LabelId> {
        &self.edge_label_counter
    }

    pub(crate) fn edge_label_index(&self) -> &RwLock<HashMap<LabelId, Vec<EdgeTableKey>>> {
        &self.edge_label_index
    }
}

impl Default for GraphDataStore {
    fn default() -> Self {
        Self::new()
    }
}
