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

    pub fn apply_schema(&self, schema: crate::vertex::VertexSchema) {
        for shard in &self.shards {
            shard.write().set_schema(schema.clone());
        }
    }

    pub fn label(&self) -> graphdb_core::types::LabelId {
        self.label
    }

    pub fn label_name(&self) -> &str {
        &self.label_name
    }

    pub fn add_property(&self, prop: StoragePropertyDef) -> StorageResult<()> {
        for shard in &self.shards {
            shard.write().add_property(prop.clone())?;
        }
        Ok(())
    }

    pub fn remove_property(&self, prop_name: &str) -> StorageResult<()> {
        for shard in &self.shards {
            shard.write().remove_property(prop_name)?;
        }
        Ok(())
    }

    pub fn rename_property(&self, old_name: &str, new_name: &str) -> StorageResult<()> {
        for shard in &self.shards {
            shard.write().rename_property(old_name, new_name)?;
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
}
