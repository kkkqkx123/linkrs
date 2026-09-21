use super::{CsrWithProperties, RowVisibility};
use graphdb_core::types::{EdgeId, Timestamp};
use graphdb_core::{StorageError, StorageResult};

impl RowVisibility {
    pub(crate) fn new(create_ts: Timestamp) -> Self {
        Self {
            create_ts,
            delete_ts: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn is_visible_at(&self, query_ts: Timestamp) -> bool {
        crate::mvcc_visibility::Visibility::is_visible(query_ts, self.create_ts, self.delete_ts)
    }

    pub(crate) fn mark_deleted(&mut self, ts: Timestamp) {
        if self.delete_ts.is_none() {
            self.delete_ts = Some(ts);
        }
    }
}

impl CsrWithProperties {
    pub fn mark_deleted(&mut self, edge_id: EdgeId, ts: Timestamp) -> bool {
        if self.inline {
            return false;
        }
        if let Some(pos) = self.mapped_row(edge_id) {
            if let Some(vis) = self.visibility.get_mut(pos) {
                if vis.delete_ts.is_some() {
                    return false;
                }
                vis.mark_deleted(ts);
                return true;
            }
        }
        false
    }

    pub fn mark_deleted_at_row(&mut self, row_idx: usize, ts: Timestamp) -> StorageResult<()> {
        if row_idx >= self.visibility.len() {
            return Ok(());
        }
        if self.visibility[row_idx].delete_ts.is_some() {
            return Err(StorageError::invalid_operation(
                "record already marked deleted",
            ));
        }
        self.visibility[row_idx].mark_deleted(ts);
        Ok(())
    }

    pub fn is_deleted_at_row(&self, row_idx: usize) -> bool {
        if let Some(vis) = self.visibility.get(row_idx) {
            return vis.delete_ts.is_some();
        }
        false
    }

    pub fn revert_deletion_at_row(&mut self, row_idx: usize) -> bool {
        if let Some(vis) = self.visibility.get_mut(row_idx) {
            if vis.delete_ts.is_some() {
                vis.delete_ts = None;
                return true;
            }
        }
        false
    }

    pub fn revert_deletion_for_edge(&mut self, edge_id: EdgeId) -> bool {
        if self.inline {
            return false;
        }
        if let Some(pos) = self.mapped_row(edge_id) {
            return self.revert_deletion_at_row(pos);
        }
        false
    }
}
