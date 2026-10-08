use super::super::udf::{LoadedPlugin, SharedPlugin, UdfError, UdfLoader};
use super::super::CustomFunction;
use super::FunctionRegistry;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

/// Metadata kept for each dynamically loaded UDF library.
///
/// The shared plugin handle keeps the underlying `Library` alive, so entries
/// must be removed through `unload_dynamic_udf` (dropping the entry unloads
/// the library).
pub struct LoadedExtension {
    /// Upper-cased function name.
    pub name: String,
    /// Canonical library path.
    pub path: PathBuf,
    /// Modification time observed at load, used by reload decisions.
    pub loaded_mtime: Option<SystemTime>,
    /// Shared plugin handle.
    pub plugin: SharedPlugin,
}

impl std::fmt::Debug for LoadedExtension {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoadedExtension")
            .field("name", &self.name)
            .field("path", &self.path)
            .finish()
    }
}
/// Snapshot describing a loaded dynamic UDF for listing purposes.
#[derive(Debug, Clone, PartialEq)]
pub struct DynamicUdfInfo {
    pub name: String,
    pub path: String,
    pub description: String,
    pub min_arity: usize,
    pub max_arity: usize,
    pub is_pure: bool,
}

impl FunctionRegistry {
    /// Modification time of a loaded dynamic UDF library, when recorded.
    pub fn dynamic_udf_mtime(&self, name: &str) -> Option<SystemTime> {
        self.dynamic_libraries
            .get(&name.to_uppercase())
            .and_then(|ext| ext.loaded_mtime)
    }
    /// Load a UDF dynamic library and register the exported function.
    ///
    /// Returns the registered (original-case) function name. The library
    /// handle is retained until `unload_dynamic_udf` is called.
    pub fn load_dynamic_udf(&mut self, path: &Path) -> Result<String, UdfError> {
        let loaded = UdfLoader::load(path)?;
        self.register_loaded_plugin(loaded)
    }

    /// Install a UDF from an `INSTALL EXTENSION ... FROM` source.
    ///
    /// Local file paths load directly; `http(s)://` sources return
    /// `RepoDownloadUnsupported` until a repository module is added.
    pub fn install_dynamic_udf(&mut self, source: &str) -> Result<String, UdfError> {
        let loaded = UdfLoader::install_from_source(source)?;
        self.register_loaded_plugin(loaded)
    }
    fn register_loaded_plugin(&mut self, loaded: LoadedPlugin) -> Result<String, UdfError> {
        let name = loaded.plugin.name().to_string();
        let upper = name.to_uppercase();
        if self.builtin_functions.contains_key(&upper) {
            return Err(UdfError::BuiltinConflict(name));
        }
        if let Some(existing) = self.dynamic_libraries.get(&upper) {
            return Err(UdfError::AlreadyLoaded(
                name,
                existing.path.to_string_lossy().to_string(),
            ));
        }
        if let Some(existing) = self.custom_functions.get(&upper) {
            if !existing.is_dynamic() {
                return Err(UdfError::AlreadyLoaded(
                    name,
                    "custom function registry".to_string(),
                ));
            }
        }
        let func = CustomFunction::new_dynamic(Arc::clone(&loaded.plugin));
        self.register_custom_full(func);
        self.dynamic_libraries.insert(
            upper,
            LoadedExtension {
                name: name.clone(),
                path: loaded.path,
                loaded_mtime: loaded.loaded_mtime,
                plugin: loaded.plugin,
            },
        );
        Ok(name)
    }
    /// Register an in-process plugin without a dynamic library.
    ///
    /// Primarily intended for tests and embeddings that build the plugin
    /// in the host binary; production libraries go through `load_dynamic_udf`.
    pub fn register_dynamic_plugin(&mut self, plugin: SharedPlugin) -> Result<String, UdfError> {
        let name = plugin.name().to_string();
        if name.trim().is_empty() {
            return Err(UdfError::InvalidName("<in-process>".to_string()));
        }
        let upper = name.to_uppercase();
        if self.builtin_functions.contains_key(&upper) {
            return Err(UdfError::BuiltinConflict(name));
        }
        if self.dynamic_libraries.contains_key(&upper) {
            let existing = &self.dynamic_libraries[&upper];
            return Err(UdfError::AlreadyLoaded(
                name,
                existing.path.to_string_lossy().to_string(),
            ));
        }
        let func = CustomFunction::new_dynamic(Arc::clone(&plugin));
        self.register_custom_full(func);
        self.dynamic_libraries.insert(
            upper,
            LoadedExtension {
                name: name.clone(),
                path: PathBuf::from("<in-process>"),
                loaded_mtime: None,
                plugin,
            },
        );
        Ok(name)
    }
    /// Unload a previously loaded dynamic UDF by function name.
    pub fn unload_dynamic_udf(&mut self, name: &str) -> Result<(), UdfError> {
        let upper = name.to_uppercase();
        let extension = self
            .dynamic_libraries
            .remove(&upper)
            .ok_or_else(|| UdfError::NotLoaded(name.to_string()))?;
        let _ = extension;
        // Only remove the registry entry if it still points at a dynamic
        // function; a newer non-dynamic registration must be preserved.
        let remove = self
            .custom_functions
            .get(&upper)
            .map(|f| f.is_dynamic())
            .unwrap_or(false);
        if remove {
            self.custom_functions.remove(&upper);
        }
        Ok(())
    }
    /// Reload a dynamic UDF from its original library path.
    ///
    /// Returns `true` when the library was reloaded, `false` when the file
    /// is unchanged since the initial load and reloading was skipped.
    /// In-process plugins (no library path) always report `false`.
    pub fn reload_dynamic_udf(&mut self, name: &str) -> Result<bool, UdfError> {
        let upper = name.to_uppercase();
        let current_mtime = self
            .dynamic_libraries
            .get(&upper)
            .ok_or_else(|| UdfError::NotLoaded(name.to_string()))?
            .loaded_mtime;
        let path = self.dynamic_libraries[&upper].path.clone();
        if path.to_string_lossy() == "<in-process>" {
            return Ok(false);
        }
        let current_file_mtime = std::fs::metadata(&path)
            .ok()
            .and_then(|m| m.modified().ok());
        if current_mtime.is_some() && current_mtime == current_file_mtime {
            return Ok(false);
        }
        // Load the replacement before dropping the old one so a failed
        // reload keeps the previous version active.
        let loaded = UdfLoader::load(&path)?;
        let func = CustomFunction::new_dynamic(Arc::clone(&loaded.plugin));
        let registered_name = loaded.plugin.name().to_string();
        self.register_custom_full(func);
        self.dynamic_libraries.insert(
            upper,
            LoadedExtension {
                name: registered_name,
                path: loaded.path,
                loaded_mtime: loaded.loaded_mtime,
                plugin: loaded.plugin,
            },
        );
        Ok(true)
    }
    /// Check whether a function name resolves to a dynamic UDF.
    pub fn is_dynamic(&self, name: &str) -> bool {
        self.dynamic_libraries.contains_key(&name.to_uppercase())
    }
    /// List all loaded dynamic UDFs ordered by name.
    pub fn list_dynamic_udfs(&self) -> Vec<DynamicUdfInfo> {
        let mut infos: Vec<DynamicUdfInfo> = self
            .dynamic_libraries
            .values()
            .map(|ext| DynamicUdfInfo {
                name: ext.name.clone(),
                path: ext.path.to_string_lossy().to_string(),
                description: ext.plugin.description().to_string(),
                min_arity: ext.plugin.min_arity(),
                max_arity: ext.plugin.max_arity(),
                is_pure: ext.plugin.is_pure(),
            })
            .collect();
        infos.sort_by(|a, b| a.name.cmp(&b.name));
        infos
    }
}
