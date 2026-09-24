use super::ShardedVertexTable;
use crate::schema::ChangeDetails;
use crate::types::StoragePropertyDef;
use graphdb_core::StorageResult;

impl ShardedVertexTable {
    pub fn schema(&self) -> crate::vertex::VertexSchema {
        // Shard 0 is the schema authority: every shard holds the same schema
        // and all schema mutations are applied to each shard in order.
        self.shards[0].read().schema().clone()
    }

    /// Replace the schema on every shard without staging.
    ///
    /// Recovery and undo compensation only: it replays an already-committed
    /// schema (WAL redo, transaction rollback) rather than evolving live
    /// state. All live schema evolution must go through the staged
    /// prepare/fill/publish machine (`add_property`, `remove_property`,
    /// `rename_property` and their staged fan-outs).
    pub(crate) fn apply_schema(&self, schema: crate::vertex::VertexSchema) -> StorageResult<()> {
        for shard in &self.shards {
            shard.write().set_schema(schema.clone())?;
        }
        Ok(())
    }

    pub fn label(&self) -> graphdb_core::types::LabelId {
        self.label
    }

    pub fn label_name(&self) -> &str {
        &self.label_name
    }

    pub fn add_property(&self, prop: StoragePropertyDef) -> StorageResult<()> {
        // Single-step convenience over the staged machine: prepare, fill and
        // publish each fan out to every shard, aborting everywhere when a
        // phase fails midway.
        self.prepare_add_property_staged(prop)?;
        if let Err(error) = self.fill_pending_schema_change() {
            self.abort_pending_schema_change();
            return Err(error);
        }
        if let Err(error) = self.publish_pending_schema_change() {
            self.abort_pending_schema_change();
            return Err(error);
        }
        Ok(())
    }

    pub fn remove_property(&self, prop_name: &str) -> StorageResult<()> {
        self.prepare_remove_property_staged(prop_name)?;
        if let Err(error) = self.fill_pending_schema_change() {
            self.abort_pending_schema_change();
            return Err(error);
        }
        if let Err(error) = self.publish_pending_schema_change() {
            self.abort_pending_schema_change();
            return Err(error);
        }
        Ok(())
    }

    pub fn rename_property(&self, old_name: &str, new_name: &str) -> StorageResult<()> {
        self.prepare_rename_property_staged(old_name, new_name)?;
        if let Err(error) = self.fill_pending_schema_change() {
            self.abort_pending_schema_change();
            return Err(error);
        }
        if let Err(error) = self.publish_pending_schema_change() {
            self.abort_pending_schema_change();
            return Err(error);
        }
        Ok(())
    }

    pub fn rebuild_schema_change_from_redo(&self, details: ChangeDetails) -> StorageResult<()> {
        for shard in &self.shards {
            shard
                .write()
                .rebuild_schema_change_from_redo(details.clone())?;
        }
        Ok(())
    }

    /// Abort staged changes on every shard holding one. Shards without a
    /// pending change are skipped so compensation after a partial fan-out
    /// never fails on already-clean shards.
    fn abort_staged_on_all_shards(&self) {
        for shard in &self.shards {
            if shard.read().has_pending_schema_change() {
                let _ = shard.write().abort_pending_schema_change();
            }
        }
    }

    /// Staged add-column fan-out: prepare, fill and publish run as separate
    /// passes so callers can interleave checkpoints between phases. Any phase
    /// failing midway aborts the staged change on all shards, leaving no
    /// residual column behind.
    pub fn prepare_add_property_staged(&self, prop: StoragePropertyDef) -> StorageResult<()> {
        for shard in &self.shards {
            if let Err(error) = shard.write().prepare_add_property_staged(prop.clone()) {
                self.abort_staged_on_all_shards();
                return Err(error);
            }
        }
        Ok(())
    }

    /// Staged drop-column fan-out with the same abort compensation.
    pub fn prepare_remove_property_staged(&self, prop_name: &str) -> StorageResult<()> {
        for shard in &self.shards {
            if let Err(error) = shard.write().prepare_remove_property_staged(prop_name) {
                self.abort_staged_on_all_shards();
                return Err(error);
            }
        }
        Ok(())
    }

    /// Staged rename-column fan-out with the same abort compensation.
    pub fn prepare_rename_property_staged(
        &self,
        old_name: &str,
        new_name: &str,
    ) -> StorageResult<()> {
        for shard in &self.shards {
            if let Err(error) = shard
                .write()
                .prepare_rename_property_staged(old_name, new_name)
            {
                self.abort_staged_on_all_shards();
                return Err(error);
            }
        }
        Ok(())
    }

    /// Fill the staged change on every shard, aborting everywhere on failure.
    pub fn fill_pending_schema_change(&self) -> StorageResult<()> {
        for shard in &self.shards {
            if let Err(error) = shard.write().fill_pending_schema_change() {
                self.abort_staged_on_all_shards();
                return Err(error);
            }
        }
        Ok(())
    }

    /// Publish the staged change on every shard, aborting everywhere on failure.
    pub fn publish_pending_schema_change(&self) -> StorageResult<()> {
        for shard in &self.shards {
            if let Err(error) = shard.write().publish_pending_schema_change() {
                self.abort_staged_on_all_shards();
                return Err(error);
            }
        }
        Ok(())
    }

    /// Abort the staged change on every shard holding one.
    pub fn abort_pending_schema_change(&self) {
        self.abort_staged_on_all_shards();
    }
}
