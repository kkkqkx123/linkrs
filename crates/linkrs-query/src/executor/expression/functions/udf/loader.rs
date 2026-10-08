//! Dynamic library loading for UDF plugins.

use super::dynamic::DynamicPlugin;
use super::error::UdfError;
use super::plugin::{
    SharedPlugin, UdfAbiVersionFn, UdfCreateFn, UdfDestroyFn, UdfPlugin, UDF_ABI_VERSION,
    UDF_ABI_VERSION_SYMBOL, UDF_CREATE_SYMBOL, UDF_DESTROY_SYMBOL,
};
use libloading::{Library, Symbol};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

/// A successfully loaded plugin together with its origin metadata.
pub struct LoadedPlugin {
    /// Shared handle registered in the function registry.
    pub plugin: SharedPlugin,
    /// Canonical library path.
    pub path: PathBuf,
    /// File modification time observed at load, used by reload decisions.
    pub loaded_mtime: Option<SystemTime>,
}

impl std::fmt::Debug for LoadedPlugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoadedPlugin")
            .field("name", &self.plugin.name())
            .field("path", &self.path)
            .finish()
    }
}

/// Loader for UDF dynamic libraries.
pub struct UdfLoader;

impl UdfLoader {
    /// Install a plugin from an `INSTALL EXTENSION ... FROM` source.
    ///
    /// Reservation point for a future repository module (download +
    /// checksum + signature verification). Local file paths load directly;
    /// `http(s)://` sources return `RepoDownloadUnsupported`.
    pub fn install_from_source(source: &str) -> Result<LoadedPlugin, UdfError> {
        let lower = source.to_ascii_lowercase();
        if lower.starts_with("http://") || lower.starts_with("https://") {
            return Err(UdfError::RepoDownloadUnsupported(source.to_string()));
        }
        Self::load(Path::new(source))
    }

    /// Load the plugin exported by the library at `path`.
    pub fn load(path: &Path) -> Result<LoadedPlugin, UdfError> {
        let canonical = Self::validate_path(path)?;
        let path_display = canonical.to_string_lossy().to_string();
        let loaded_mtime = std::fs::metadata(&canonical)
            .ok()
            .and_then(|m| m.modified().ok());

        let lib = unsafe { Library::new(&canonical) }
            .map_err(|e| UdfError::LibraryLoad(path_display.clone(), e.to_string()))?;
        let lib = Arc::new(lib);

        // Copy the raw function pointers out of the symbols so no
        // lifetime-bound `Symbol` escapes this function.
        let create: UdfCreateFn = unsafe {
            let symbol: Symbol<UdfCreateFn> =
                lib.get(UDF_CREATE_SYMBOL.as_bytes()).map_err(|_| {
                    UdfError::SymbolNotFound(path_display.clone(), UDF_CREATE_SYMBOL.into())
                })?;
            *symbol
        };
        let destroy: UdfDestroyFn = unsafe {
            let symbol: Symbol<UdfDestroyFn> =
                lib.get(UDF_DESTROY_SYMBOL.as_bytes()).map_err(|_| {
                    UdfError::SymbolNotFound(path_display.clone(), UDF_DESTROY_SYMBOL.into())
                })?;
            *symbol
        };

        // Optional ABI compatibility probe.
        if let Ok(symbol) = unsafe { lib.get::<UdfAbiVersionFn>(UDF_ABI_VERSION_SYMBOL.as_bytes()) }
        {
            let version_fn: UdfAbiVersionFn = *symbol;
            let reported = unsafe { version_fn() };
            if reported != UDF_ABI_VERSION {
                return Err(UdfError::VersionMismatch(
                    path_display,
                    UDF_ABI_VERSION,
                    reported,
                ));
            }
        }

        let raw = unsafe { create() };
        let owned = unsafe {
            DynamicPlugin::from_raw(raw, destroy, Arc::clone(&lib), path_display.clone())
        }?;
        if owned.name().trim().is_empty() {
            return Err(UdfError::InvalidName(path_display));
        }
        if owned.min_arity() > owned.max_arity() {
            return Err(UdfError::InvalidName(format!(
                "{path_display}: min_arity exceeds max_arity"
            )));
        }

        Ok(LoadedPlugin {
            plugin: Arc::new(owned),
            path: canonical,
            loaded_mtime,
        })
    }

    /// Reject missing files and platform-mismatched extensions early.
    fn validate_path(path: &Path) -> Result<PathBuf, UdfError> {
        let display = path.to_string_lossy().to_string();
        if !path.exists() {
            return Err(UdfError::InvalidPath(
                display,
                "file does not exist".to_string(),
            ));
        }
        let expected = Self::expected_extension();
        let actual = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        if actual != expected {
            return Err(UdfError::InvalidPath(
                display,
                format!("expected a '*.{expected}' dynamic library"),
            ));
        }
        path.canonicalize()
            .map_err(|e| UdfError::Io(display, e.to_string()))
    }

    fn expected_extension() -> &'static str {
        if cfg!(target_os = "windows") {
            "dll"
        } else if cfg!(target_os = "macos") {
            "dylib"
        } else {
            "so"
        }
    }
}
