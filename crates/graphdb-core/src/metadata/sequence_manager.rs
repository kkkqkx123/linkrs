use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;

use crate::wal::redo::UpdateSequenceRedo;
use crate::wal::traits::WalWriter;
use crate::wal::types::WalOpType;
use crate::StorageError;

use super::sequence::SequenceDef;

/// Trait for sequence persistence backends
pub trait SequenceStorage: Send + Sync {
    /// Load all sequences from storage
    fn load_all(&self) -> Result<Vec<SequenceDef>, StorageError>;
    /// Save a sequence definition
    fn save(&self, def: &SequenceDef) -> Result<(), StorageError>;
    /// Delete a sequence by name
    fn delete(&self, name: &str) -> Result<(), StorageError>;
    /// Update the current_value of a sequence
    fn update_value(&self, name: &str, value: i64) -> Result<(), StorageError>;
}

/// Sequence manager
///
/// Manages in-memory cache of sequences with optional persistence and WAL backing.
/// Thread-safe via `parking_lot::RwLock`.
pub struct SequenceManager {
    sequences: RwLock<HashMap<String, Arc<SequenceDef>>>,
    storage: Option<Arc<dyn SequenceStorage>>,
    wal_writer: Option<Arc<parking_lot::Mutex<dyn WalWriter>>>,
}

impl SequenceManager {
    /// Create a new SequenceManager without persistence
    pub fn new() -> Self {
        Self {
            sequences: RwLock::new(HashMap::new()),
            storage: None,
            wal_writer: None,
        }
    }

    /// Create a new SequenceManager with a persistence backend
    pub fn with_storage(storage: Arc<dyn SequenceStorage>) -> Self {
        Self {
            sequences: RwLock::new(HashMap::new()),
            storage: Some(storage),
            wal_writer: None,
        }
    }

    /// Create a new SequenceManager with persistence and WAL support
    pub fn with_wal(
        storage: Arc<dyn SequenceStorage>,
        wal_writer: Arc<parking_lot::Mutex<dyn WalWriter>>,
    ) -> Self {
        Self {
            sequences: RwLock::new(HashMap::new()),
            storage: Some(storage),
            wal_writer: Some(wal_writer),
        }
    }

    /// Initialize by loading all sequences from storage
    pub fn initialize(&self) -> Result<(), StorageError> {
        if let Some(ref storage) = self.storage {
            let defs = storage.load_all()?;
            let mut map = self.sequences.write();
            for def in defs {
                let name = def.name.clone();
                map.insert(name, Arc::new(def));
            }
        }
        Ok(())
    }

    /// Create a new sequence
    pub fn create_sequence(
        &self,
        name: String,
        start: i64,
        increment: i64,
        min_value: i64,
        max_value: i64,
        cycle: bool,
    ) -> Result<(), StorageError> {
        {
            let map = self.sequences.read();
            if map.contains_key(&name) {
                return Err(StorageError::db_error(format!(
                    "Sequence '{}' already exists",
                    name
                )));
            }
        }

        let def = SequenceDef::new(name.clone(), start, increment, min_value, max_value, cycle);

        if let Some(ref storage) = self.storage {
            storage.save(&def)?;
        }

        let mut map = self.sequences.write();
        map.insert(name, Arc::new(def));
        Ok(())
    }

    /// Drop (delete) a sequence
    pub fn drop_sequence(&self, name: &str) -> Result<(), StorageError> {
        {
            let mut map = self.sequences.write();
            if map.remove(name).is_none() {
                return Err(StorageError::db_error(format!(
                    "Sequence '{}' does not exist",
                    name
                )));
            }
        }

        if let Some(ref storage) = self.storage {
            storage.delete(name)?;
        }

        Ok(())
    }

    /// Get the current value of a sequence without incrementing
    pub fn current_value(&self, name: &str) -> Result<i64, StorageError> {
        let map = self.sequences.read();
        let def = map
            .get(name)
            .ok_or_else(|| StorageError::db_error(format!("Sequence '{}' does not exist", name)))?;
        Ok(def.current_value())
    }

    /// Get the next value of a sequence (atomic increment)
    pub fn next_value(&self, name: &str) -> Result<i64, StorageError> {
        let value = {
            let map = self.sequences.read();
            let def = map.get(name).ok_or_else(|| {
                StorageError::db_error(format!("Sequence '{}' does not exist", name))
            })?;
            def.next_value()?
        };

        if let Some(ref storage) = self.storage {
            storage.update_value(name, value)?;
        }

        // Write to WAL for crash recovery
        if let Some(ref wal_writer) = self.wal_writer {
            let redo = UpdateSequenceRedo {
                space_id: 0,
                table_name: name.to_string(),
                next_value: value as u64,
            };
            let payload = postcard::to_allocvec(&redo)
                .map_err(|e| StorageError::serialize_error(e.to_string()))?;
            let mut writer = wal_writer.lock();
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            writer
                .append_entry(WalOpType::UpdateSequence, ts, &payload)
                .map_err(|e| StorageError::db_error(format!("WAL write error: {}", e)))?;
        }

        Ok(value)
    }

    /// Alter sequence properties
    pub fn alter_sequence(
        &self,
        name: &str,
        increment: Option<i64>,
        min_value: Option<i64>,
        max_value: Option<i64>,
        cycle: Option<bool>,
    ) -> Result<(), StorageError> {
        let map = self.sequences.read();
        let def = map
            .get(name)
            .ok_or_else(|| StorageError::db_error(format!("Sequence '{}' does not exist", name)))?;

        let new_increment = increment.unwrap_or(def.increment);
        let new_min = min_value.unwrap_or(def.min_value);
        let new_max = max_value.unwrap_or(def.max_value);
        let new_cycle = cycle.unwrap_or(def.cycle);
        let current = def.current_value();

        drop(map);

        let new_def = SequenceDef::new(
            name.to_string(),
            current,
            new_increment,
            new_min,
            new_max,
            new_cycle,
        );

        if let Some(ref storage) = self.storage {
            storage.save(&new_def)?;
        }

        let mut map = self.sequences.write();
        map.insert(name.to_string(), Arc::new(new_def));
        Ok(())
    }

    /// Flush all sequences to storage
    pub fn flush(&self) -> Result<(), StorageError> {
        if let Some(ref storage) = self.storage {
            let map = self.sequences.read();
            for (_name, def) in map.iter() {
                storage.save(def)?;
            }
        }
        Ok(())
    }

    /// List all sequence names
    pub fn list_sequences(&self) -> Vec<String> {
        let map = self.sequences.read();
        map.keys().cloned().collect()
    }

    /// Check if a sequence exists
    pub fn exists(&self, name: &str) -> bool {
        let map = self.sequences.read();
        map.contains_key(name)
    }
}

impl Default for SequenceManager {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for SequenceManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SequenceManager")
            .field("count", &self.sequences.read().len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicI64;

    struct MockStorage {
        data: RwLock<HashMap<String, Vec<u8>>>,
    }

    impl MockStorage {
        fn new() -> Self {
            Self {
                data: RwLock::new(HashMap::new()),
            }
        }
    }

    impl SequenceStorage for MockStorage {
        fn load_all(&self) -> Result<Vec<SequenceDef>, StorageError> {
            let data = self.data.read();
            let mut result = Vec::new();
            for (_key, _value) in data.iter() {
                // Simplified: in real impl would deserialize
                // For tests we just return empty
            }
            Ok(result)
        }

        fn save(&self, def: &SequenceDef) -> Result<(), StorageError> {
            let key = format!("sequences:{}", def.name);
            let value = postcard::to_allocvec(def)
                .map_err(|e| StorageError::serialize_error(e.to_string()))?;
            self.data.write().insert(key, value);
            Ok(())
        }

        fn delete(&self, name: &str) -> Result<(), StorageError> {
            let key = format!("sequences:{}", name);
            self.data.write().remove(&key);
            Ok(())
        }

        fn update_value(&self, name: &str, value: i64) -> Result<(), StorageError> {
            let key = format!("sequences:{}", name);
            let mut data = self.data.write();
            if let Some(raw) = data.get(&key) {
                let mut def: SequenceDef = postcard::from_bytes(raw)
                    .map_err(|e| StorageError::deserialize_error(e.to_string()))?;
                def.set_value(value);
                let new_raw = postcard::to_allocvec(&def)
                    .map_err(|e| StorageError::serialize_error(e.to_string()))?;
                data.insert(key, new_raw);
            }
            Ok(())
        }
    }

    #[test]
    fn test_manager_create_drop() {
        let manager = SequenceManager::new();
        manager
            .create_sequence("seq1".to_string(), 1, 1, 1, 100, false)
            .unwrap();
        assert!(manager.exists("seq1"));
        assert_eq!(manager.current_value("seq1").unwrap(), 1);

        manager.drop_sequence("seq1").unwrap();
        assert!(!manager.exists("seq1"));
    }

    #[test]
    fn test_manager_duplicate_create() {
        let manager = SequenceManager::new();
        manager
            .create_sequence("seq1".to_string(), 1, 1, 1, 100, false)
            .unwrap();
        assert!(manager
            .create_sequence("seq1".to_string(), 1, 1, 1, 100, false)
            .is_err());
    }

    #[test]
    fn test_manager_next_value() {
        let manager = SequenceManager::new();
        manager
            .create_sequence("seq1".to_string(), 10, 5, 1, 1000, false)
            .unwrap();
        assert_eq!(manager.next_value("seq1").unwrap(), 15);
        assert_eq!(manager.next_value("seq1").unwrap(), 20);
        assert_eq!(manager.current_value("seq1").unwrap(), 20);
    }

    #[test]
    fn test_manager_alter() {
        let manager = SequenceManager::new();
        manager
            .create_sequence("seq1".to_string(), 1, 1, 1, 100, false)
            .unwrap();
        manager
            .alter_sequence("seq1", Some(10), None, None, Some(true))
            .unwrap();
        assert_eq!(manager.next_value("seq1").unwrap(), 11);
    }

    #[test]
    fn test_manager_list() {
        let manager = SequenceManager::new();
        manager
            .create_sequence("a".to_string(), 1, 1, 1, 100, false)
            .unwrap();
        manager
            .create_sequence("b".to_string(), 1, 1, 1, 100, false)
            .unwrap();
        let mut names = manager.list_sequences();
        names.sort();
        assert_eq!(names, vec!["a", "b"]);
    }

    #[test]
    fn test_manager_with_storage() {
        let storage = Arc::new(MockStorage::new());
        let manager = SequenceManager::with_storage(storage);
        manager
            .create_sequence("seq1".to_string(), 1, 1, 1, 100, false)
            .unwrap();
        assert!(manager.exists("seq1"));
    }
}
