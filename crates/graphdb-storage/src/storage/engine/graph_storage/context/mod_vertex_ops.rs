use std::sync::atomic::Ordering;
use crate::core::types::{LabelId, Timestamp, VertexId};
use crate::core::{StorageError, StorageResult, Value};
use crate::storage::vertex::VertexRecord;

use super::GraphStorageContext;

impl GraphStorageContext {
    pub fn insert_vertex(
        &self,
        label: LabelId,
        external_id: &str,
        properties: &[(String, Value)],
        ts: Timestamp,
    ) -> StorageResult<u32> {
        if !self.persistent.is_open.load(Ordering::Acquire) {
            return Err(StorageError::storage_not_open());
        }
        let mut vertex_tables = self.persistent.data_store.vertex_tables().write();
        let table = vertex_tables
            .get_mut(&label)
            .ok_or_else(|| StorageError::label_not_found(format!("vertex label {}", label)))?;

        let internal_id = table.insert(external_id, properties, ts)?;

        self.persistent
            .cache_manager
            .cache_vertex_id(label, external_id, internal_id, ts);
        self.mark_vertex_modified(label);

        Ok(internal_id)
    }

    pub fn insert_vertex_by_i64(
        &self,
        label: LabelId,
        external_id: i64,
        properties: &[(String, Value)],
        ts: Timestamp,
    ) -> StorageResult<u32> {
        if !self.persistent.is_open.load(Ordering::Acquire) {
            return Err(StorageError::storage_not_open());
        }
        let mut vertex_tables = self.persistent.data_store.vertex_tables().write();
        let table = vertex_tables
            .get_mut(&label)
            .ok_or_else(|| StorageError::label_not_found(format!("vertex label {}", label)))?;

        let internal_id = table.insert_by_i64(external_id, properties, ts)?;

        self.persistent.cache_manager.cache_vertex_id(
            label,
            &external_id.to_string(),
            internal_id,
            ts,
        );
        self.mark_vertex_modified(label);

        Ok(internal_id)
    }

    pub fn get_vertex(
        &self,
        label: LabelId,
        external_id: &str,
        ts: Timestamp,
    ) -> Option<VertexRecord> {
        if !self.persistent.is_open.load(Ordering::Acquire) {
            return None;
        }

        let internal_id = self
            .persistent
            .cache_manager
            .get_cached_vertex_id(label, external_id, ts)
            .or_else(|| {
                let id = {
                    let vertex_tables = self.persistent.data_store.vertex_tables().read();
                    vertex_tables.get(&label)?.get_internal_id(external_id, ts)
                };
                if let Some(id) = id {
                    self.persistent
                        .cache_manager
                        .cache_vertex_id(label, external_id, id, ts);
                }
                id
            })?;

        if let Some(cached) =
            self.persistent
                .cache_manager
                .get_cached_vertex(label, internal_id, ts)
        {
            return Some(VertexRecord {
                internal_id: cached.internal_id,
                vid: cached
                    .external_id
                    .parse::<i64>()
                    .map(crate::core::types::VertexId::from_int64)
                    .unwrap_or_else(|_| {
                        crate::core::types::VertexId::from_string(&cached.external_id)
                    }),
                properties: cached.properties,
            });
        }

        let record = {
            let vertex_tables = self.persistent.data_store.vertex_tables().read();
            vertex_tables
                .get(&label)?
                .get_by_internal_id(internal_id, ts)?
        };

        self.persistent.cache_manager.cache_vertex(
            label,
            internal_id,
            external_id.to_string(),
            record.properties.clone(),
            ts,
        );

        Some(record)
    }

    pub fn get_vertex_by_i64(
        &self,
        label: LabelId,
        external_id: i64,
        ts: Timestamp,
    ) -> Option<VertexRecord> {
        if !self.persistent.is_open.load(Ordering::Acquire) {
            return None;
        }

        let external_id_str = external_id.to_string();
        let internal_id = self
            .persistent
            .cache_manager
            .get_cached_vertex_id(label, &external_id_str, ts)
            .or_else(|| {
                let id = {
                    let vertex_tables = self.persistent.data_store.vertex_tables().read();
                    vertex_tables
                        .get(&label)?
                        .get_internal_id_by_i64(external_id, ts)
                };
                if let Some(id) = id {
                    self.persistent
                        .cache_manager
                        .cache_vertex_id(label, &external_id_str, id, ts);
                }
                id
            })?;

        if let Some(cached) =
            self.persistent
                .cache_manager
                .get_cached_vertex(label, internal_id, ts)
        {
            return Some(VertexRecord {
                internal_id: cached.internal_id,
                vid: crate::core::types::VertexId::from_int64(external_id),
                properties: cached.properties,
            });
        }

        let record = {
            let vertex_tables = self.persistent.data_store.vertex_tables().read();
            vertex_tables
                .get(&label)?
                .get_by_internal_id(internal_id, ts)?
        };

        self.persistent.cache_manager.cache_vertex(
            label,
            internal_id,
            external_id_str,
            record.properties.clone(),
            ts,
        );

        Some(record)
    }

    pub fn get_vertex_by_internal_id(
        &self,
        label: LabelId,
        internal_id: u32,
        ts: Timestamp,
    ) -> Option<VertexRecord> {
        if !self.persistent.is_open.load(Ordering::Acquire) {
            return None;
        }

        if let Some(cached) =
            self.persistent
                .cache_manager
                .get_cached_vertex(label, internal_id, ts)
        {
            return Some(VertexRecord {
                internal_id: cached.internal_id,
                vid: cached
                    .external_id
                    .parse::<i64>()
                    .map(crate::core::types::VertexId::from_int64)
                    .unwrap_or_else(|_| {
                        crate::core::types::VertexId::from_string(&cached.external_id)
                    }),
                properties: cached.properties,
            });
        }

        let record = {
            let vertex_tables = self.persistent.data_store.vertex_tables().read();
            vertex_tables
                .get(&label)?
                .get_by_internal_id(internal_id, ts)?
        };

        let external_id = {
            let vertex_tables = self.persistent.data_store.vertex_tables().read();
            vertex_tables
                .get(&label)?
                .get_external_id(internal_id, ts)
                .map(|k| k.to_string())
                .unwrap_or_default()
        };

        if !external_id.is_empty() {
            self.persistent
                .cache_manager
                .cache_vertex_id(label, &external_id, internal_id, ts);
        }

        self.persistent.cache_manager.cache_vertex(
            label,
            internal_id,
            external_id,
            record.properties.clone(),
            ts,
        );

        Some(record)
    }

    pub fn get_external_id(
        &self,
        label: LabelId,
        internal_id: u32,
        ts: Timestamp,
    ) -> Option<String> {
        let vertex_tables = self.persistent.data_store.vertex_tables().read();
        vertex_tables
            .get(&label)?
            .get_external_id(internal_id, ts)
            .map(|k| k.to_string())
    }

    pub fn get_external_id_any(&self, internal_id: u32, ts: Timestamp) -> Option<String> {
        let vertex_tables = self.persistent.data_store.vertex_tables().read();
        vertex_tables
            .values()
            .find_map(|t| t.get_external_id(internal_id, ts))
            .map(|k| k.to_string())
    }

    pub fn get_external_id_by_internal_id(
        &self,
        label: LabelId,
        internal_id: u32,
    ) -> Option<VertexId> {
        let vertex_tables = self.persistent.data_store.vertex_tables().read();
        let table = vertex_tables.get(&label)?;
        let key = table.get_external_id_raw(internal_id)?;
        Some(match key {
            crate::storage::vertex::IdKey::Int(i) => VertexId::from_int64(i),
            crate::storage::vertex::IdKey::Text(s) => VertexId::from_string(s),
        })
    }

    pub fn delete_vertex(
        &self,
        label: LabelId,
        external_id: &str,
        ts: Timestamp,
    ) -> StorageResult<()> {
        if !self.persistent.is_open.load(Ordering::Acquire) {
            return Err(StorageError::storage_not_open());
        }

        let mut vertex_tables = self.persistent.data_store.vertex_tables().write();
        let table = vertex_tables
            .get_mut(&label)
            .ok_or_else(|| StorageError::label_not_found(format!("vertex label {}", label)))?;

        let internal_id = table.get_internal_id(external_id, ts);
        table.delete(external_id, ts)?;

        self.persistent
            .cache_manager
            .remove_cached_vertex_id(label, external_id);
        if let Some(id) = internal_id {
            self.persistent
                .cache_manager
                .remove_cached_vertex(label, id);
        }
        self.mark_vertex_modified(label);

        Ok(())
    }

    pub fn delete_vertex_by_i64(
        &self,
        label: LabelId,
        external_id: i64,
        ts: Timestamp,
    ) -> StorageResult<()> {
        if !self.persistent.is_open.load(Ordering::Acquire) {
            return Err(StorageError::storage_not_open());
        }

        let mut vertex_tables = self.persistent.data_store.vertex_tables().write();
        let table = vertex_tables
            .get_mut(&label)
            .ok_or_else(|| StorageError::label_not_found(format!("vertex label {}", label)))?;

        let internal_id = table.get_internal_id_by_i64(external_id, ts);
        let external_id_str = external_id.to_string();
        table.delete_by_i64(external_id, ts)?;

        self.persistent
            .cache_manager
            .remove_cached_vertex_id(label, &external_id_str);
        if let Some(id) = internal_id {
            self.persistent
                .cache_manager
                .remove_cached_vertex(label, id);
        }
        self.mark_vertex_modified(label);

        Ok(())
    }

    pub fn update_vertex_property(
        &self,
        label: LabelId,
        external_id: &str,
        property_name: &str,
        value: &Value,
        ts: Timestamp,
    ) -> StorageResult<()> {
        if !self.persistent.is_open.load(Ordering::Acquire) {
            return Err(StorageError::storage_not_open());
        }

        let mut vertex_tables = self.persistent.data_store.vertex_tables().write();
        let table = vertex_tables
            .get_mut(&label)
            .ok_or_else(|| StorageError::label_not_found(format!("vertex label {}", label)))?;

        let internal_id = table
            .get_internal_id(external_id, ts)
            .ok_or(StorageError::vertex_not_found())?;

        table.update_property(internal_id, property_name, value, ts)?;

        self.persistent
            .cache_manager
            .remove_cached_vertex(label, internal_id);
        self.mark_vertex_modified(label);

        Ok(())
    }

    pub fn update_vertex_property_by_i64(
        &self,
        label: LabelId,
        external_id: i64,
        property_name: &str,
        value: &Value,
        ts: Timestamp,
    ) -> StorageResult<()> {
        if !self.persistent.is_open.load(Ordering::Acquire) {
            return Err(StorageError::storage_not_open());
        }

        let mut vertex_tables = self.persistent.data_store.vertex_tables().write();
        let table = vertex_tables
            .get_mut(&label)
            .ok_or_else(|| StorageError::label_not_found(format!("vertex label {}", label)))?;

        let internal_id = table
            .get_internal_id_by_i64(external_id, ts)
            .ok_or(StorageError::vertex_not_found())?;

        table.update_property(internal_id, property_name, value, ts)?;

        self.persistent
            .cache_manager
            .remove_cached_vertex(label, internal_id);
        self.mark_vertex_modified(label);

        Ok(())
    }
}
