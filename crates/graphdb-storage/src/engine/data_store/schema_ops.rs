//! Catalog schema mutations: register, drop and rename of vertex/edge types.

use std::sync::Arc;

use parking_lot::RwLock;

use crate::edge::EdgeStore;
use crate::vertex::ShardedVertexTable;
use graphdb_core::types::LabelId;
use graphdb_core::{StorageError, StorageResult};

use super::GraphDataStore;

impl GraphDataStore {
    pub(crate) fn register_vertex_type(
        &self,
        storage_name: String,
        requested_label: Option<LabelId>,
        table: impl FnOnce(LabelId) -> StorageResult<ShardedVertexTable>,
    ) -> StorageResult<LabelId> {
        let mut names = self.write_vertex_label_names();
        if names.contains_key(&storage_name) {
            return Err(StorageError::label_already_exists(storage_name));
        }
        let mut counter = self.write_vertex_counter();
        let mut tables = self.write_vertex_tables();
        let label = requested_label.unwrap_or(*counter);
        if tables.contains_key(&label) {
            return Err(StorageError::label_already_exists(format!(
                "label_id {}",
                label
            )));
        }
        *counter = (*counter).max(label.saturating_add(1));
        tables.insert(label, Arc::new(table(label)?));
        names.insert(storage_name, label);
        Ok(label)
    }

    pub(crate) fn register_edge_type(
        &self,
        storage_name: String,
        requested_label: Option<LabelId>,
        src_label: LabelId,
        dst_label: LabelId,
        table: impl FnOnce(LabelId) -> StorageResult<EdgeStore>,
    ) -> StorageResult<LabelId> {
        let mut names = self.write_edge_label_names();
        if names.contains_key(&storage_name) {
            return Err(StorageError::label_already_exists(storage_name));
        }
        let mut counter = self.write_edge_counter();
        let vertex_tables = self.read_vertex_tables();
        if src_label != 0 && !vertex_tables.contains_key(&src_label) {
            return Err(StorageError::label_not_found(format!(
                "source label {}",
                src_label
            )));
        }
        if dst_label != 0 && !vertex_tables.contains_key(&dst_label) {
            return Err(StorageError::label_not_found(format!(
                "destination label {}",
                dst_label
            )));
        }
        let mut edge_tables = self.write_edge_tables();
        let mut index = self.write_edge_label_index();
        let label = requested_label.unwrap_or(*counter);
        let key = super::EdgeTableKey::new(src_label, dst_label, label);
        if edge_tables.contains_key(&key) {
            return Err(StorageError::label_already_exists(format!(
                "label_id {}",
                label
            )));
        }
        *counter = (*counter).max(label.saturating_add(1));
        edge_tables.insert(key, Arc::new(RwLock::new(table(label)?)));
        index.entry(label).or_default().push(key);
        names.insert(storage_name, label);
        Ok(label)
    }

    pub(crate) fn drop_vertex_type(&self, name: &str) -> StorageResult<LabelId> {
        let mut vertex_names = self.write_vertex_label_names();
        let mut edge_names = self.write_edge_label_names();
        let mut vertex_tables = self.write_vertex_tables();
        let mut edge_tables = self.write_edge_tables();
        let mut edge_index = self.write_edge_label_index();
        let label = *vertex_names
            .get(name)
            .ok_or_else(|| StorageError::label_not_found(name.to_string()))?;
        vertex_names.remove(name);
        vertex_tables.remove(&label);
        let keys: Vec<_> = edge_tables
            .keys()
            .filter(|key| key.src_label == label || key.dst_label == label)
            .copied()
            .collect();
        for key in keys {
            edge_tables.remove(&key);
            if let Some(indexed_keys) = edge_index.get_mut(&key.edge_label) {
                indexed_keys.retain(|candidate| *candidate != key);
                if indexed_keys.is_empty() {
                    edge_index.remove(&key.edge_label);
                    edge_names.retain(|_, candidate| *candidate != key.edge_label);
                }
            }
        }
        Ok(label)
    }

    pub(crate) fn drop_edge_type(&self, name: &str) -> StorageResult<LabelId> {
        let mut names = self.write_edge_label_names();
        let mut tables = self.write_edge_tables();
        let mut index = self.write_edge_label_index();
        let label = *names
            .get(name)
            .ok_or_else(|| StorageError::label_not_found(name.to_string()))?;
        names.remove(name);
        for key in index.remove(&label).unwrap_or_default() {
            tables.remove(&key);
        }
        Ok(label)
    }

    pub(crate) fn rename_vertex_label_by_id(
        &self,
        label: LabelId,
        new_name: &str,
    ) -> StorageResult<()> {
        let mut names = self.write_vertex_label_names();
        let old_name = names
            .iter()
            .find(|(_, id)| **id == label)
            .map(|(name, _)| name.clone())
            .ok_or_else(|| StorageError::label_not_found(format!("label {}", label)))?;
        if names.contains_key(new_name) {
            return Err(StorageError::db_error(format!(
                "Vertex label \"{}\" already exists",
                new_name
            )));
        }
        names.remove(&old_name);
        names.insert(new_name.to_string(), label);
        Ok(())
    }

    pub(crate) fn rename_edge_label_by_id(
        &self,
        label: LabelId,
        new_name: &str,
    ) -> StorageResult<()> {
        let mut names = self.write_edge_label_names();
        let old_name = names
            .iter()
            .find(|(_, id)| **id == label)
            .map(|(name, _)| name.clone())
            .ok_or_else(|| StorageError::label_not_found(format!("edge label {}", label)))?;
        if names.contains_key(new_name) {
            return Err(StorageError::db_error(format!(
                "Edge label \"{}\" already exists",
                new_name
            )));
        }
        names.remove(&old_name);
        names.insert(new_name.to_string(), label);
        Ok(())
    }

    pub(crate) fn drop_vertex_type_by_label(&self, label: LabelId) -> StorageResult<()> {
        let mut vertex_names = self.write_vertex_label_names();
        let mut edge_names = self.write_edge_label_names();
        let mut vertex_tables = self.write_vertex_tables();
        let mut edge_tables = self.write_edge_tables();
        let mut edge_index = self.write_edge_label_index();
        if vertex_tables.remove(&label).is_none() {
            return Err(StorageError::label_not_found(format!(
                "vertex label {}",
                label
            )));
        }
        vertex_names.retain(|_, candidate| *candidate != label);
        let keys: Vec<_> = edge_tables
            .keys()
            .filter(|key| key.src_label == label || key.dst_label == label)
            .copied()
            .collect();
        for key in keys {
            edge_tables.remove(&key);
            if let Some(indexed_keys) = edge_index.get_mut(&key.edge_label) {
                indexed_keys.retain(|candidate| *candidate != key);
                if indexed_keys.is_empty() {
                    edge_index.remove(&key.edge_label);
                    edge_names.retain(|_, candidate| *candidate != key.edge_label);
                }
            }
        }
        Ok(())
    }

    pub(crate) fn drop_edge_partition(&self, key: super::EdgeTableKey) -> StorageResult<()> {
        let mut names = self.write_edge_label_names();
        let mut tables = self.write_edge_tables();
        let mut index = self.write_edge_label_index();
        if tables.remove(&key).is_none() {
            return Ok(());
        }
        if let Some(keys) = index.get_mut(&key.edge_label) {
            keys.retain(|candidate| *candidate != key);
            if keys.is_empty() {
                index.remove(&key.edge_label);
                names.retain(|_, label| *label != key.edge_label);
            }
        }
        Ok(())
    }
}
