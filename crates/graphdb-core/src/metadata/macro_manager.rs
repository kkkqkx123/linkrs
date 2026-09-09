use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::types::expr::Expression;
use crate::wal::redo::{CreateMacroRedo, DropMacroRedo};
use crate::wal::traits::WalWriter;
use crate::wal::types::WalOpType;
use crate::StorageError;

/// A single macro parameter with an optional default expression.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MacroParamDef {
    /// Parameter name (case-insensitive at expansion time).
    pub name: String,
    /// Default value used when the call site omits this argument.
    pub default: Option<Expression>,
}

/// A user-defined macro: a named, parameterized expression template.
///
/// Macros are pure query-time sugar: they are stored as parsed [`Expression`]
/// trees and expanded by the binder before planning. The executor never sees
/// a macro.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MacroDef {
    /// Macro name (stored as written; looked up case-insensitively).
    pub name: String,
    /// Ordered parameter list.
    pub params: Vec<MacroParamDef>,
    /// Macro body template. Parameter references are `Expression::Variable`
    /// nodes naming a parameter; expansion substitutes call-site arguments.
    pub body: Expression,
}

impl MacroDef {
    pub fn new(name: String, params: Vec<MacroParamDef>, body: Expression) -> Self {
        Self { name, params, body }
    }

    /// Number of required parameters (those without defaults).
    pub fn required_params(&self) -> usize {
        self.params.iter().filter(|p| p.default.is_none()).count()
    }
}

/// Trait for macro persistence backends.
///
/// The storage engine can implement this trait against its catalog KV area;
/// until then [`MemMacroStorage`] provides the default in-memory backend.
pub trait MacroStorage: Send + Sync {
    /// Load all macro definitions from storage.
    fn load_all(&self) -> Result<Vec<MacroDef>, StorageError>;
    /// Save (insert or overwrite) a macro definition.
    fn save(&self, def: &MacroDef) -> Result<(), StorageError>;
    /// Delete a macro definition by name.
    fn delete(&self, name: &str) -> Result<(), StorageError>;
}

/// In-memory [`MacroStorage`] backend (no durability beyond WAL).
#[derive(Debug, Default)]
pub struct MemMacroStorage {
    data: RwLock<HashMap<String, MacroDef>>,
}

impl MemMacroStorage {
    pub fn new() -> Self {
        Self {
            data: RwLock::new(HashMap::new()),
        }
    }
}

impl MacroStorage for MemMacroStorage {
    fn load_all(&self) -> Result<Vec<MacroDef>, StorageError> {
        Ok(self.data.read().values().cloned().collect())
    }

    fn save(&self, def: &MacroDef) -> Result<(), StorageError> {
        self.data.write().insert(def.name.clone(), def.clone());
        Ok(())
    }

    fn delete(&self, name: &str) -> Result<(), StorageError> {
        self.data.write().remove(name);
        Ok(())
    }
}

/// Macro catalog manager.
///
/// Owns the in-memory macro map with optional persistence and WAL backing,
/// mirroring [`super::sequence_manager::SequenceManager`].
/// Thread-safe via `parking_lot::RwLock`.
pub struct MacroManager {
    macros: RwLock<HashMap<String, Arc<MacroDef>>>,
    storage: Option<Arc<dyn MacroStorage>>,
    wal_writer: Option<Arc<parking_lot::Mutex<dyn WalWriter>>>,
}

impl MacroManager {
    /// Create a new MacroManager without persistence.
    pub fn new() -> Self {
        Self {
            macros: RwLock::new(HashMap::new()),
            storage: None,
            wal_writer: None,
        }
    }

    /// Create a new MacroManager with a persistence backend.
    pub fn with_storage(storage: Arc<dyn MacroStorage>) -> Self {
        Self {
            macros: RwLock::new(HashMap::new()),
            storage: Some(storage),
            wal_writer: None,
        }
    }

    /// Create a new MacroManager with persistence and WAL support.
    pub fn with_wal(
        storage: Arc<dyn MacroStorage>,
        wal_writer: Arc<parking_lot::Mutex<dyn WalWriter>>,
    ) -> Self {
        Self {
            macros: RwLock::new(HashMap::new()),
            storage: Some(storage),
            wal_writer: Some(wal_writer),
        }
    }

    fn canonical(name: &str) -> String {
        name.to_ascii_uppercase()
    }

    /// Initialize by loading all macros from storage.
    pub fn initialize(&self) -> Result<(), StorageError> {
        if let Some(ref storage) = self.storage {
            let defs = storage.load_all()?;
            let mut map = self.macros.write();
            for def in defs {
                map.insert(Self::canonical(&def.name), Arc::new(def));
            }
        }
        Ok(())
    }

    /// Validate a macro definition structurally (no catalog access).
    ///
    /// Checks: non-empty name, no duplicate parameter names, and that the
    /// body only references declared parameters or globals. Unknown
    /// `Expression::Variable` leaves that are not parameters are treated as
    /// query variables (allowed); direct self-reference by macro name in a
    /// nested call is rejected at expansion time.
    pub fn validate_definition(def: &MacroDef) -> Result<(), StorageError> {
        if def.name.trim().is_empty() {
            return Err(StorageError::db_error("Macro name must not be empty"));
        }
        let mut seen = std::collections::HashSet::new();
        for param in &def.params {
            if param.name.trim().is_empty() {
                return Err(StorageError::db_error(format!(
                    "Macro '{}' has an empty parameter name",
                    def.name
                )));
            }
            let key = param.name.to_ascii_uppercase();
            if !seen.insert(key) {
                return Err(StorageError::db_error(format!(
                    "Macro '{}' has duplicate parameter '{}'",
                    def.name, param.name
                )));
            }
        }
        Ok(())
    }

    /// Create a new macro definition.
    pub fn create_macro(&self, def: MacroDef) -> Result<(), StorageError> {
        Self::validate_definition(&def)?;
        let key = Self::canonical(&def.name);
        {
            let map = self.macros.read();
            if map.contains_key(&key) {
                return Err(StorageError::db_error(format!(
                    "Macro '{}' already exists",
                    def.name
                )));
            }
        }

        if let Some(ref storage) = self.storage {
            storage.save(&def)?;
        }
        self.write_wal_create(&def)?;

        let mut map = self.macros.write();
        // Winner-takes-all under concurrency: second writer reports exists.
        if map.contains_key(&key) {
            return Err(StorageError::db_error(format!(
                "Macro '{}' already exists",
                def.name
            )));
        }
        map.insert(key, Arc::new(def));
        Ok(())
    }

    /// Drop (delete) a macro definition.
    pub fn drop_macro(&self, name: &str) -> Result<(), StorageError> {
        let key = Self::canonical(name);
        {
            let mut map = self.macros.write();
            if map.remove(&key).is_none() {
                return Err(StorageError::db_error(format!(
                    "Macro '{}' does not exist",
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

    /// Look up a macro definition by name (case-insensitive).
    pub fn get_macro(&self, name: &str) -> Option<Arc<MacroDef>> {
        let map = self.macros.read();
        map.get(&Self::canonical(name)).cloned()
    }

    /// Check if a macro exists (case-insensitive).
    pub fn exists(&self, name: &str) -> bool {
        let map = self.macros.read();
        map.contains_key(&Self::canonical(name))
    }

    /// List all macro definitions, ordered by name for stable output.
    pub fn list_macros(&self) -> Vec<Arc<MacroDef>> {
        let map = self.macros.read();
        let mut defs: Vec<Arc<MacroDef>> = map.values().cloned().collect();
        defs.sort_by(|a, b| a.name.cmp(&b.name));
        defs
    }

    fn wal_timestamp() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }

    fn write_wal_create(&self, def: &MacroDef) -> Result<(), StorageError> {
        if let Some(ref wal_writer) = self.wal_writer {
            let redo = CreateMacroRedo {
                space_name: String::new(),
                name: def.name.clone(),
                params: def.params.clone(),
                body: def.body.clone(),
            };
            let payload = postcard::to_allocvec(&redo)
                .map_err(|e| StorageError::serialize_error(e.to_string()))?;
            let mut writer = wal_writer.lock();
            writer
                .append_entry(WalOpType::CreateMacro, Self::wal_timestamp(), &payload)
                .map_err(|e| StorageError::db_error(format!("WAL write error: {}", e)))?;
        }
        Ok(())
    }

    fn write_wal_drop(&self, name: &str) -> Result<(), StorageError> {
        if let Some(ref wal_writer) = self.wal_writer {
            let redo = DropMacroRedo {
                space_name: String::new(),
                name: name.to_string(),
            };
            let payload = postcard::to_allocvec(&redo)
                .map_err(|e| StorageError::serialize_error(e.to_string()))?;
            let mut writer = wal_writer.lock();
            writer
                .append_entry(WalOpType::DropMacro, Self::wal_timestamp(), &payload)
                .map_err(|e| StorageError::db_error(format!("WAL write error: {}", e)))?;
        }
        Ok(())
    }

    /// Replay a WAL `CreateMacro` record during recovery.
    pub fn replay_create(&self, redo: &CreateMacroRedo) -> Result<(), StorageError> {
        let def = MacroDef::new(redo.name.clone(), redo.params.clone(), redo.body.clone());
        Self::validate_definition(&def)?;
        let mut map = self.macros.write();
        map.insert(Self::canonical(&def.name), Arc::new(def));
        Ok(())
    }

    /// Replay a WAL `DropMacro` record during recovery.
    pub fn replay_drop(&self, redo: &DropMacroRedo) -> Result<(), StorageError> {
        let mut map = self.macros.write();
        map.remove(&Self::canonical(&redo.name));
        Ok(())
    }
}

impl Default for MacroManager {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for MacroManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MacroManager")
            .field("count", &self.macros.read().len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::expr::Expression;

    fn sample_def(name: &str) -> MacroDef {
        MacroDef::new(
            name.to_string(),
            vec![MacroParamDef {
                name: "x".to_string(),
                default: None,
            }],
            Expression::Binary {
                left: Box::new(Expression::variable("x")),
                op: crate::types::operators::BinaryOperator::Multiply,
                right: Box::new(Expression::literal(crate::Value::Int(2))),
            },
        )
    }

    #[test]
    fn test_macro_create_get_drop() {
        let manager = MacroManager::new();
        manager.create_macro(sample_def("double")).unwrap();
        assert!(manager.exists("double"));
        assert!(manager.exists("DOUBLE"));
        let def = manager.get_macro("Double").unwrap();
        assert_eq!(def.params.len(), 1);
        assert_eq!(def.required_params(), 1);
        manager.drop_macro("double").unwrap();
        assert!(!manager.exists("double"));
    }

    #[test]
    fn test_macro_duplicate_create() {
        let manager = MacroManager::new();
        manager.create_macro(sample_def("m")).unwrap();
        assert!(manager.create_macro(sample_def("M")).is_err());
    }

    #[test]
    fn test_macro_drop_missing() {
        let manager = MacroManager::new();
        assert!(manager.drop_macro("nope").is_err());
    }

    #[test]
    fn test_macro_duplicate_params_rejected() {
        let mut def = sample_def("bad");
        def.params.push(MacroParamDef {
            name: "X".to_string(),
            default: None,
        });
        assert!(MacroManager::validate_definition(&def).is_err());
    }

    #[test]
    fn test_macro_list_sorted() {
        let manager = MacroManager::new();
        manager.create_macro(sample_def("b")).unwrap();
        manager.create_macro(sample_def("a")).unwrap();
        let names: Vec<String> = manager
            .list_macros()
            .iter()
            .map(|d| d.name.clone())
            .collect();
        assert_eq!(names, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn test_macro_with_storage() {
        let storage = Arc::new(MemMacroStorage::new());
        let manager = MacroManager::with_storage(storage);
        manager.create_macro(sample_def("m1")).unwrap();
        assert!(manager.exists("m1"));
        manager.initialize().unwrap();
        assert!(manager.exists("m1"));
    }

    #[test]
    fn test_macro_replay() {
        let manager = MacroManager::new();
        let def = sample_def("r");
        manager
            .replay_create(&CreateMacroRedo {
                space_name: String::new(),
                name: def.name.clone(),
                params: def.params.clone(),
                body: def.body.clone(),
            })
            .unwrap();
        assert!(manager.exists("r"));
        manager
            .replay_drop(&DropMacroRedo {
                space_name: String::new(),
                name: "r".to_string(),
            })
            .unwrap();
        assert!(!manager.exists("r"));
    }
}
