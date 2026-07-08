use crate::core::types::Index;
use crate::core::StorageError;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

const INDEX_FORMAT_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct IndexSnapshot {
    version: u32,
    tag_indexes: Vec<(u64, String, Index)>,
    edge_indexes: Vec<(u64, String, Index)>,
}

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
}

pub struct IndexManager {
    tag_indexes: Arc<RwLock<HashMap<(u64, String), Index>>>,
    edge_indexes: Arc<RwLock<HashMap<(u64, String), Index>>>,
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
        }
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

        for (space_id, name, index) in snapshot.tag_indexes {
            self.tag_indexes.write().insert((space_id, name), index);
        }

        for (space_id, name, index) in snapshot.edge_indexes {
            self.edge_indexes.write().insert((space_id, name), index);
        }

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
        let mut indexes = self.tag_indexes.write();
        let key = (space_id, index.name.clone());
        if indexes.contains_key(&key) {
            return Ok(false);
        }
        let mut index_with_space_id = index.clone();
        index_with_space_id.space_id = space_id;
        indexes.insert(key, index_with_space_id);
        Ok(true)
    }

    fn drop_tag_index(&self, space_id: u64, index_name: &str) -> Result<bool, StorageError> {
        let mut indexes = self.tag_indexes.write();
        let key = (space_id, index_name.to_string());
        Ok(indexes.remove(&key).is_some())
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
        let mut indexes = self.tag_indexes.write();
        indexes.retain(|_, index| !(index.space_id == space_id && index.schema_name == tag_name));
        Ok(())
    }

    fn create_edge_index(&self, space_id: u64, index: &Index) -> Result<bool, StorageError> {
        let mut indexes = self.edge_indexes.write();
        let key = (space_id, index.name.clone());
        if indexes.contains_key(&key) {
            return Ok(false);
        }
        let mut index_with_space_id = index.clone();
        index_with_space_id.space_id = space_id;
        indexes.insert(key, index_with_space_id);
        Ok(true)
    }

    fn drop_edge_index(&self, space_id: u64, index_name: &str) -> Result<bool, StorageError> {
        let mut indexes = self.edge_indexes.write();
        let key = (space_id, index_name.to_string());
        Ok(indexes.remove(&key).is_some())
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
        let mut indexes = self.edge_indexes.write();
        indexes.retain(|_, index| !(index.space_id == space_id && index.schema_name == edge_type));
        Ok(())
    }
}
