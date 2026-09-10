//! UDF plugin interface.
//!
//! A dynamically loaded library must export `udf_create` / `udf_destroy`
//! and may optionally export `udf_abi_version` for compatibility checks.

use crate::executor::expression::{ExpressionError, ExpressionErrorType};
use graphdb_core::Value;

/// Factory function type exported by a UDF dynamic library.
#[allow(improper_ctypes_definitions)]
pub type UdfCreateFn = unsafe extern "C" fn() -> *mut dyn UdfPlugin;

/// Destructor function type exported by a UDF dynamic library.
#[allow(improper_ctypes_definitions)]
pub type UdfDestroyFn = unsafe extern "C" fn(*mut dyn UdfPlugin);

/// Optional ABI version probe exported by a UDF dynamic library.
pub type UdfAbiVersionFn = unsafe extern "C" fn() -> u32;

/// Symbol name of the plugin factory function.
pub const UDF_CREATE_SYMBOL: &str = "udf_create";
/// Symbol name of the plugin destructor function.
pub const UDF_DESTROY_SYMBOL: &str = "udf_destroy";
/// Symbol name of the optional ABI version probe.
pub const UDF_ABI_VERSION_SYMBOL: &str = "udf_abi_version";

/// ABI version implemented by the host. Plugins reporting a different
/// version through `udf_abi_version` are rejected at load time.
pub const UDF_ABI_VERSION: u32 = 1;

/// Plugin interface implemented by user defined functions.
///
/// The trait mirrors the metadata required by `CustomFunction` so a plugin
/// can be registered without additional glue code. Only argument values and
/// the return value cross the library boundary; plugins never observe
/// database internals.
pub trait UdfPlugin: Send + Sync {
    /// Function name used for registration (case-insensitive).
    fn name(&self) -> &str;

    /// Human readable description.
    fn description(&self) -> &str;

    /// Minimum number of accepted arguments.
    fn min_arity(&self) -> usize;

    /// Maximum number of accepted arguments.
    fn max_arity(&self) -> usize;

    /// Whether the function is deterministic (same input always yields same output).
    fn is_pure(&self) -> bool;

    /// Execute the function.
    fn execute(&self, args: &[Value]) -> Result<Value, ExpressionError>;
}

/// Shared ownership handle used to register a plugin as a custom function.
pub type SharedPlugin = std::sync::Arc<dyn UdfPlugin>;

/// Validate the argument count against the plugin-declared range.
pub fn check_arity(plugin: &dyn UdfPlugin, args: &[Value]) -> Result<(), ExpressionError> {
    if args.len() < plugin.min_arity() || args.len() > plugin.max_arity() {
        return Err(ExpressionError::new(
            ExpressionErrorType::InvalidArgumentCount,
            format!(
                "function '{}' expects {}..={} argument(s), got {}",
                plugin.name(),
                plugin.min_arity(),
                plugin.max_arity(),
                args.len(),
            ),
        ));
    }
    Ok(())
}
