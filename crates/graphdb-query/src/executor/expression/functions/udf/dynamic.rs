//! Owned handle for a plugin instance living in a dynamic library.
//!
//! The instance is created through `udf_create` and must be destroyed through
//! `udf_destroy` before the library handle is released. `DynamicPlugin` keeps
//! an `Arc<Library>` alive for exactly that long.

use super::error::UdfError;
use super::plugin::{check_arity, UdfDestroyFn, UdfPlugin};
use crate::executor::expression::{ExpressionError, ExpressionErrorType};
use graphdb_core::Value;
use libloading::Library;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;

/// Owned plugin instance backed by a dynamic library.
pub struct DynamicPlugin {
    raw: *mut dyn UdfPlugin,
    destroy: UdfDestroyFn,
    /// Kept alive so the vtable behind `raw` stays valid.
    _lib: Arc<Library>,
    /// Library path used in error messages.
    path: String,
}

// The plugin trait requires Send + Sync; the wrapper upholds the same bound
// because execution only crosses the boundary through `&self`.
unsafe impl Send for DynamicPlugin {}
unsafe impl Sync for DynamicPlugin {}

impl DynamicPlugin {
    /// Take ownership of a raw plugin pointer created by `udf_create`.
    ///
    /// # Safety
    /// `raw` must be a non-null pointer returned by the matching `udf_create`
    /// of the same library version, and `destroy` must be the matching
    /// `udf_destroy` symbol from that library.
    pub unsafe fn from_raw(
        raw: *mut dyn UdfPlugin,
        destroy: UdfDestroyFn,
        lib: Arc<Library>,
        path: String,
    ) -> Result<Self, UdfError> {
        if raw.is_null() {
            return Err(UdfError::NullPlugin(path));
        }
        Ok(Self {
            raw,
            destroy,
            _lib: lib,
            path,
        })
    }

    fn inner(&self) -> &dyn UdfPlugin {
        unsafe { &*self.raw }
    }
}

impl Drop for DynamicPlugin {
    fn drop(&mut self) {
        unsafe {
            (self.destroy)(self.raw);
        }
    }
}

impl std::fmt::Debug for DynamicPlugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DynamicPlugin")
            .field("name", &self.inner().name())
            .field("path", &self.path)
            .finish()
    }
}

impl UdfPlugin for DynamicPlugin {
    fn name(&self) -> &str {
        self.inner().name()
    }

    fn description(&self) -> &str {
        self.inner().description()
    }

    fn min_arity(&self) -> usize {
        self.inner().min_arity()
    }

    fn max_arity(&self) -> usize {
        self.inner().max_arity()
    }

    fn is_pure(&self) -> bool {
        self.inner().is_pure()
    }

    fn execute(&self, args: &[Value]) -> Result<Value, ExpressionError> {
        check_arity(self.inner(), args)?;
        let name = self.inner().name().to_string();
        match catch_unwind(AssertUnwindSafe(|| self.inner().execute(args))) {
            Ok(result) => result,
            Err(payload) => {
                let detail = payload
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_else(|| "unknown panic".to_string());
                Err(ExpressionError::new(
                    ExpressionErrorType::FunctionExecutionError,
                    UdfError::ExecutionPanic(name, detail).to_string(),
                ))
            }
        }
    }
}

/// Execute an in-process plugin with the same isolation guarantees.
///
/// Used by `CustomFunctionImpl::Dynamic` so both Library-backed and
/// in-process plugins share one execution path.
pub fn execute_isolated(plugin: &dyn UdfPlugin, args: &[Value]) -> Result<Value, ExpressionError> {
    check_arity(plugin, args)?;
    let name = plugin.name().to_string();
    match catch_unwind(AssertUnwindSafe(|| plugin.execute(args))) {
        Ok(result) => result,
        Err(payload) => {
            let detail = payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "unknown panic".to_string());
            Err(ExpressionError::new(
                ExpressionErrorType::FunctionExecutionError,
                UdfError::ExecutionPanic(name, detail).to_string(),
            ))
        }
    }
}
