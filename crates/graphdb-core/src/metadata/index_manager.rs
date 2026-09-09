use super::schema_events::{SchemaChangeCallback, SchemaChangeEvent};
use crate::event_dispatch::{EventFilter, EventSubscriptions, SubscriptionId};
use crate::types::Index;
use crate::StorageError;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

const INDEX_FORMAT_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct IndexSnapshot {
    version: u32,
    tag_indexes: Vec<(u64, String, Index)>,
    edge_indexes: Vec<(u64, String, Index)>,
}

use crate::types::IndexStatus;

pub trait IndexMetadataManager: Send + Sync + std::fmt::Debug {
    fn create_tag_index(&self, space_id: u64, index: &Index) -> Result<bool, StorageError>;
    fn drop_tag_index(&self, space_id: u64, index_name: &str) -> Result<bool, StorageError>;
    fn get_tag_index(&self, space_id: u64, index_name: &str)
        -> Result<Option<Index>, StorageError>;
    fn list_tag_indexes(&self, space_id: u64) -> Result<Vec<Index>, StorageError>;
    fn drop_tag_indexes_by_tag(&self, space_id: u64, tag_name: &str) -> Result<(), StorageError>;

    fn create_edge_index(&self, space_id: u64, index: &Index) -> Result<bool, StorageError>;
    fn drop_edge_index(&self, space_id: u64, index_name: &str) -> Result<bool, StorageError>;
    fn get_edge_index(
        &self,
        space_id: u64,
        index_name: &str,
    ) -> Result<Option<Index>, StorageError>;
    fn list_edge_indexes(&self, space_id: u64) -> Result<Vec<Index>, StorageError>;
    fn drop_edge_indexes_by_type(&self, space_id: u64, edge_type: &str)
        -> Result<(), StorageError>;

    /// Set the status of a tag index (used for generation rebuild lifecycle).
    fn set_tag_index_status(
        &self,
        space_id: u64,
        index_name: &str,
        status: IndexStatus,
    ) -> Result<bool, StorageError>;

    /// Set the status of an edge index (used for generation rebuild lifecycle).
    fn set_edge_index_status(
        &self,
        space_id: u64,
        index_name: &str,
        status: IndexStatus,
    ) -> Result<bool, StorageError>;
}

pub struct IndexManager {
    tag_indexes: Arc<RwLock<HashMap<(u64, String), Index>>>,
    edge_indexes: Arc<RwLock<HashMap<(u64, String), Index>>>,
    next_index_id: AtomicU64,
    schema_callbacks: Arc<EventSubscriptions<SchemaChangeEvent>>,
}

impl std::fmt::Debug for IndexManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IndexManager")
            .field("tag_indexes_count", &self.tag_indexes.read().len())
            .field("edge_indexes_count", &self.edge_indexes.read().len())
            .finish()
    }
}

impl IndexManager {
    pub fn new() -> Self {
        Self {
            tag_indexes: Arc::new(RwLock::new(HashMap::new())),
            edge_indexes: Arc::new(RwLock::new(HashMap::new())),
            next_index_id: AtomicU64::new(1),
            schema_callbacks: Arc::new(EventSubscriptions::new()),
        }
    }

    /// Register a runtime observer for index DDL events.
    pub fn register_schema_callback(&self, callback: SchemaChangeCallback) -> SubscriptionId {
        self.schema_callbacks.add(callback)
    }

    /// Register a filtered observer invoked only when `filter` returns true.
    pub fn register_schema_callback_filtered(
        &self,
        callback: SchemaChangeCallback,
        filter: EventFilter<SchemaChangeEvent>,
    ) -> SubscriptionId {
        self.schema_callbacks.add_filtered(callback, Some(filter))
    }

    /// Remove a previously registered observer. Returns true if present.
    pub fn unregister_schema_callback(&self, id: SubscriptionId) -> bool {
        self.schema_callbacks.remove(id)
    }

    /// Number of registered index observers.
    pub fn schema_callback_count(&self) -> usize {
        self.schema_callbacks.len()
    }

    fn emit_schema_event(&self, event: SchemaChangeEvent) {
        self.schema_callbacks.dispatch("schema", &event);
    }

    pub fn save_indexes(&self, path: &Path) -> Result<(), StorageError> {
        use std::fs::{self, File};
        use std::io::Write;

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| StorageError::io_error(e.to_string()))?;
        }

        let tag_indexes: Vec<(u64, String, Index)> = self
            .tag_indexes
            .read()
            .iter()
            .map(|((space_id, name), index)| (*space_id, name.clone(), index.clone()))
            .collect();

        let edge_indexes: Vec<(u64, String, Index)> = self
            .edge_indexes
            .read()
            .iter()
            .map(|((space_id, name), index)| (*space_id, name.clone(), index.clone()))
            .collect();

        let snapshot = IndexSnapshot {
            version: INDEX_FORMAT_VERSION,
            tag_indexes,
            edge_indexes,
        };

        let json = serde_json::to_string_pretty(&snapshot)
            .map_err(|e| StorageError::serialize_error(e.to_string()))?;

        let mut file = File::create(path).map_err(|e| StorageError::io_error(e.to_string()))?;
        file.write_all(json.as_bytes())
            .map_err(|e| StorageError::io_error(e.to_string()))?;

        Ok(())
    }

    pub fn load_indexes(&self, path: &Path) -> Result<(), StorageError> {
        use std::fs::File;
        use std::io::Read;

        if !path.exists() {
            return Ok(());
        }

        let mut file = File::open(path).map_err(|e| StorageError::io_error(e.to_string()))?;
        let mut json = String::new();
        file.read_to_string(&mut json)
            .map_err(|e| StorageError::io_error(e.to_string()))?;

        let snapshot: IndexSnapshot = serde_json::from_str(&json)
            .map_err(|e| StorageError::deserialize_error(e.to_string()))?;

        if snapshot.version > INDEX_FORMAT_VERSION {
            return Err(StorageError::deserialize_error(format!(
                "Index snapshot version {} is newer than supported version {}",
                snapshot.version, INDEX_FORMAT_VERSION
            )));
        }

        self.tag_indexes.write().clear();
        self.edge_indexes.write().clear();

        let mut max_id = 0;
        for (space_id, name, index) in snapshot.tag_indexes {
            if index.id > max_id {
                max_id = index.id;
            }
            self.tag_indexes.write().insert((space_id, name), index);
        }

        for (space_id, name, index) in snapshot.edge_indexes {
            if index.id > max_id {
                max_id = index.id;
            }
            self.edge_indexes.write().insert((space_id, name), index);
        }

        self.next_index_id.store(max_id + 1, Ordering::SeqCst);

        Ok(())
    }
}

impl Default for IndexManager {
    fn default() -> Self {
        Self::new()
    }
}

impl IndexMetadataManager for IndexManager {
    fn create_tag_index(&self, space_id: u64, index: &Index) -> Result<bool, StorageError> {
        let created = {
            let mut indexes = self.tag_indexes.write();
            let key = (space_id, index.name.clone());
            if indexes.contains_key(&key) {
                return Ok(false);
            }
            let mut index_with_space_id = index.clone();
            index_with_space_id.space_id = space_id;
            if index_with_space_id.id == 0 {
                index_with_space_id.id = self.next_index_id.fetch_add(1, Ordering::SeqCst);
            }
            indexes.insert(key, index_with_space_id);
            true
        };
        if created {
            self.emit_schema_event(SchemaChangeEvent::TagIndexCreated {
                space_id,
                index_name: index.name.clone(),
            });
        }
        Ok(created)
    }

    fn drop_tag_index(&self, space_id: u64, index_name: &str) -> Result<bool, StorageError> {
        let removed = {
            let mut indexes = self.tag_indexes.write();
            let key = (space_id, index_name.to_string());
            indexes.remove(&key).is_some()
        };
        if removed {
            self.emit_schema_event(SchemaChangeEvent::TagIndexDropped {
                space_id,
                index_name: index_name.to_string(),
            });
        }
        Ok(removed)
    }

    fn get_tag_index(
        &self,
        space_id: u64,
        index_name: &str,
    ) -> Result<Option<Index>, StorageError> {
        let indexes = self.tag_indexes.read();
        Ok(indexes.get(&(space_id, index_name.to_string())).cloned())
    }

    fn list_tag_indexes(&self, space_id: u64) -> Result<Vec<Index>, StorageError> {
        let indexes = self.tag_indexes.read();
        Ok(indexes
            .iter()
            .filter(|((sid, _), _)| *sid == space_id)
            .map(|(_, index)| index.clone())
            .collect())
    }

    fn drop_tag_indexes_by_tag(&self, space_id: u64, tag_name: &str) -> Result<(), StorageError> {
        let removed: Vec<String> = {
            let mut indexes = self.tag_indexes.write();
            let mut removed = Vec::new();
            indexes.retain(|_, index| {
                let drop_it = index.space_id == space_id && index.schema_name == tag_name;
                if drop_it {
                    removed.push(index.name.clone());
                }
                !drop_it
            });
            removed
        };
        for index_name in removed {
            self.emit_schema_event(SchemaChangeEvent::TagIndexDropped {
                space_id,
                index_name,
            });
        }
        Ok(())
    }

    fn create_edge_index(&self, space_id: u64, index: &Index) -> Result<bool, StorageError> {
        let created = {
            let mut indexes = self.edge_indexes.write();
            let key = (space_id, index.name.clone());
            if indexes.contains_key(&key) {
                return Ok(false);
            }
            let mut index_with_space_id = index.clone();
            index_with_space_id.space_id = space_id;
            if index_with_space_id.id == 0 {
                index_with_space_id.id = self.next_index_id.fetch_add(1, Ordering::SeqCst);
            }
            indexes.insert(key, index_with_space_id);
            true
        };
        if created {
            self.emit_schema_event(SchemaChangeEvent::EdgeIndexCreated {
                space_id,
                index_name: index.name.clone(),
            });
        }
        Ok(created)
    }

    fn drop_edge_index(&self, space_id: u64, index_name: &str) -> Result<bool, StorageError> {
        let removed = {
            let mut indexes = self.edge_indexes.write();
            let key = (space_id, index_name.to_string());
            indexes.remove(&key).is_some()
        };
        if removed {
            self.emit_schema_event(SchemaChangeEvent::EdgeIndexDropped {
                space_id,
                index_name: index_name.to_string(),
            });
        }
        Ok(removed)
    }

    fn get_edge_index(
        &self,
        space_id: u64,
        index_name: &str,
    ) -> Result<Option<Index>, StorageError> {
        let indexes = self.edge_indexes.read();
        Ok(indexes.get(&(space_id, index_name.to_string())).cloned())
    }

    fn list_edge_indexes(&self, space_id: u64) -> Result<Vec<Index>, StorageError> {
        let indexes = self.edge_indexes.read();
        Ok(indexes
            .iter()
            .filter(|((sid, _), _)| *sid == space_id)
            .map(|(_, index)| index.clone())
            .collect())
    }

    fn drop_edge_indexes_by_type(
        &self,
        space_id: u64,
        edge_type: &str,
    ) -> Result<(), StorageError> {
        let removed: Vec<String> = {
            let mut indexes = self.edge_indexes.write();
            let mut removed = Vec::new();
            indexes.retain(|_, index| {
                let drop_it = index.space_id == space_id && index.schema_name == edge_type;
                if drop_it {
                    removed.push(index.name.clone());
                }
                !drop_it
            });
            removed
        };
        for index_name in removed {
            self.emit_schema_event(SchemaChangeEvent::EdgeIndexDropped {
                space_id,
                index_name,
            });
        }
        Ok(())
    }

    fn set_tag_index_status(
        &self,
        space_id: u64,
        index_name: &str,
        status: IndexStatus,
    ) -> Result<bool, StorageError> {
        let mut indexes = self.tag_indexes.write();
        let key = (space_id, index_name.to_string());
        if let Some(index) = indexes.get_mut(&key) {
            index.status = status;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn set_edge_index_status(
        &self,
        space_id: u64,
        index_name: &str,
        status: IndexStatus,
    ) -> Result<bool, StorageError> {
        let mut indexes = self.edge_indexes.write();
        let key = (space_id, index_name.to_string());
        if let Some(index) = indexes.get_mut(&key) {
            index.status = status;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{IndexConfig, IndexType};
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn tag_index(name: &str, schema: &str) -> Index {
        Index::new(IndexConfig {
            id: 0,
            name: name.to_string(),
            space_id: 1,
            schema_name: schema.to_string(),
            fields: Vec::new(),
            properties: Vec::new(),
            index_type: IndexType::TagIndex,
            is_unique: false,
            covering: false,
            partial_condition: None,
        })
    }

    #[test]
    fn batch_drop_emits_one_event_per_index() {
        let manager = IndexManager::new();
        manager
            .create_tag_index(1, &tag_index("idx_a", "Person"))
            .unwrap();
        manager
            .create_tag_index(1, &tag_index("idx_b", "Person"))
            .unwrap();

        let hits = Arc::new(AtomicUsize::new(0));
        let probe = Arc::clone(&hits);
        manager.register_schema_callback(Arc::new(move |event| {
            if matches!(event, SchemaChangeEvent::TagIndexDropped { .. }) {
                probe.fetch_add(1, Ordering::SeqCst);
            }
        }));

        manager.drop_tag_indexes_by_tag(1, "Person").unwrap();
        assert_eq!(hits.load(Ordering::SeqCst), 2);
        assert!(manager.list_tag_indexes(1).unwrap().is_empty());
    }

    #[test]
    fn unregister_stops_delivery() {
        let manager = IndexManager::new();
        let hits = Arc::new(AtomicUsize::new(0));
        let probe = Arc::clone(&hits);
        let id = manager.register_schema_callback(Arc::new(move |_| {
            probe.fetch_add(1, Ordering::SeqCst);
        }));
        assert!(manager.unregister_schema_callback(id));
        manager
            .create_tag_index(1, &tag_index("idx_c", "Person"))
            .unwrap();
        assert_eq!(hits.load(Ordering::SeqCst), 0);
    }
}
