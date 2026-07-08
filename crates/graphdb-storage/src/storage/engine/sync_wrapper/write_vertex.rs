use super::SyncWrapper;
use crate::core::types::VertexId;
use crate::core::{StorageError, Value, Vertex};
use crate::storage::{StorageClient, StorageTransactionContextOps};
use crate::sync::types::ChangeType;

impl<S: StorageClient + StorageTransactionContextOps + 'static> SyncWrapper<S> {
    fn detect_changed_properties(
        tag_name: &str,
        old_vertex: &Vertex,
        new_vertex: &Vertex,
    ) -> Vec<(String, Value)> {
        let mut changed_props = Vec::new();

        let old_tag = old_vertex.tags.iter().find(|t| t.name == tag_name);
        let new_tag = new_vertex.tags.iter().find(|t| t.name == tag_name);

        match (old_tag, new_tag) {
            (Some(old_tag), Some(new_tag)) => {
                for (prop_name, new_value) in &new_tag.properties {
                    match old_tag.properties.get(prop_name) {
                        Some(old_value) if old_value != new_value => {
                            changed_props.push((prop_name.clone(), new_value.clone()));
                        }
                        None => {
                            changed_props.push((prop_name.clone(), new_value.clone()));
                        }
                        _ => {}
                    }
                }
            }
            (None, Some(new_tag)) => {
                for (prop_name, value) in &new_tag.properties {
                    changed_props.push((prop_name.clone(), value.clone()));
                }
            }
            _ => {}
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

        for tag in &vertex.tags {
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
                sync_manager
                    .on_vertex_change_direct_sync(
                        space_id,
                        &tag.name,
                        &vid_value,
                        &props,
                        ChangeType::Insert,
                    )
                    .map_err(|e| {
                        StorageError::db_error(format!("Failed to sync vertex insert: {}", e))
                    })?;
            }
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

        for tag in &new_vertex.tags {
            let changed_props = Self::detect_changed_properties(&tag.name, old_vertex, new_vertex);

            if changed_props.is_empty() {
                continue;
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
                sync_manager
                    .on_vertex_change_direct_sync(
                        space_id,
                        &tag.name,
                        &vid_value,
                        &changed_props,
                        ChangeType::Update,
                    )
                    .map_err(|e| {
                        StorageError::db_error(format!("Failed to sync vertex update: {}", e))
                    })?;
            }
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

        for tag in &vertex.tags {
            let props: Vec<(String, Value)> = tag
                .properties
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();

            if props.is_empty() {
                continue;
            }

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
                sync_manager
                    .on_vertex_change_direct_sync(
                        space_id,
                        &tag.name,
                        &id_value,
                        &props,
                        ChangeType::Delete,
                    )
                    .map_err(|e| {
                        StorageError::db_error(format!("Failed to sync vertex delete: {}", e))
                    })?;
            }
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
            for tag in &vertex.tags {
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
                    sync_manager
                        .on_vertex_change_direct_sync(
                            space_id,
                            &tag.name,
                            &vid_value,
                            &props,
                            ChangeType::Insert,
                        )
                        .map_err(|e| {
                            StorageError::db_error(format!("Failed to sync vertex insert: {}", e))
                        })?;
                }
            }
        }

        Ok(())
    }
}
