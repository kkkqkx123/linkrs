//! ID Indexer
//!
//! Maps external IDs (strings or integers) to internal vertex IDs.
//! Provides O(1) lookup in both directions.
//!
//! # Architecture
//!
//! This module uses a two-level design:
//!
//! - **IdManager**: Core business logic for ID management
//!   - Bidirectional mapping between external IDs and internal indices
//!   - Compact/remapping algorithm
//!   - Uses HashMap for storage (no unnecessary concurrency)
//!
//! - **IdIndexer**: Simple wrapper for API consistency
//!   - All operations are single-threaded at runtime
//!   - External synchronization via GraphDataStore's RwLock ensures correctness

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::core::error::{StorageError, StorageResult};

const DEFAULT_INITIAL_CAPACITY: usize = 1024;
const DEFAULT_GROWTH_FACTOR: f64 = 1.5;
const MAX_CAPACITY: usize = u32::MAX as usize;

const ID_KEY_TYPE_INT: u8 = 0;
const ID_KEY_TYPE_TEXT: u8 = 1;

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub enum IdKey {
    Int(i64),
    Text(String),
}

impl IdKey {
    /// Write the key bytes into an existing buffer to avoid extra allocations.
    /// The buffer is cleared before writing.
    pub fn write_to(&self, buf: &mut Vec<u8>) {
        buf.clear();
        match self {
            IdKey::Int(val) => {
                buf.reserve(9);
                buf.push(ID_KEY_TYPE_INT);
                buf.extend_from_slice(&val.to_be_bytes());
            }
            IdKey::Text(val) => {
                buf.reserve(1 + val.len());
                buf.push(ID_KEY_TYPE_TEXT);
                buf.extend_from_slice(val.as_bytes());
            }
        }
    }

    pub fn from_bytes(bytes: &[u8]) -> StorageResult<Self> {
        if bytes.is_empty() {
            return Err(StorageError::deserialize_error(
                "Empty IdKey bytes".to_string(),
            ));
        }

        match bytes[0] {
            ID_KEY_TYPE_INT => {
                if bytes.len() != 9 {
                    return Err(StorageError::deserialize_error(format!(
                        "Invalid Int IdKey length: {}",
                        bytes.len()
                    )));
                }
                let val_bytes: [u8; 8] = bytes[1..9].try_into().map_err(|_| {
                    StorageError::deserialize_error("Invalid Int IdKey bytes".to_string())
                })?;
                Ok(IdKey::Int(i64::from_be_bytes(val_bytes)))
            }
            ID_KEY_TYPE_TEXT => {
                let text = String::from_utf8(bytes[1..].to_vec())
                    .map_err(|e| StorageError::deserialize_error(e.to_string()))?;
                Ok(IdKey::Text(text))
            }
            tag => Err(StorageError::deserialize_error(format!(
                "Unknown IdKey type tag: {}",
                tag
            ))),
        }
    }
}

impl std::fmt::Display for IdKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IdKey::Int(val) => write!(f, "{}", val),
            IdKey::Text(val) => write!(f, "{}", val),
        }
    }
}

#[derive(Debug, Clone)]
pub struct IdIndexerConfig {
    pub initial_capacity: usize,
    pub growth_factor: f64,
    pub max_capacity: usize,
    pub enable_free_list: bool,
}

impl Default for IdIndexerConfig {
    fn default() -> Self {
        Self {
            initial_capacity: DEFAULT_INITIAL_CAPACITY,
            growth_factor: DEFAULT_GROWTH_FACTOR,
            max_capacity: MAX_CAPACITY,
            enable_free_list: true,
        }
    }
}

impl IdIndexerConfig {
    pub fn with_initial_capacity(mut self, capacity: usize) -> Self {
        self.initial_capacity = capacity;
        self
    }
}

/// Core bidirectional mapping between external IDs and internal indices.
///
/// This struct manages the fundamental lookup operations:
/// - Key → ID mapping (via HashMap)
/// - ID → Key reverse mapping (via Vec)
///
/// IdManager is the authoritative source for ID management logic.
#[derive(Debug)]
pub struct IdManager {
    keys: Vec<Option<IdKey>>,
    key_to_id: HashMap<IdKey, u32>,
    config: IdIndexerConfig,
}

impl IdManager {
    pub fn new() -> Self {
        Self::with_config(IdIndexerConfig::default())
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self::with_config(IdIndexerConfig::default().with_initial_capacity(capacity))
    }

    pub fn with_config(config: IdIndexerConfig) -> Self {
        let capacity = config.initial_capacity.min(config.max_capacity);
        Self {
            keys: Vec::with_capacity(capacity),
            key_to_id: HashMap::with_capacity(capacity),
            config,
        }
    }

    pub fn insert(&mut self, key: IdKey) -> StorageResult<u32> {
        if self.key_to_id.contains_key(&key) {
            return Err(StorageError::vertex_already_exists(format!("{:?}", key)));
        }

        if self.keys.len() >= self.config.max_capacity {
            return Err(StorageError::capacity_exceeded());
        }

        if self.keys.len() >= self.keys.capacity() {
            let current_capacity = self.keys.capacity();
            if current_capacity >= self.config.max_capacity {
                return Err(StorageError::capacity_exceeded());
            }

            let new_capacity = ((current_capacity as f64 * self.config.growth_factor) as usize)
                .min(self.config.max_capacity)
                .max(current_capacity + 1);
            self.keys.reserve(new_capacity - current_capacity);
        }

        let index = self.keys.len() as u32;
        self.keys.push(Some(key.clone()));
        self.key_to_id.insert(key, index);

        Ok(index)
    }

    pub fn get_id(&self, key: &IdKey) -> Option<u32> {
        self.key_to_id.get(key).copied()
    }

    pub fn get_key(&self, index: u32) -> Option<IdKey> {
        self.keys.get(index as usize)?.as_ref().cloned()
    }

    pub fn contains(&self, key: &IdKey) -> bool {
        self.key_to_id.contains_key(key)
    }

    pub fn len(&self) -> usize {
        self.key_to_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.key_to_id.is_empty()
    }

    pub fn remove(&mut self, key: &IdKey) -> Option<u32> {
        self.key_to_id.remove(key).map(|idx| {
            if (idx as usize) < self.keys.len() {
                self.keys[idx as usize] = None;
            }
            idx
        })
    }

    pub fn iter(&self) -> Vec<(IdKey, u32)> {
        self.key_to_id
            .iter()
            .map(|(key, &idx)| (key.clone(), idx))
            .collect()
    }

    pub fn clear(&mut self) {
        self.key_to_id.clear();
        self.keys.clear();
    }

    pub fn compact(&mut self) -> StorageResult<HashMap<u32, u32>> {
        let entries: Vec<(u32, IdKey)> = self
            .key_to_id
            .iter()
            .map(|(key, &idx)| (idx, key.clone()))
            .collect();

        if entries.is_empty() {
            return Ok(HashMap::new());
        }

        let mut entries = entries;
        entries.sort_by_key(|(old_id, _)| *old_id);

        let mut mapping = HashMap::new();
        for (new_id, (old_id, _)) in entries.iter().enumerate() {
            let new_id_u32 = new_id as u32;
            if *old_id != new_id_u32 {
                mapping.insert(*old_id, new_id_u32);
            }
        }

        if mapping.is_empty() {
            return Ok(HashMap::new());
        }

        self.rebuild_with_mapping(&entries)?;

        Ok(mapping)
    }

    fn rebuild_with_mapping(&mut self, entries: &[(u32, IdKey)]) -> StorageResult<()> {
        let mut new_keys = vec![None; entries.len()];
        for (new_id, (_, key)) in entries.iter().enumerate() {
            new_keys[new_id] = Some(key.clone());
        }

        let mut new_key_to_id = HashMap::with_capacity(entries.len());
        for (new_id, (_, key)) in entries.iter().enumerate() {
            new_key_to_id.insert(key.clone(), new_id as u32);
        }

        self.keys = new_keys;
        self.key_to_id = new_key_to_id;

        Ok(())
    }

    pub fn set_at(&mut self, index: u32, key: IdKey) {
        if self.key_to_id.contains_key(&key) {
            return;
        }
        while self.keys.len() <= index as usize {
            self.keys.push(None);
        }
        self.keys[index as usize] = Some(key.clone());
        self.key_to_id.insert(key, index);
    }

    pub fn memory_usage(&self) -> usize {
        let keys_size = self.keys.capacity() * std::mem::size_of::<Option<IdKey>>();
        let map_estimate = self.key_to_id.len()
            * (std::mem::size_of::<IdKey>() + std::mem::size_of::<u32>());
        keys_size + map_estimate
    }

    pub fn memory_size(&self) -> usize {
        self.memory_usage() + std::mem::size_of::<Self>()
    }
}

impl Default for IdManager {
    fn default() -> Self {
        Self::new()
    }
}

/// ID indexer wrapper for simple, single-threaded ID management.
///
/// Although this struct is cloneable (contains Arc), actual concurrent access
/// is prevented by the external RwLock in GraphDataStore. All operations
/// are effectively single-threaded.
///
/// This design removes unnecessary DashMap overhead while keeping the interface
/// unchanged for compatibility.
#[derive(Debug, Clone)]
pub struct IdIndexer {
    manager: Arc<Mutex<IdManager>>,
    config: IdIndexerConfig,
}

impl IdIndexer {
    pub fn new() -> Self {
        Self::with_config(IdIndexerConfig::default())
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self::with_config(IdIndexerConfig::default().with_initial_capacity(capacity))
    }

    pub fn with_config(config: IdIndexerConfig) -> Self {
        Self {
            manager: Arc::new(Mutex::new(IdManager::with_config(config.clone()))),
            config,
        }
    }

    pub fn insert(&self, key: IdKey) -> StorageResult<u32> {
        let mut manager = self.manager.lock();
        manager.insert(key)
    }

    pub fn get_index(&self, key: &IdKey) -> Option<u32> {
        let manager = self.manager.lock();
        manager.get_id(key)
    }

    pub fn get_key(&self, index: u32) -> Option<IdKey> {
        let manager = self.manager.lock();
        manager.get_key(index)
    }

    pub fn contains(&self, key: &IdKey) -> bool {
        let manager = self.manager.lock();
        manager.contains(key)
    }

    pub fn len(&self) -> usize {
        let manager = self.manager.lock();
        manager.len()
    }

    pub fn is_empty(&self) -> bool {
        let manager = self.manager.lock();
        manager.is_empty()
    }

    pub fn remove(&self, key: &IdKey) -> Option<u32> {
        let mut manager = self.manager.lock();
        manager.remove(key)
    }

    pub fn iter(&self) -> Vec<(IdKey, u32)> {
        let manager = self.manager.lock();
        manager.iter()
    }

    pub fn clear(&self) {
        let mut manager = self.manager.lock();
        manager.clear();
    }

    pub fn memory_usage(&self) -> usize {
        let manager = self.manager.lock();
        manager.memory_usage()
    }

    pub fn memory_size(&self) -> usize {
        let manager = self.manager.lock();
        manager.memory_size() + std::mem::size_of::<Self>()
    }

    pub fn compact(&self) -> StorageResult<HashMap<u32, u32>> {
        let mut manager = self.manager.lock();
        manager.compact()
    }

    pub fn set_at(&self, index: u32, key: IdKey) {
        let mut manager = self.manager.lock();
        manager.set_at(index, key);
    }
}

impl Default for IdIndexer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_operations() {
        let indexer = IdIndexer::new();

        let idx1 = indexer.insert(IdKey::Text("vertex1".to_string())).unwrap();
        assert_eq!(idx1, 0);

        let idx2 = indexer.insert(IdKey::Text("vertex2".to_string())).unwrap();
        assert_eq!(idx2, 1);

        assert_eq!(
            indexer.get_index(&IdKey::Text("vertex1".to_string())),
            Some(0)
        );
        assert_eq!(
            indexer.get_index(&IdKey::Text("vertex2".to_string())),
            Some(1)
        );
        assert_eq!(indexer.get_index(&IdKey::Text("vertex3".to_string())), None);

        assert_eq!(
            indexer.get_key(0),
            Some(IdKey::Text("vertex1".to_string()))
        );
        assert_eq!(
            indexer.get_key(1),
            Some(IdKey::Text("vertex2".to_string()))
        );
    }

    #[test]
    fn test_int_id_operations() {
        let indexer = IdIndexer::new();

        let idx1 = indexer.insert(IdKey::Int(100)).unwrap();
        assert_eq!(idx1, 0);

        let idx2 = indexer.insert(IdKey::Int(200)).unwrap();
        assert_eq!(idx2, 1);

        assert_eq!(indexer.get_index(&IdKey::Int(100)), Some(0));
        assert_eq!(indexer.get_index(&IdKey::Int(200)), Some(1));
        assert_eq!(indexer.get_index(&IdKey::Int(300)), None);

        assert_eq!(indexer.get_key(0), Some(IdKey::Int(100)));
        assert_eq!(indexer.get_key(1), Some(IdKey::Int(200)));
    }

    #[test]
    fn test_mixed_id_operations() {
        let indexer = IdIndexer::new();

        let idx1 = indexer.insert(IdKey::Int(100)).unwrap();
        let idx2 = indexer.insert(IdKey::Text("vertex1".to_string())).unwrap();
        let idx3 = indexer.insert(IdKey::Int(200)).unwrap();

        assert_eq!(idx1, 0);
        assert_eq!(idx2, 1);
        assert_eq!(idx3, 2);

        assert_eq!(indexer.len(), 3);
    }

    #[test]
    fn test_dynamic_expansion() {
        let indexer = IdIndexer::with_config(IdIndexerConfig {
            initial_capacity: 2,
            growth_factor: 2.0,
            max_capacity: MAX_CAPACITY,
            enable_free_list: true,
        });

        assert!(indexer.insert(IdKey::Text("v1".to_string())).is_ok());
        assert!(indexer.insert(IdKey::Text("v2".to_string())).is_ok());
        assert!(indexer.insert(IdKey::Text("v3".to_string())).is_ok());
        assert!(indexer.insert(IdKey::Text("v4".to_string())).is_ok());
        assert!(indexer.insert(IdKey::Text("v5".to_string())).is_ok());

        assert_eq!(indexer.len(), 5);
    }

    #[test]
    fn test_duplicate_insert() {
        let indexer = IdIndexer::new();

        assert!(indexer.insert(IdKey::Text("v1".to_string())).is_ok());
        assert!(indexer.insert(IdKey::Text("v1".to_string())).is_err());
    }

    #[test]
    fn test_max_capacity() {
        let indexer = IdIndexer::with_config(IdIndexerConfig {
            initial_capacity: 2,
            growth_factor: DEFAULT_GROWTH_FACTOR,
            max_capacity: 3,
            enable_free_list: true,
        });

        assert!(indexer.insert(IdKey::Text("v1".to_string())).is_ok());
        assert!(indexer.insert(IdKey::Text("v2".to_string())).is_ok());
        assert!(indexer.insert(IdKey::Text("v3".to_string())).is_ok());
        assert!(indexer.insert(IdKey::Text("v4".to_string())).is_err());
    }

    #[test]
    fn test_concurrent_parallel_inserts() {
        use std::sync::Arc as StdArc;
        use std::thread;

        let indexer = StdArc::new(IdIndexer::new());
        let mut handles = vec![];

        for thread_id in 0..4 {
            let indexer_clone = StdArc::clone(&indexer);
            let handle = thread::spawn(move || {
                for i in 0..25 {
                    let key = IdKey::Text(format!("v_{}_{}", thread_id, i));
                    let _ = indexer_clone.insert(key);
                }
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.join().expect("thread panicked");
        }

        assert_eq!(indexer.len(), 100);
    }

    #[test]
    fn test_concurrent_mixed_operations() {
        use std::sync::Arc as StdArc;
        use std::thread;

        let indexer = StdArc::new(IdIndexer::new());

        for i in 0..10 {
            let key = IdKey::Text(format!("v{}", i));
            let _ = indexer.insert(key);
        }

        let mut handles = vec![];

        for _ in 0..2 {
            let indexer_clone = StdArc::clone(&indexer);
            let handle = thread::spawn(move || {
                for i in 0..10 {
                    let key = IdKey::Text(format!("v{}", i));
                    let _ = indexer_clone.get_index(&key);
                }
            });
            handles.push(handle);
        }

        let indexer_clone = StdArc::clone(&indexer);
        let handle = thread::spawn(move || {
            for i in 10..20 {
                let key = IdKey::Text(format!("v{}", i));
                let _ = indexer_clone.insert(key);
            }
        });
        handles.push(handle);

        for handle in handles {
            handle.join().expect("thread panicked");
        }

        assert_eq!(indexer.len(), 20);
    }

    #[test]
    fn test_remove() {
        let indexer = IdIndexer::new();

        indexer.insert(IdKey::Text("v1".to_string())).unwrap();
        indexer.insert(IdKey::Text("v2".to_string())).unwrap();
        indexer.insert(IdKey::Text("v3".to_string())).unwrap();

        assert_eq!(indexer.len(), 3);

        indexer.remove(&IdKey::Text("v2".to_string()));
        assert_eq!(indexer.len(), 2);

        assert_eq!(
            indexer.get_index(&IdKey::Text("v2".to_string())),
            None
        );
    }
}
