//! `GraphDataStore`: catalog struct plus its four concern modules.
//!
//! - [`lock_metrics`]: catalog/table lock metering, guards and write sets;
//! - [`access`]: metered read/write accessors for the seven registries;
//! - [`partitions`]: edge partition scatter-gather and lazy creation;
//! - [`schema_ops`]: register/drop/rename catalog mutations.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;

use crate::edge::EdgeStore;
use crate::vertex::ShardedVertexTable;
use graphdb_core::types::LabelId;

mod access;
mod lock_metrics;
mod partitions;
mod schema_ops;

pub(crate) use lock_metrics::CatalogLockOperation;

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
    pub(super) vertex_tables: RwLock<HashMap<LabelId, Arc<ShardedVertexTable>>>,
    pub(super) edge_tables: RwLock<HashMap<EdgeTableKey, Arc<RwLock<EdgeStore>>>>,
    pub(super) vertex_label_names: RwLock<HashMap<String, LabelId>>,
    pub(super) edge_label_names: RwLock<HashMap<String, LabelId>>,
    pub(super) vertex_label_counter: RwLock<LabelId>,
    pub(super) edge_label_counter: RwLock<LabelId>,
    /// Reverse index: edge_label -> list of EdgeTableKeys
    /// Enables O(1) lookup of all tables for a given edge label
    /// Significantly improves performance of edge property operations
    pub(super) edge_label_index: RwLock<HashMap<LabelId, Vec<EdgeTableKey>>>,
    pub(super) lock_metrics: lock_metrics::CatalogLockMetrics,
    pub(super) table_lock_metrics: lock_metrics::TableLockMetrics,
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
            lock_metrics: lock_metrics::CatalogLockMetrics::default(),
            table_lock_metrics: lock_metrics::TableLockMetrics::default(),
        }
    }
}

impl Default for GraphDataStore {
    fn default() -> Self {
        Self::new()
    }
}
