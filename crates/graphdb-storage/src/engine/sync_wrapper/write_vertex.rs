use super::SyncWrapper;
use crate::StorageClient;
use graphdb_core::types::VertexId;
use graphdb_core::{StorageError, Value, Vertex};
use graphdb_sync::types::ChangeType;

impl<S: StorageClient + 'static> SyncWrapper<S> {
    fn detect_changed_properties(
        old_vertex: &Vertex,
        new_vertex: &Vertex,
    ) -> Vec<(String, Value)> {
        let mut changed_props = Vec::new();

        for (prop_name, new_value) in &new_vertex.tag.properties {
            match old_vertex.tag.properties.get(prop_name) {
                Some(old_value) if old_value != new_value => {
                    changed_props.push((prop_name.clone(), new_value.clone()));
                }
                None => {
                    changed_props.push((prop_name.clone(), new_value.clone()));
                }
                _ => {}
            }
        }

        changed_props
    }

    pub(super) fn sync_insert_vertex(
        &mut self,
        space: &str,
        vertex: &Vertex,
    ) -> Result<(), StorageError> {
        if !self.enabled {
            return Ok(());
        }

        let Some(sync_manager) = self.get_sync_manager() else {
            return Ok(());
        };

        let space_id = self.inner.get_space_id(space)?;
        let txn_id = self.get_current_txn_id();

        let tag = &vertex.tag;
        let props: Vec<(String, Value)> = tag
            .properties
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        if props.is_empty() {
            return Ok(());
        }

        let vid_value = Value::from(vertex.vid);
        if let Some(txn_id) = txn_id {
            sync_manager
                .on_vertex_change_with_txn(
                    txn_id,
                    space_id,
                    &tag.name,
                    &vid_value,
                    &props,
                    ChangeType::Insert,
                )
                .map_err(|e| {
                    StorageError::db_error(format!("Failed to sync vertex insert: {}", e))
                })?;
        } else {
            return Err(StorageError::db_error(
                "Synchronized writes require an operation transaction context".to_string(),
            ));
        }

        Ok(())
    }

    pub(super) fn sync_update_vertex(
        &mut self,
        space: &str,
        old_vertex: &Vertex,
        new_vertex: &Vertex,
    ) -> Result<(), StorageError> {
        if !self.enabled {
            return Ok(());
        }

        let Some(sync_manager) = self.get_sync_manager() else {
            return Ok(());
        };

        let space_id = self.inner.get_space_id(space)?;
        let txn_id = self.get_current_txn_id();

        let tag = &new_vertex.tag;
        let changed_props = Self::detect_changed_properties(old_vertex, new_vertex);

        if changed_props.is_empty() {
            return Ok(());
        }

        let vid_value = Value::from(new_vertex.vid);
        if let Some(txn_id) = txn_id {
            sync_manager
                .on_vertex_change_with_txn(
                    txn_id,
                    space_id,
                    &tag.name,
                    &vid_value,
                    &changed_props,
                    ChangeType::Update,
                )
                .map_err(|e| {
                    StorageError::db_error(format!("Failed to sync vertex update: {}", e))
                })?;
        } else {
            return Err(StorageError::db_error(
                "Synchronized writes require an operation transaction context".to_string(),
            ));
        }

        Ok(())
    }

    pub(super) fn sync_delete_vertex(
        &mut self,
        space: &str,
        id: &VertexId,
        vertex: &Vertex,
    ) -> Result<(), StorageError> {
        if !self.enabled {
            return Ok(());
        }

        let Some(sync_manager) = self.get_sync_manager() else {
            return Ok(());
        };

        let space_id = self.inner.get_space_id(space)?;
        let txn_id = self.get_current_txn_id();
        let id_value = Value::from(*id);

        let tag = &vertex.tag;
        let props: Vec<(String, Value)> = tag
            .properties
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        if let Some(txn_id) = txn_id {
            sync_manager
                .on_vertex_change_with_txn(
                    txn_id,
                    space_id,
                    &tag.name,
                    &id_value,
                    &props,
                    ChangeType::Delete,
                )
                .map_err(|e| {
                    StorageError::db_error(format!("Failed to sync vertex delete: {}", e))
                })?;
        } else {
            return Err(StorageError::db_error(
                "Synchronized writes require an operation transaction context".to_string(),
            ));
        }

        Ok(())
    }

    pub(super) fn sync_batch_insert_vertices(
        &mut self,
        space: &str,
        vertices: &[Vertex],
    ) -> Result<(), StorageError> {
        if !self.enabled {
            return Ok(());
        }

        let Some(sync_manager) = self.get_sync_manager() else {
            return Ok(());
        };

        let space_id = self.inner.get_space_id(space)?;
        let txn_id = self.get_current_txn_id();

        for vertex in vertices {
            let tag = &vertex.tag;
            let props: Vec<(String, Value)> = tag
                .properties
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();

            if props.is_empty() {
                continue;
            }

            let vid_value = Value::from(vertex.vid);
            if let Some(txn_id) = txn_id {
                sync_manager
                    .on_vertex_change_with_txn(
                        txn_id,
                        space_id,
                        &tag.name,
                        &vid_value,
                        &props,
                        ChangeType::Insert,
                    )
                    .map_err(|e| {
                        StorageError::db_error(format!("Failed to sync vertex insert: {}", e))
                    })?;
            } else {
                return Err(StorageError::db_error(
                    "Synchronized writes require an operation transaction context".to_string(),
                ));
            }
        }

        Ok(())
    }
}
