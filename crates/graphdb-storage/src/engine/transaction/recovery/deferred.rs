use crate::engine::graph_storage::GraphStorageContext;
use graphdb_core::StorageResult;

impl GraphStorageContext {
    pub(crate) fn replay_deferred_edges(&self) -> StorageResult<()> {
        let deferred_inserts = self.take_deferred_edge_inserts();
        for (redo, ts) in deferred_inserts {
            self.do_replay_insert_edge(&redo, ts)?;
        }

        let deferred_deletes = self.take_deferred_edge_deletes();
        for (redo, ts) in deferred_deletes {
            self.do_replay_delete_edge(&redo, ts)?;
        }

        Ok(())
    }
}
