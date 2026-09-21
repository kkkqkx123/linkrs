use super::super::EdgeStore;
use graphdb_core::types::{EdgeId, Timestamp};
use graphdb_core::{StorageError, StorageResult};

impl EdgeStore {
    /// Release-mode cross-copy consistency check for one edge.
    ///
    /// Same predicates as the debug-only probe below, but fail-closed:
    /// a property row without authority or a deleted-state mismatch
    /// returns an error instead of continuing silently. Used on commit
    /// success paths; error rollback paths keep the debug-only probe so
    /// the original failure stays visible.
    pub(crate) fn ensure_copies_consistent(&self, edge_id: EdgeId) -> StorageResult<()> {
        if self.properties.get_row_for_edge(edge_id).is_some()
            && !self.mvcc.edge_timestamps.contains_key(&edge_id)
        {
            return Err(StorageError::data_corruption(format!(
                "property row mapping without authority entry for {:?}",
                edge_id
            )));
        }
        if !self.properties.is_inline_stub() {
            if let Some(info) = self.mvcc.edge_timestamps.get(&edge_id) {
                if let Some(row) = self.properties.get_row_for_edge(edge_id) {
                    let expected_deleted = info.delete_ts != Timestamp::MAX;
                    if self.properties.is_deleted_at_row(row) != expected_deleted {
                        return Err(StorageError::data_corruption(format!(
                            "property deleted-state drift from authority for {:?}",
                            edge_id
                        )));
                    }
                }
            }
        }
        Ok(())
    }

    /// Debug-only cross-copy consistency check for one edge.
    ///
    /// Release builds skip the whole body (zero overhead): every arm is a
    /// `debug_assert`. A property row mapping must never outlive its
    /// authority entry (orphan row), and a present property row must carry
    /// the same deleted state as the authority. Called on insert success,
    /// delete success and delete-rollback success. The whole-table
    /// counterpart [`EdgeStore::audit_copy_drift`](super::super::EdgeStore::audit_copy_drift)
    /// adds the CSR cold-half stamp comparison for maintenance-time audits.
    pub(crate) fn debug_assert_copies_consistent(&self, edge_id: EdgeId) {
        debug_assert!(
            self.properties.get_row_for_edge(edge_id).is_none()
                || self.mvcc.edge_timestamps.contains_key(&edge_id),
            "property row mapping without authority entry"
        );
        if !self.properties.is_inline_stub() {
            if let Some(info) = self.mvcc.edge_timestamps.get(&edge_id) {
                if let Some(row) = self.properties.get_row_for_edge(edge_id) {
                    let expected_deleted = info.delete_ts != Timestamp::MAX;
                    debug_assert_eq!(
                        self.properties.is_deleted_at_row(row),
                        expected_deleted,
                        "property deleted-state drift from authority"
                    );
                }
            }
        }
    }
}
