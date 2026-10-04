//! Custom (non-built-in) function definitions.
//!
//! Covers Rust closures, C ABI callbacks and dynamically loaded UDF plugins.

use crate::executor::expression::{ExpressionError, ExpressionErrorType};
use graphdb_core::value_conversion::core_value_to_graphdb;
use graphdb_core::Value;

use std::ffi::c_void;

use super::udf::{execute_isolated, SharedPlugin};
use super::ExpressionFunction;

/// C Function Context Structure (Opaque Pointers)
pub struct CFunctionContext {
    /// Result value
    pub result: Option<Value>,
    /// Error message
    pub error: Option<String>,
    /// Aggregation status (used for aggregate functions)
    pub aggregate_state: Option<Box<dyn std::any::Any + Send>>,
    /// User data pointer
    pub user_data: usize,
    /// Number of parameters
    pub argc: usize,
    /// Parameter array (converted to C API format)
    pub argv: Vec<graphdb_core::types::c_api::graphdb_value_t>,
}

impl Default for CFunctionContext {
    fn default() -> Self {
        Self::new()
    }
}

impl CFunctionContext {
    pub fn new() -> Self {
        Self {
            result: None,
            error: None,
            aggregate_state: None,
            user_data: 0,
            argc: 0,
            argv: Vec::new(),
        }
    }

    pub fn with_user_data(user_data: usize) -> Self {
        Self {
            result: None,
            error: None,
            aggregate_state: None,
            user_data,
            argc: 0,
            argv: Vec::new(),
        }
    }

    pub fn set_result(&mut self, value: Value) {
        self.result = Some(value);
    }

    pub fn set_error(&mut self, error: String) {
        self.error = Some(error);
    }

    /// Set the aggregation status
    pub fn set_aggregate_state<T: std::any::Any + Send + 'static>(&mut self, state: T) {
        self.aggregate_state = Some(Box::new(state));
    }

    /// Obtain the aggregated status.
    pub fn get_aggregate_state<T: std::any::Any + Send + 'static>(&self) -> Option<&T> {
        self.aggregate_state.as_ref()?.downcast_ref::<T>()
    }

    /// Obtain a variable reference to the aggregated status
    pub fn get_aggregate_state_mut<T: std::any::Any + Send + 'static>(&mut self) -> Option<&mut T> {
        self.aggregate_state.as_mut()?.downcast_mut::<T>()
    }
}

/// Scalar function callback type
pub type ScalarFunctionCallback =
    extern "C" fn(*mut CFunctionContext, i32, *mut graphdb_core::types::c_api::graphdb_value_t);

/// Aggregation step callback type
pub type AggregateStepCallback =
    extern "C" fn(*mut CFunctionContext, i32, *mut graphdb_core::types::c_api::graphdb_value_t);

/// Aggregate final callback type
pub type AggregateFinalCallback = extern "C" fn(*mut CFunctionContext);

/// Implementation of custom functions and their types
#[derive(Clone)]
pub enum CustomFunctionImpl {
    /// Custom functions implemented in Rust
    Rust(fn(&[Value]) -> Result<Value, ExpressionError>),
    /// A scalar function implemented using a C callback
    C {
        /// Scalar function callback (stores the address of the function pointer)
        scalar_callback: usize,
        /// User data (storage pointer addresses)
        user_data: usize,
    },
    /// Aggregate functions implemented using C callbacks
    Aggregate {
        /// Aggregation step callback (pointer address to the storage function)
        step_callback: usize,
        /// Aggregated final callback (address of the stored function pointer)
        final_callback: usize,
        /// User data (storage pointer address)
        user_data: usize,
    },
    /// A function backed by a dynamically loaded UDF plugin.
    Dynamic {
        /// Shared plugin handle; keeps the dynamic library alive.
        plugin: SharedPlugin,
    },
}

impl std::fmt::Debug for CustomFunctionImpl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CustomFunctionImpl::Rust(_) => write!(f, "Rust closure"),
            CustomFunctionImpl::C { .. } => write!(f, "C scalar callback"),
            CustomFunctionImpl::Aggregate { .. } => write!(f, "C aggregate callback"),
            CustomFunctionImpl::Dynamic { plugin } => {
                write!(f, "Dynamic UDF '{}'", plugin.name())
            }
        }
    }
}

/// Custom function definition
#[derive(Debug, Clone)]
pub struct CustomFunction {
    /// Function name
    pub name: String,
    /// Number of parameters
    pub arity: usize,
    /// "Do you accept variable parameters?"
    pub is_variadic: bool,
    /// Function description
    pub description: String,
    /// Function implementation
    pub implementation: CustomFunctionImpl,
}

impl CustomFunction {
    /// Create a new custom Rust function.
    pub fn new_rust(
        name: impl Into<String>,
        arity: usize,
        is_variadic: bool,
        description: impl Into<String>,
        implementation: fn(&[Value]) -> Result<Value, ExpressionError>,
    ) -> Self {
        Self {
            name: name.into(),
            arity,
            is_variadic,
            description: description.into(),
            implementation: CustomFunctionImpl::Rust(implementation),
        }
    }

    /// Create a new custom C callback function.
    pub fn new_c(
        name: impl Into<String>,
        arity: usize,
        is_variadic: bool,
        description: impl Into<String>,
        scalar_callback: ScalarFunctionCallback,
        user_data: *mut c_void,
    ) -> Self {
        Self {
            name: name.into(),
            arity,
            is_variadic,
            description: description.into(),
            implementation: CustomFunctionImpl::C {
                scalar_callback: scalar_callback as usize,
                user_data: user_data as usize,
            },
        }
    }

    /// Create a new C callback aggregation function
    pub fn new_c_aggregate(
        name: impl Into<String>,
        arity: usize,
        is_variadic: bool,
        description: impl Into<String>,
        step_callback: AggregateStepCallback,
        final_callback: AggregateFinalCallback,
        user_data: *mut c_void,
    ) -> Self {
        Self {
            name: name.into(),
            arity,
            is_variadic,
            description: description.into(),
            implementation: CustomFunctionImpl::Aggregate {
                step_callback: step_callback as usize,
                final_callback: final_callback as usize,
                user_data: user_data as usize,
            },
        }
    }

    /// Create a custom function backed by a dynamically loaded UDF plugin.
    ///
    /// Arity metadata is read from the plugin so registration stays in sync
    /// with the library implementation.
    pub fn new_dynamic(plugin: SharedPlugin) -> Self {
        let (min_arity, max_arity) = (plugin.min_arity(), plugin.max_arity());
        Self {
            name: plugin.name().to_string(),
            arity: min_arity,
            is_variadic: min_arity != max_arity,
            description: plugin.description().to_string(),
            implementation: CustomFunctionImpl::Dynamic { plugin },
        }
    }

    /// Check whether the function is backed by a dynamic UDF plugin.
    pub fn is_dynamic(&self) -> bool {
        matches!(self.implementation, CustomFunctionImpl::Dynamic { .. })
    }

    /// Check whether it is an aggregate function.
    pub fn is_aggregate(&self) -> bool {
        matches!(self.implementation, CustomFunctionImpl::Aggregate { .. })
    }

    /// Execute the function
    pub fn execute(&self, args: &[Value]) -> Result<Value, ExpressionError> {
        match &self.implementation {
            CustomFunctionImpl::Rust(func) => func(args),
            CustomFunctionImpl::C {
                scalar_callback,
                user_data: _,
            } => {
                // Creating a C function context
                let mut ctx = CFunctionContext::new();
                ctx.argc = args.len();
                ctx.argv = args.iter().map(core_value_to_graphdb).collect();
                let ctx_ptr = &mut ctx as *mut CFunctionContext;

                // Convert a `usize` value back to a function pointer
                let callback: ScalarFunctionCallback =
                    unsafe { std::mem::transmute(*scalar_callback) };

                // Calling a C callback
                let argv_ptr = if ctx.argv.is_empty() {
                    std::ptr::null_mut()
                } else {
                    ctx.argv.as_mut_ptr()
                };
                callback(ctx_ptr, args.len() as i32, argv_ptr);

                // Check for errors.
                if let Some(error) = ctx.error {
                    return Err(ExpressionError::new(
                        ExpressionErrorType::FunctionExecutionError,
                        error,
                    ));
                }

                // Return the value set by the callback.
                ctx.result.ok_or_else(|| {
                    ExpressionError::new(
                        ExpressionErrorType::FunctionExecutionError,
                        format!("The function '{}' does not set a return value", self.name),
                    )
                })
            }
            CustomFunctionImpl::Aggregate { .. } => Err(ExpressionError::new(
                ExpressionErrorType::InvalidOperation,
                "Aggregation functions need to be executed within the aggregation context"
                    .to_string(),
            )),
            CustomFunctionImpl::Dynamic { plugin } => execute_isolated(plugin.as_ref(), args),
        }
    }
}

impl ExpressionFunction for CustomFunction {
    fn name(&self) -> &str {
        &self.name
    }

    fn arity(&self) -> usize {
        self.arity
    }

    fn is_variadic(&self) -> bool {
        self.is_variadic
    }

    fn execute(&self, args: &[Value]) -> Result<Value, ExpressionError> {
        self.execute(args)
    }

    fn description(&self) -> &str {
        &self.description
    }
}
