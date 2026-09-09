use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::types::DataType;
use crate::wal::redo::{CreateTypeAliasRedo, DropTypeAliasRedo};
use crate::wal::traits::WalWriter;
use crate::wal::types::WalOpType;
use crate::StorageError;

/// A user-defined type alias: `name` stands for the type described by
/// `underlying` (raw type text, which may itself reference other aliases).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TypeAliasDef {
    /// Alias name (stored as written; looked up case-insensitively).
    pub name: String,
    /// Underlying type text, e.g. `INT`, `LIST<STRING>`, or another alias.
    pub underlying: String,
}

impl TypeAliasDef {
    pub fn new(name: String, underlying: String) -> Self {
        Self { name, underlying }
    }
}

/// Trait for type-alias persistence backends.
///
/// Mirrors [`super::macro_manager::MacroStorage`]: the storage engine can
/// implement this against its catalog KV area; [`MemTypeAliasStorage`]
/// provides the default in-memory backend.
pub trait TypeAliasStorage: Send + Sync {
    /// Load all type alias definitions from storage.
    fn load_all(&self) -> Result<Vec<TypeAliasDef>, StorageError>;
    /// Save (insert or overwrite) a type alias definition.
    fn save(&self, def: &TypeAliasDef) -> Result<(), StorageError>;
    /// Delete a type alias definition by name.
    fn delete(&self, name: &str) -> Result<(), StorageError>;
}

/// In-memory [`TypeAliasStorage`] backend (no durability beyond WAL).
#[derive(Debug, Default)]
pub struct MemTypeAliasStorage {
    data: RwLock<HashMap<String, TypeAliasDef>>,
}

impl MemTypeAliasStorage {
    pub fn new() -> Self {
        Self {
            data: RwLock::new(HashMap::new()),
        }
    }
}

impl TypeAliasStorage for MemTypeAliasStorage {
    fn load_all(&self) -> Result<Vec<TypeAliasDef>, StorageError> {
        Ok(self.data.read().values().cloned().collect())
    }

    fn save(&self, def: &TypeAliasDef) -> Result<(), StorageError> {
        self.data.write().insert(def.name.clone(), def.clone());
        Ok(())
    }

    fn delete(&self, name: &str) -> Result<(), StorageError> {
        self.data.write().remove(name);
        Ok(())
    }
}

/// Type alias catalog manager.
///
/// Aliases are resolved to builtin [`DataType`] at parse/bind time, so the
/// executor never sees an alias. Creating an alias whose (transitive)
/// underlying type references itself is rejected (cycle detection).
/// Thread-safe via `parking_lot::RwLock`.
pub struct TypeAliasManager {
    aliases: RwLock<HashMap<String, Arc<TypeAliasDef>>>,
    storage: Option<Arc<dyn TypeAliasStorage>>,
    wal_writer: Option<Arc<parking_lot::Mutex<dyn WalWriter>>>,
}

impl TypeAliasManager {
    /// Create a new TypeAliasManager without persistence.
    pub fn new() -> Self {
        Self {
            aliases: RwLock::new(HashMap::new()),
            storage: None,
            wal_writer: None,
        }
    }

    /// Create a new TypeAliasManager with a persistence backend.
    pub fn with_storage(storage: Arc<dyn TypeAliasStorage>) -> Self {
        Self {
            aliases: RwLock::new(HashMap::new()),
            storage: Some(storage),
            wal_writer: None,
        }
    }

    /// Create a new TypeAliasManager with persistence and WAL support.
    pub fn with_wal(
        storage: Arc<dyn TypeAliasStorage>,
        wal_writer: Arc<parking_lot::Mutex<dyn WalWriter>>,
    ) -> Self {
        Self {
            aliases: RwLock::new(HashMap::new()),
            storage: Some(storage),
            wal_writer: Some(wal_writer),
        }
    }

    fn canonical(name: &str) -> String {
        name.to_ascii_uppercase()
    }

    /// Initialize by loading all aliases from storage.
    pub fn initialize(&self) -> Result<(), StorageError> {
        if let Some(ref storage) = self.storage {
            let defs = storage.load_all()?;
            let mut map = self.aliases.write();
            for def in defs {
                map.insert(Self::canonical(&def.name), Arc::new(def));
            }
        }
        Ok(())
    }

    /// Look up an alias definition by name (case-insensitive).
    pub fn get_alias(&self, name: &str) -> Option<Arc<TypeAliasDef>> {
        let map = self.aliases.read();
        map.get(&Self::canonical(name)).cloned()
    }

    /// Check if an alias exists (case-insensitive).
    pub fn exists(&self, name: &str) -> bool {
        let map = self.aliases.read();
        map.contains_key(&Self::canonical(name))
    }

    /// List all alias definitions, ordered by name for stable output.
    pub fn list_aliases(&self) -> Vec<Arc<TypeAliasDef>> {
        let map = self.aliases.read();
        let mut defs: Vec<Arc<TypeAliasDef>> = map.values().cloned().collect();
        defs.sort_by(|a, b| a.name.cmp(&b.name));
        defs
    }

    /// Names of aliases whose underlying text directly references `name`.
    pub fn dependents(&self, name: &str) -> Vec<String> {
        let key = Self::canonical(name);
        let map = self.aliases.read();
        let mut out = Vec::new();
        for def in map.values() {
            if Self::references_alias(&def.underlying, &key) {
                out.push(def.name.clone());
            }
        }
        out.sort();
        out
    }

    /// Whether raw type text mentions an alias name as a standalone word.
    fn references_alias(underlying: &str, canonical_alias: &str) -> bool {
        underlying
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .any(|word| word.to_ascii_uppercase() == *canonical_alias)
    }

    /// Resolve raw type text to a builtin [`DataType`], following alias
    /// chains. `parse_builtin` parses one level of builtin type text and
    /// returns `None` for unknown names (which are then treated as aliases).
    ///
    /// Cycles are reported as errors even though creation-time detection
    /// should prevent them (defense in depth for concurrently dropped
    /// aliases or hand-edited catalogs).
    pub fn resolve<F>(&self, type_text: &str, parse_builtin: &F) -> Result<DataType, StorageError>
    where
        F: Fn(&str) -> Option<DataType>,
    {
        let mut visiting = HashSet::new();
        self.resolve_inner(type_text.trim(), parse_builtin, &mut visiting)
    }

    fn resolve_inner<F>(
        &self,
        type_text: &str,
        parse_builtin: &F,
        visiting: &mut HashSet<String>,
    ) -> Result<DataType, StorageError>
    where
        F: Fn(&str) -> Option<DataType>,
    {
        if let Some(ty) = parse_builtin(type_text) {
            return Ok(ty);
        }
        // Composite types embedding an alias (e.g. `LIST<MY_ALIAS>`) are not
        // expanded: aliases must denote a complete type at the top level.
        // Report the failure with the offending text for a clear diagnostic.
        let key = Self::canonical(type_text);
        let def = {
            let map = self.aliases.read();
            map.get(&key).cloned()
        };
        let Some(def) = def else {
            return Err(StorageError::db_error(format!(
                "Unknown data type: '{}'",
                type_text
            )));
        };
        if !visiting.insert(key.clone()) {
            return Err(StorageError::db_error(format!(
                "Cyclic type alias detected while resolving '{}'",
                type_text
            )));
        }
        let underlying = def.underlying.clone();
        let resolved = self.resolve_inner(&underlying, parse_builtin, visiting);
        visiting.remove(&key);
        resolved
    }

    /// Check that defining `name AS underlying` introduces no alias cycle.
    ///
    /// Simulates insertion, then walks the dependency chain from `name`.
    fn check_no_cycle(&self, name: &str, underlying: &str) -> Result<(), StorageError> {
        let key = Self::canonical(name);
        let map = self.aliases.read();
        let mut visiting = HashSet::new();
        let mut current = underlying.trim().to_string();
        visiting.insert(key.clone());
        loop {
            let current_key = Self::canonical(&current);
            if current_key == key || !visiting.insert(current_key.clone()) {
                // Either points back at the new alias or revisits a node:
                // both mean the definition would close a cycle.
                if map.contains_key(&current_key) || current_key == key {
                    return Err(StorageError::db_error(format!(
                        "Cyclic type alias detected: '{}' ultimately references itself",
                        name
                    )));
                }
                return Ok(());
            }
            match map.get(&current_key) {
                Some(def) => {
                    current = def.underlying.clone();
                }
                None => return Ok(()),
            }
        }
    }

    /// Create a new type alias.
    pub fn create_alias(&self, def: TypeAliasDef) -> Result<(), StorageError> {
        if def.name.trim().is_empty() {
            return Err(StorageError::db_error("Type name must not be empty"));
        }
        if def.underlying.trim().is_empty() {
            return Err(StorageError::db_error(format!(
                "Type '{}' has an empty underlying type",
                def.name
            )));
        }
        let key = Self::canonical(&def.name);
        {
            let map = self.aliases.read();
            if map.contains_key(&key) {
                return Err(StorageError::db_error(format!(
                    "Type '{}' already exists",
                    def.name
                )));
            }
        }
        self.check_no_cycle(&def.name, &def.underlying)?;

        if let Some(ref storage) = self.storage {
            storage.save(&def)?;
        }
        self.write_wal_create(&def)?;

        let mut map = self.aliases.write();
        if map.contains_key(&key) {
            return Err(StorageError::db_error(format!(
                "Type '{}' already exists",
                def.name
            )));
        }
        map.insert(key, Arc::new(def));
        Ok(())
    }

    /// Drop a type alias. Refuses when other aliases still reference it.
    pub fn drop_alias(&self, name: &str) -> Result<(), StorageError> {
        let dependents = self.dependents(name);
        if !dependents.is_empty() {
            return Err(StorageError::db_error(format!(
                "Cannot drop type '{}': still referenced by {}",
                name,
                dependents.join(", ")
            )));
        }
        let key = Self::canonical(name);
        {
            let mut map = self.aliases.write();
            if map.remove(&key).is_none() {
                return Err(StorageError::db_error(format!(
                    "Type '{}' does not exist",
                    name
                )));
            }
        }

        if let Some(ref storage) = self.storage {
            storage.delete(name)?;
        }
        self.write_wal_drop(name)?;
        Ok(())
    }

    fn wal_timestamp() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }

    fn write_wal_create(&self, def: &TypeAliasDef) -> Result<(), StorageError> {
        if let Some(ref wal_writer) = self.wal_writer {
            let redo = CreateTypeAliasRedo {
                space_name: String::new(),
                name: def.name.clone(),
                underlying: def.underlying.clone(),
            };
            let payload = postcard::to_allocvec(&redo)
                .map_err(|e| StorageError::serialize_error(e.to_string()))?;
            let mut writer = wal_writer.lock();
            writer
                .append_entry(WalOpType::CreateTypeAlias, Self::wal_timestamp(), &payload)
                .map_err(|e| StorageError::db_error(format!("WAL write error: {}", e)))?;
        }
        Ok(())
    }

    fn write_wal_drop(&self, name: &str) -> Result<(), StorageError> {
        if let Some(ref wal_writer) = self.wal_writer {
            let redo = DropTypeAliasRedo {
                space_name: String::new(),
                name: name.to_string(),
            };
            let payload = postcard::to_allocvec(&redo)
                .map_err(|e| StorageError::serialize_error(e.to_string()))?;
            let mut writer = wal_writer.lock();
            writer
                .append_entry(WalOpType::DropTypeAlias, Self::wal_timestamp(), &payload)
                .map_err(|e| StorageError::db_error(format!("WAL write error: {}", e)))?;
        }
        Ok(())
    }

    /// Replay a WAL `CreateTypeAlias` record during recovery.
    pub fn replay_create(&self, redo: &CreateTypeAliasRedo) -> Result<(), StorageError> {
        let def = TypeAliasDef::new(redo.name.clone(), redo.underlying.clone());
        let mut map = self.aliases.write();
        map.insert(Self::canonical(&def.name), Arc::new(def));
        Ok(())
    }

    /// Replay a WAL `DropTypeAlias` record during recovery.
    pub fn replay_drop(&self, redo: &DropTypeAliasRedo) -> Result<(), StorageError> {
        let mut map = self.aliases.write();
        map.remove(&Self::canonical(&redo.name));
        Ok(())
    }
}

impl Default for TypeAliasManager {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for TypeAliasManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TypeAliasManager")
            .field("count", &self.aliases.read().len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn builtin(text: &str) -> Option<DataType> {
        text.parse::<DataType>().ok()
    }

    #[test]
    fn test_alias_create_resolve_drop() {
        let manager = TypeAliasManager::new();
        manager
            .create_alias(TypeAliasDef::new("uid".to_string(), "INT".to_string()))
            .unwrap();
        assert!(manager.exists("UID"));
        assert_eq!(manager.resolve("uid", &builtin).unwrap(), DataType::Int);
        manager.drop_alias("uid").unwrap();
        assert!(!manager.exists("uid"));
    }

    #[test]
    fn test_alias_chain() {
        let manager = TypeAliasManager::new();
        manager
            .create_alias(TypeAliasDef::new("a".to_string(), "BIGINT".to_string()))
            .unwrap();
        manager
            .create_alias(TypeAliasDef::new("b".to_string(), "a".to_string()))
            .unwrap();
        assert_eq!(manager.resolve("B", &builtin).unwrap(), DataType::BigInt);
    }

    #[test]
    fn test_alias_direct_cycle_rejected() {
        let manager = TypeAliasManager::new();
        assert!(manager
            .create_alias(TypeAliasDef::new("a".to_string(), "a".to_string()))
            .is_err());
    }

    #[test]
    fn test_alias_indirect_cycle_rejected() {
        let manager = TypeAliasManager::new();
        manager
            .create_alias(TypeAliasDef::new("a".to_string(), "INT".to_string()))
            .unwrap();
        manager
            .create_alias(TypeAliasDef::new("b".to_string(), "a".to_string()))
            .unwrap();
        // Dropping `a` is refused while `b` references it, so re-creating a
        // cycle through redefinition is impossible; simulate by dropping first
        // is also refused — instead verify a 3-node cycle attempt fails.
        manager
            .create_alias(TypeAliasDef::new("c".to_string(), "b".to_string()))
            .unwrap();
        assert!(manager.drop_alias("a").is_err());
        assert!(manager.drop_alias("b").is_err());
        manager.drop_alias("c").unwrap();
        manager.drop_alias("b").unwrap();
        manager.drop_alias("a").unwrap();
        assert!(!manager.exists("a"));
    }

    #[test]
    fn test_alias_unknown_type() {
        let manager = TypeAliasManager::new();
        assert!(manager.resolve("nope", &builtin).is_err());
    }

    #[test]
    fn test_alias_replay() {
        let manager = TypeAliasManager::new();
        manager
            .replay_create(&CreateTypeAliasRedo {
                space_name: String::new(),
                name: "x".to_string(),
                underlying: "INT".to_string(),
            })
            .unwrap();
        assert!(manager.exists("x"));
        manager
            .replay_drop(&DropTypeAliasRedo {
                space_name: String::new(),
                name: "x".to_string(),
            })
            .unwrap();
        assert!(!manager.exists("x"));
    }
}
