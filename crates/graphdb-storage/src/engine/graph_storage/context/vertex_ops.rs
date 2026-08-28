use graphdb_core::types::{LabelId, Timestamp, VertexId};
use graphdb_core::{StorageError, StorageResult, Value};
use crate::vertex::VertexRecord;
use std::sync::atomic::Ordering;

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

        // Lazily register snapshot for this vertex label if needed
        self.ensure_vertex_snapshot_registered(label);

        let internal_id = self
            .persistent
            .data_store
            .with_vertex_tables_mut(|vertex_tables| {
                let table = vertex_tables.get(&label).ok_or_else(|| {
                    StorageError::label_not_found(format!("vertex label {}", label))
                })?;
                table.insert(external_id, properties, ts)
            })?;

        self.persistent
            .cache_manager
            .cache_vertex_id(label, external_id, internal_id, ts);
        self.mark_vertex_modified(label);
        self.observe_vertex_id_string(label);

        Ok(internal_id)
    }

    pub fn insert_vertex_by_i64(
        &self,
        label: LabelId,
        external_id: i64,
        properties: &[(String, Value)],
        ts: Timestamp,
    ) -> StorageResult<u32> {
        if external_id < 0 {
            return Err(StorageError::invalid_input(format!(
                "Vertex id cannot be negative: {}",
                external_id
            )));
        }
        if !self.persistent.is_open.load(Ordering::Acquire) {
            return Err(StorageError::storage_not_open());
        }
        let internal_id = self
            .persistent
            .data_store
            .with_vertex_tables_mut(|vertex_tables| {
                let table = vertex_tables.get(&label).ok_or_else(|| {
                    StorageError::label_not_found(format!("vertex label {}", label))
                })?;
                table.insert_by_i64(external_id, properties, ts)
            })?;

        self.persistent.cache_manager.cache_vertex_id(
            label,
            &external_id.to_string(),
            internal_id,
            ts,
        );
        self.mark_vertex_modified(label);
        self.observe_vertex_id_i64(label, external_id);

        Ok(internal_id)
    }

    /// Pre-allocate capacity for `additional` more vertices in the given label's table.
    /// Call before batch inserts to avoid repeated hash rehashing.
    pub fn reserve_vertex_capacity(&self, label: LabelId, additional: usize) {
        let _ = self
            .persistent
            .data_store
            .with_vertex_tables_mut(|vertex_tables| {
                if let Some(table) = vertex_tables.get(&label) {
                    table.reserve_id_capacity(additional);
                }
                Ok(())
            });
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
                let id = self
                    .persistent
                    .data_store
                    .with_vertex_tables(|vertex_tables| {
                        vertex_tables.get(&label)?.get_internal_id(external_id, ts)
                    });
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
                    .map(graphdb_core::types::VertexId::from_int64)
                    .unwrap_or_else(|_| {
                        graphdb_core::types::VertexId::from_string(&cached.external_id)
                    }),
                properties: cached.properties,
            });
        }

        let record = self.read_record(label, internal_id, None, ts)?;

        self.persistent.cache_manager.cache_vertex(
            label,
            internal_id,
            external_id.to_string(),
            record.properties.clone(),
            ts,
        );

        Some(record)
    }

    /// Read a vertex record by internal ID, optionally restricted to a
    /// property projection. Never consults or populates the full-record cache
    /// (a projected read must not poison it with partial properties).
    fn read_record(
        &self,
        label: LabelId,
        internal_id: u32,
        projection: Option<&[String]>,
        ts: Timestamp,
    ) -> Option<VertexRecord> {
        // Lazily register the statement snapshot for this label (MVCC GC
        // coordination for read-only statement contexts).
        self.ensure_vertex_snapshot_registered(label);
        self.persistent
            .data_store
            .with_vertex_tables(|vertex_tables| -> Option<VertexRecord> {
                let table = vertex_tables.get(&label)?;
                match projection {
                    Some(proj) => table.get_projected_by_internal_id(internal_id, ts, Some(proj)),
                    None => table.get_by_internal_id(internal_id, ts),
                }
            })
    }

    /// Fetch a vertex restricted to the given property projection, skipping
    /// the full-record cache so partial results never replace cached vertices.
    pub fn get_vertex_projected(
        &self,
        label: LabelId,
        external_id: &str,
        projection: &[String],
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
                let id = self
                    .persistent
                    .data_store
                    .with_vertex_tables(|vertex_tables| {
                        vertex_tables.get(&label)?.get_internal_id(external_id, ts)
                    });
                if let Some(id) = id {
                    self.persistent
                        .cache_manager
                        .cache_vertex_id(label, external_id, id, ts);
                }
                id
            })?;

        self.read_record(label, internal_id, Some(projection), ts)
    }

    pub fn get_vertex_by_i64_projected(
        &self,
        label: LabelId,
        external_id: i64,
        projection: &[String],
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
                let id = self
                    .persistent
                    .data_store
                    .with_vertex_tables(|vertex_tables| {
                        vertex_tables
                            .get(&label)?
                            .get_internal_id_by_i64(external_id, ts)
                    });
                if let Some(id) = id {
                    self.persistent
                        .cache_manager
                        .cache_vertex_id(label, &external_id_str, id, ts);
                }
                id
            })?;

        self.read_record(label, internal_id, Some(projection), ts)
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

        // Lazily register the statement snapshot for this label.
        self.ensure_vertex_snapshot_registered(label);

        let external_id_str = external_id.to_string();
        let internal_id = self
            .persistent
            .cache_manager
            .get_cached_vertex_id(label, &external_id_str, ts)
            .or_else(|| {
                let id = self
                    .persistent
                    .data_store
                    .with_vertex_tables(|vertex_tables| {
                        vertex_tables
                            .get(&label)?
                            .get_internal_id_by_i64(external_id, ts)
                    });
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
                vid: graphdb_core::types::VertexId::from_int64(external_id),
                properties: cached.properties,
            });
        }

        let record = self.persistent.data_store.with_vertex_tables(
            |vertex_tables| -> Option<VertexRecord> {
                vertex_tables
                    .get(&label)?
                    .get_by_internal_id(internal_id, ts)
            },
        )?;

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

        // Lazily register snapshot for this vertex label if needed
        self.ensure_vertex_snapshot_registered(label);

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
                    .map(graphdb_core::types::VertexId::from_int64)
                    .unwrap_or_else(|_| {
                        graphdb_core::types::VertexId::from_string(&cached.external_id)
                    }),
                properties: cached.properties,
            });
        }

        let record = self.persistent.data_store.with_vertex_tables(
            |vertex_tables| -> Option<VertexRecord> {
                vertex_tables
                    .get(&label)?
                    .get_by_internal_id(internal_id, ts)
            },
        )?;

        let external_id = self
            .persistent
            .data_store
            .with_vertex_tables(|vertex_tables| -> Option<String> {
                vertex_tables
                    .get(&label)
                    .and_then(|table| table.get_external_id(internal_id, ts))
                    .map(|k| k.to_string())
            })
            .unwrap_or_default();

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
        // Lazily register the statement snapshot for this label.
        self.ensure_vertex_snapshot_registered(label);
        self.persistent
            .data_store
            .with_vertex_tables(|vertex_tables| {
                vertex_tables
                    .get(&label)?
                    .get_external_id(internal_id, ts)
                    .map(|k| k.to_string())
            })
    }

    pub fn get_external_id_any(&self, internal_id: u32, ts: Timestamp) -> Option<String> {
        // Lazily register the statement snapshot for every vertex label.
        let labels: Vec<LabelId> = self
            .persistent
            .data_store
            .with_vertex_tables(|tables| tables.keys().copied().collect());
        for label in labels {
            self.ensure_vertex_snapshot_registered(label);
        }
        self.persistent
            .data_store
            .with_vertex_tables(|vertex_tables| {
                vertex_tables
                    .values()
                    .find_map(|t| t.get_external_id(internal_id, ts))
                    .map(|k| k.to_string())
            })
    }
    pub fn get_external_id_by_internal_id(
        &self,
        label: LabelId,
        internal_id: u32,
    ) -> Option<VertexId> {
        self.persistent
            .data_store
            .with_vertex_tables(|vertex_tables| {
                let table = vertex_tables.get(&label)?;
                let key = table.get_external_id_raw(internal_id)?;
                Some(match key {
                    crate::vertex::IdKey::Int(i) => VertexId::from_int64(i),
                    crate::vertex::IdKey::Text(s) => VertexId::from_string(s),
                })
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

        let internal_id = self
            .persistent
            .data_store
            .with_vertex_tables_mut(|vertex_tables| {
                let table = vertex_tables.get(&label).ok_or_else(|| {
                    StorageError::label_not_found(format!("vertex label {}", label))
                })?;
                let internal_id = table.get_internal_id(external_id, ts);
                table.delete(external_id, ts)?;
                Ok(internal_id)
            })?;

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

        let external_id_str = external_id.to_string();
        let internal_id = self
            .persistent
            .data_store
            .with_vertex_tables_mut(|vertex_tables| {
                let table = vertex_tables.get(&label).ok_or_else(|| {
                    StorageError::label_not_found(format!("vertex label {}", label))
                })?;
                let internal_id = table.get_internal_id_by_i64(external_id, ts);
                table.delete_by_i64(external_id, ts)?;
                Ok(internal_id)
            })?;

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

    pub fn batch_delete_vertices(
        &self,
        label: LabelId,
        external_ids: &[&str],
        ts: Timestamp,
    ) -> StorageResult<usize> {
        if !self.persistent.is_open.load(Ordering::Acquire) {
            return Err(StorageError::storage_not_open());
        }

        let count = self
            .persistent
            .data_store
            .with_vertex_tables_mut(|vertex_tables| {
                let table = vertex_tables.get(&label).ok_or_else(|| {
                    StorageError::label_not_found(format!("vertex label {}", label))
                })?;
                table.batch_delete(external_ids, ts)
            })?;

        for external_id in external_ids {
            self.persistent
                .cache_manager
                .remove_cached_vertex_id(label, external_id);
        }
        self.mark_vertex_modified(label);

        Ok(count)
    }

    pub fn batch_delete_vertices_by_i64(
        &self,
        label: LabelId,
        external_ids: &[i64],
        ts: Timestamp,
    ) -> StorageResult<usize> {
        if !self.persistent.is_open.load(Ordering::Acquire) {
            return Err(StorageError::storage_not_open());
        }

        let count = self
            .persistent
            .data_store
            .with_vertex_tables_mut(|vertex_tables| {
                let table = vertex_tables.get(&label).ok_or_else(|| {
                    StorageError::label_not_found(format!("vertex label {}", label))
                })?;
                table.batch_delete_i64(external_ids, ts)
            })?;

        for external_id in external_ids {
            self.persistent
                .cache_manager
                .remove_cached_vertex_id(label, &external_id.to_string());
        }
        self.mark_vertex_modified(label);

        Ok(count)
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

        let internal_id = self
            .persistent
            .data_store
            .with_vertex_tables_mut(|vertex_tables| {
                let table = vertex_tables.get(&label).ok_or_else(|| {
                    StorageError::label_not_found(format!("vertex label {}", label))
                })?;
                let internal_id = table
                    .get_internal_id(external_id, ts)
                    .ok_or(StorageError::vertex_not_found())?;
                table.update_property(internal_id, property_name, value, ts)?;
                Ok(internal_id)
            })?;

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

        let internal_id = self
            .persistent
            .data_store
            .with_vertex_tables_mut(|vertex_tables| {
                let table = vertex_tables.get(&label).ok_or_else(|| {
                    StorageError::label_not_found(format!("vertex label {}", label))
                })?;
                let internal_id = table
                    .get_internal_id_by_i64(external_id, ts)
                    .ok_or(StorageError::vertex_not_found())?;
                table.update_property(internal_id, property_name, value, ts)?;
                Ok(internal_id)
            })?;

        self.persistent
            .cache_manager
            .remove_cached_vertex(label, internal_id);
        self.mark_vertex_modified(label);

        Ok(())
    }
}
