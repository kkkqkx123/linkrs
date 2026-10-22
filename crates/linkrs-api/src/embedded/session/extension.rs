//! UDF / extension management for an embedded session.
//!
//! These methods operate on the process-wide function registry shared with the
//! query engine; they never reach the query planner. `execute_extension`
//! emits an admin message through the shared `single_message_result` helper
//! owned by `state`.

use super::Session;
use crate::api_core::{CoreError, CoreResult};
use linkrs_query::executor::expression::functions::registry::global_registry;
use linkrs_query::executor::expression::functions::registry::DynamicUdfInfo;
use linkrs_query::executor::expression::functions::CustomFunction;
use linkrs_query::parser::ast::stmt::ExtensionAction;
use linkrs_query::parser::ast::stmt::ExtensionStmt;
use linkrs_storage::StorageClient;
use std::path::Path;

impl<S: StorageClient + Clone + Send + Sync + 'static + linkrs_storage::UndoTarget> Session<S> {
    /// Register a custom function
    pub fn register_custom_function(&self, function: CustomFunction) -> CoreResult<()> {
        let mut registry = global_registry().write();
        registry.register_custom_full(function);
        Ok(())
    }

    /// Load a UDF dynamic library and register the exported function.
    ///
    /// Returns the registered function name.
    pub fn load_extension(&self, path: &Path) -> CoreResult<String> {
        let mut registry = global_registry().write();
        registry
            .load_dynamic_udf(path)
            .map_err(|e| CoreError::InvalidParameter(e.to_string()))
    }

    /// Unload a previously loaded dynamic UDF by function name.
    pub fn unload_extension(&self, name: &str) -> CoreResult<()> {
        let mut registry = global_registry().write();
        registry
            .unload_dynamic_udf(name)
            .map_err(|e| CoreError::InvalidParameter(e.to_string()))
    }

    /// Reload a dynamic UDF from its original library path.
    ///
    /// Returns `true` when the library was reloaded, `false` when the file
    /// is unchanged and reloading was skipped.
    pub fn reload_extension(&self, name: &str) -> CoreResult<bool> {
        let mut registry = global_registry().write();
        registry
            .reload_dynamic_udf(name)
            .map_err(|e| CoreError::InvalidParameter(e.to_string()))
    }

    /// Install a UDF from an `INSTALL EXTENSION ... FROM` source.
    ///
    /// Local file paths load directly; `http(s)://` sources return a
    /// repository-unsupported error until a repository module is added.
    pub fn install_extension(&self, source: &str) -> CoreResult<String> {
        let mut registry = global_registry().write();
        registry
            .install_dynamic_udf(source)
            .map_err(|e| CoreError::InvalidParameter(e.to_string()))
    }

    /// List all loaded dynamic UDFs ordered by name.
    pub fn list_extensions(&self) -> Vec<DynamicUdfInfo> {
        global_registry().read().list_dynamic_udfs()
    }

    /// Execute an extension management statement
    /// (`LOAD EXTENSION` / `INSTALL EXTENSION` / `UNINSTALL EXTENSION` /
    /// `UPDATE EXTENSION`).
    ///
    /// Extension statements run directly against the session function
    /// registry and never reach the query planner.
    pub(crate) fn execute_extension(
        &self,
        extension: &ExtensionStmt,
    ) -> CoreResult<crate::embedded::result::QueryResult> {
        let message = match extension.action {
            ExtensionAction::Load => {
                let name = self.load_extension(Path::new(&extension.name))?;
                format!("Loaded extension '{name}' from '{}'", extension.name)
            }
            ExtensionAction::Uninstall => {
                self.unload_extension(&extension.name)?;
                format!("Uninstalled extension '{}'", extension.name)
            }
            ExtensionAction::Install => {
                let source = extension.source.clone().unwrap_or_default();
                let name = self.install_extension(&source)?;
                format!("Installed extension '{name}' from '{source}'")
            }
            ExtensionAction::Update => {
                let reloaded = self.reload_extension(&extension.name)?;
                if reloaded {
                    format!("Updated extension '{}'", extension.name)
                } else {
                    format!("Extension '{}' is already up to date", extension.name)
                }
            }
        };
        Ok(Self::single_message_result(message))
    }
}
