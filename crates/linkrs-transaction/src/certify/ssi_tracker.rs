//! SSI (Serializable Snapshot Isolation) rw-dependency tracker.
//!
//! Instead of scanning all committed write sets (O(N)), this tracker maintains
//! per-resource read locks that enable O(1) dangerous-structure detection.

use std::collections::HashMap;

use linkrs_core::types::Timestamp;

use crate::types::{ResourceId, TransactionId, WriteSet};

pub(super) struct SsiTracker {
    /// Per-resource list of active readers: resource → Vec<(txn_id, start_ts)>
    read_locks: parking_lot::RwLock<HashMap<ResourceId, Vec<(TransactionId, Timestamp)>>>,
}

impl SsiTracker {
    pub(super) fn new() -> Self {
        Self {
            read_locks: parking_lot::RwLock::new(HashMap::new()),
        }
    }

    /// Register that `txn_id` read `resource` at `start_ts`.
    fn register_read(&self, txn_id: TransactionId, resource: ResourceId, start_ts: Timestamp) {
        self.read_locks
            .write()
            .entry(resource)
            .or_default()
            .push((txn_id, start_ts));
    }

    /// Register read locks for every entity in `read_set` (vertices, edges,
    /// schema and index resources), so phantom detection covers index reads.
    pub(super) fn register_txn_reads(
        &self,
        txn_id: TransactionId,
        start_ts: Timestamp,
        read_set: &WriteSet,
    ) {
        for vid in read_set.vertices.iter() {
            self.register_read(txn_id, ResourceId::Vertex(*vid), start_ts);
        }
        for edge in read_set.edges.iter() {
            self.register_read(txn_id, ResourceId::Edge(*edge), start_ts);
        }
        for resource in read_set.schema_resources.iter() {
            self.register_read(txn_id, ResourceId::Schema(resource.clone()), start_ts);
        }
        for resource in read_set.index_resources.iter() {
            self.register_read(txn_id, ResourceId::Index(resource.clone()), start_ts);
        }
    }

    /// Snapshot of the readers registered on `resource`.
    pub(super) fn readers(&self, resource: &ResourceId) -> Vec<(TransactionId, Timestamp)> {
        self.read_locks
            .read()
            .get(resource)
            .cloned()
            .unwrap_or_default()
    }

    /// Remove all read locks held by `txn_id` (on commit or abort).
    pub(super) fn unregister_reads(&self, txn_id: TransactionId) {
        let mut locks = self.read_locks.write();
        locks.retain(|_, entries| {
            entries.retain(|(id, _)| *id != txn_id);
            !entries.is_empty()
        });
    }

    /// Prune read locks older than `oldest_active_ts`.
    pub(super) fn prune(&self, oldest_active_ts: Timestamp) {
        let mut locks = self.read_locks.write();
        locks.retain(|_, entries| {
            entries.retain(|(_, ts)| *ts > oldest_active_ts);
            !entries.is_empty()
        });
    }
}
