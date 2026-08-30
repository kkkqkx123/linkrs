use super::SyncWrapper;
use crate::{StorageClient, StorageSchemaOps};
use graphdb_core::StorageError;

macro_rules! forward_auto_commit_methods {
    ($field:ident; $(fn $name:ident(&mut self $(, $arg:ident : $ty:ty)* $(,)?) -> $ret:ty;)+) => {
        $(
            fn $name(&mut self, $($arg: $ty),*) -> $ret {
                let result = self.$field.$name($($arg),*);
                if result.is_ok() {
                    self.commit_auto_transaction()?;
                }
                result
            }
        )+
    };
}

impl<S: StorageClient + 'static> StorageSchemaOps for SyncWrapper<S> {
    forward_auto_commit_methods!(inner;
        fn create_space(&mut self, space: &mut graphdb_core::types::SpaceInfo) -> Result<bool, StorageError>;
        fn drop_space(&mut self, space: &str) -> Result<bool, StorageError>;
        fn clear_space(&mut self, space: &str) -> Result<bool, StorageError>;
        fn alter_space_comment(&mut self, space_id: u64, comment: String) -> Result<bool, StorageError>;
        fn create_tag(&mut self, space: &str, tag: &graphdb_core::types::TagInfo) -> Result<u32, StorageError>;
        fn alter_tag(
            &mut self,
            space: &str,
            tag: &str,
            additions: Vec<graphdb_core::types::PropertyDef>,
            deletions: Vec<String>,
        ) -> Result<bool, StorageError>;
        fn rename_vertex_property(
            &mut self,
            label: graphdb_core::types::LabelId,
            old_name: &str,
            new_name: &str,
        ) -> Result<(), StorageError>;
        fn rename_tag_property(
            &mut self,
            space: &str,
            tag: &str,
            old_name: &str,
            new_name: &str,
        ) -> Result<bool, StorageError>;
        fn drop_tag(&mut self, space: &str, tag: &str) -> Result<bool, StorageError>;
        fn create_edge_type(
            &mut self,
            space: &str,
            edge: &graphdb_core::types::EdgeTypeInfo,
        ) -> Result<u32, StorageError>;
        fn alter_edge_type(
            &mut self,
            space: &str,
            edge_type: &str,
            additions: Vec<graphdb_core::types::PropertyDef>,
            deletions: Vec<String>,
        ) -> Result<bool, StorageError>;
        fn drop_edge_type(&mut self, space: &str, edge: &str) -> Result<bool, StorageError>;
        fn rebuild_tag_index(&mut self, space: &str, index: &str) -> Result<bool, StorageError>;
        fn rebuild_edge_index(&mut self, space: &str, index: &str) -> Result<bool, StorageError>;
    );

    fn create_tag_index(
        &mut self,
        space: &str,
        info: &graphdb_core::types::Index,
    ) -> Result<bool, StorageError> {
        self.validate_schema_sync_context()?;
        let result = self.inner.create_tag_index(space, info)?;
        if result {
            if let Err(error) = self.stage_index_create(info, "tag") {
                if let Some(transaction_id) = self.get_current_txn_id() {
                    let _ = self.abort_transaction_fact(transaction_id);
                }
                return Err(error);
            }
        }
        self.commit_auto_transaction()?;
        Ok(result)
    }

    fn drop_tag_index(&mut self, space: &str, index: &str) -> Result<bool, StorageError> {
        let space_id = self.inner.get_space_id(space)?;
        let (fields, schema_name) = self
            .inner
            .get_tag_index(space, index)?
            .map(|definition| {
                (
                    definition
                        .fields
                        .into_iter()
                        .map(|field| field.name)
                        .collect(),
                    definition.schema_name,
                )
            })
            .unwrap_or_else(|| (Vec::new(), index.to_string()));
        self.validate_schema_sync_context()?;
        let result = self.inner.drop_tag_index(space, index)?;
        if result {
            if let Err(error) = self.stage_index_drop(space_id, index, &schema_name, "tag", &fields)
            {
                if let Some(transaction_id) = self.get_current_txn_id() {
                    let _ = self.abort_transaction_fact(transaction_id);
                }
                return Err(error);
            }
        }
        self.commit_auto_transaction()?;
        Ok(result)
    }

    fn create_edge_index(
        &mut self,
        space: &str,
        info: &graphdb_core::types::Index,
    ) -> Result<bool, StorageError> {
        self.validate_schema_sync_context()?;
        let result = self.inner.create_edge_index(space, info)?;
        if result {
            if let Err(error) = self.stage_index_create(info, "edge") {
                if let Some(transaction_id) = self.get_current_txn_id() {
                    let _ = self.abort_transaction_fact(transaction_id);
                }
                return Err(error);
            }
        }
        self.commit_auto_transaction()?;
        Ok(result)
    }

    fn drop_edge_index(&mut self, space: &str, index: &str) -> Result<bool, StorageError> {
        let space_id = self.inner.get_space_id(space)?;
        let (fields, schema_name) = self
            .inner
            .get_edge_index(space, index)?
            .map(|definition| {
                (
                    definition
                        .fields
                        .into_iter()
                        .map(|field| field.name)
                        .collect(),
                    definition.schema_name,
                )
            })
            .unwrap_or_else(|| (Vec::new(), index.to_string()));
        self.validate_schema_sync_context()?;
        let result = self.inner.drop_edge_index(space, index)?;
        if result {
            if let Err(error) =
                self.stage_index_drop(space_id, index, &schema_name, "edge", &fields)
            {
                if let Some(transaction_id) = self.get_current_txn_id() {
                    let _ = self.abort_transaction_fact(transaction_id);
                }
                return Err(error);
            }
        }
        self.commit_auto_transaction()?;
        Ok(result)
    }
}
