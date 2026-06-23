//! Expression Function Module
//!
//! Provide the function definitions and implementations during the evaluation of expressions, including both built-in functions and custom functions.
//!
//! ## Module Structure
//!
//! - `signature.rs` - 类型签名系统
//! - `registry.rs` - 函数注册表
//! - `builtin/` – Implementation of built-in functions
//!
//! ## How to use it
//!
//! ```rust
//! use crate::query::executor::expression::functions::BuiltinFunction;
//!
//! let func = BuiltinFunction::Math(MathFunction::Abs);
//! let result = func.execute(&[Value::Int(-5)]);
//! ```

pub mod builtin;
pub mod registry;
pub mod signature;

// Full-text search functions
pub mod fulltext;
pub use fulltext::{FulltextExecutionContext, FulltextFunction};

// Vector functions (re-export from builtin)
pub use builtin::vector::VectorFunction;

pub use registry::{global_registry, global_registry_ref, FunctionRegistry};
pub use signature::ValueType;

// Reexport the function types from the built-in submodule.
pub use builtin::container::ContainerFunction;
pub use builtin::conversion::ConversionFunction;
pub use builtin::datetime::DateTimeFunction;
pub use builtin::geography::GeographyFunction;
pub use builtin::graph::GraphFunction;
pub use builtin::math::MathFunction;
pub use builtin::path::PathFunction;
pub use builtin::regex::RegexFunction;
pub use builtin::string::StringFunction;
pub use builtin::utility::UtilityFunction;

use crate::core::types::operators::AggregateFunction;
use crate::core::Value;
use crate::query::executor::expression::{ExpressionError, ExpressionErrorType};

use std::ffi::c_void;

use crate::core::utils::value_conversion::core_value_to_graphdb;

/// Function reference enumeration, used to reference functions in expressions
#[derive(Debug, Clone)]
pub enum FunctionRef<'a> {
    /// Reference to built-in functions
    Builtin(&'a BuiltinFunction),
    /// Reference to a custom function
    Custom(&'a CustomFunction),
}

/// A function reference that possesses ownership
#[derive(Debug, Clone)]
pub enum OwnedFunctionRef {
    /// Reference to an internal function (with ownership)
    Builtin(BuiltinFunction),
    /// Reference to a custom function (with ownership)
    Custom(CustomFunction),
}

impl<'a> From<FunctionRef<'a>> for OwnedFunctionRef {
    fn from(func_ref: FunctionRef<'a>) -> Self {
        match func_ref {
            FunctionRef::Builtin(f) => OwnedFunctionRef::Builtin(f.clone()),
            FunctionRef::Custom(f) => OwnedFunctionRef::Custom(f.clone()),
        }
    }
}

impl OwnedFunctionRef {
    pub fn name(&self) -> &str {
        match self {
            OwnedFunctionRef::Builtin(f) => f.name(),
            OwnedFunctionRef::Custom(f) => f.name(),
        }
    }

    pub fn execute(&self, args: &[Value]) -> Result<Value, ExpressionError> {
        match self {
            OwnedFunctionRef::Builtin(f) => f.execute(args),
            OwnedFunctionRef::Custom(f) => f.execute(args),
        }
    }

    pub fn execute_with_cache(
        &self,
        args: &[Value],
        _cache: &mut (),
    ) -> Result<Value, ExpressionError> {
        match self {
            OwnedFunctionRef::Builtin(f) => f.execute_with_cache(args, _cache),
            OwnedFunctionRef::Custom(f) => f.execute(args), // Custom functions don't have cache
        }
    }
}

/// Expression function characteristics
pub trait ExpressionFunction: Send + Sync {
    /// Obtain the function name
    fn name(&self) -> &str;

    /// Determine the number of parameters
    fn arity(&self) -> usize;

    /// Check whether variable parameters are accepted.
    fn is_variadic(&self) -> bool;

    /// Execute the function
    fn execute(&self, args: &[Value]) -> Result<Value, ExpressionError>;

    /// Obtain the function description
    fn description(&self) -> &str;
}

/// Built-in function types to avoid dynamic distribution.
#[derive(Debug, Clone)]
pub enum BuiltinFunction {
    /// Mathematical functions
    Math(MathFunction),
    /// String functions
    String(StringFunction),
    /// Regular Expression Functions
    Regex(RegexFunction),
    /// Aggregate functions
    Aggregate(AggregateFunction),
    /// Type conversion functions
    Conversion(ConversionFunction),
    /// Date and time functions
    DateTime(DateTimeFunction),
    /// Geospatial functions
    Geography(GeographyFunction),
    /// Practical functions
    Utility(UtilityFunction),
    /// Graph-related functions
    Graph(GraphFunction),
    /// Container operation functions
    Container(ContainerFunction),
    /// Path function
    Path(PathFunction),
    /// Full-text search functions
    Fulltext(FulltextFunction),
    /// Vector functions
    Vector(VectorFunction),
}

impl BuiltinFunction {
    /// Get function name
    pub fn name(&self) -> &str {
        match self {
            BuiltinFunction::Math(f) => f.name(),
            BuiltinFunction::String(f) => f.name(),
            BuiltinFunction::Regex(f) => f.name(),
            BuiltinFunction::Aggregate(f) => f.name(),
            BuiltinFunction::Conversion(f) => f.name(),
            BuiltinFunction::DateTime(f) => f.name(),
            BuiltinFunction::Geography(f) => f.name(),
            BuiltinFunction::Utility(f) => f.name(),
            BuiltinFunction::Graph(f) => f.name(),
            BuiltinFunction::Container(f) => f.name(),
            BuiltinFunction::Path(f) => f.name(),
            BuiltinFunction::Fulltext(f) => f.name(),
            BuiltinFunction::Vector(f) => f.name(),
        }
    }

    /// Get the number of parameters
    pub fn arity(&self) -> usize {
        match self {
            BuiltinFunction::Math(f) => f.arity(),
            BuiltinFunction::String(f) => f.arity(),
            BuiltinFunction::Regex(f) => f.arity(),
            BuiltinFunction::Aggregate(f) => f.arity(),
            BuiltinFunction::Conversion(f) => f.arity(),
            BuiltinFunction::DateTime(f) => f.arity(),
            BuiltinFunction::Geography(f) => f.arity(),
            BuiltinFunction::Utility(f) => f.arity(),
            BuiltinFunction::Graph(f) => f.arity(),
            BuiltinFunction::Container(f) => f.arity(),
            BuiltinFunction::Path(f) => f.arity(),
            BuiltinFunction::Fulltext(_) => 0,
            BuiltinFunction::Vector(f) => f.arity(),
        }
    }

    /// Check if variable parameters are accepted
    pub fn is_variadic(&self) -> bool {
        match self {
            BuiltinFunction::Math(f) => f.is_variadic(),
            BuiltinFunction::String(f) => f.is_variadic(),
            BuiltinFunction::Regex(f) => f.is_variadic(),
            BuiltinFunction::Aggregate(f) => f.is_variadic(),
            BuiltinFunction::Conversion(f) => f.is_variadic(),
            BuiltinFunction::DateTime(f) => f.is_variadic(),
            BuiltinFunction::Geography(f) => f.is_variadic(),
            BuiltinFunction::Utility(f) => f.is_variadic(),
            BuiltinFunction::Graph(f) => f.is_variadic(),
            BuiltinFunction::Container(f) => f.is_variadic(),
            BuiltinFunction::Path(f) => f.is_variadic(),
            BuiltinFunction::Fulltext(f) => f.is_variadic(),
            BuiltinFunction::Vector(f) => f.is_variadic(),
        }
    }

    /// Get function description
    pub fn description(&self) -> &str {
        match self {
            BuiltinFunction::Math(f) => f.description(),
            BuiltinFunction::String(f) => f.description(),
            BuiltinFunction::Regex(f) => f.description(),
            BuiltinFunction::Aggregate(f) => f.description(),
            BuiltinFunction::Conversion(f) => f.description(),
            BuiltinFunction::DateTime(f) => f.description(),
            BuiltinFunction::Geography(f) => f.description(),
            BuiltinFunction::Utility(f) => f.description(),
            BuiltinFunction::Graph(f) => f.description(),
            BuiltinFunction::Container(f) => f.description(),
            BuiltinFunction::Path(f) => f.description(),
            BuiltinFunction::Fulltext(f) => f.description(),
            BuiltinFunction::Vector(f) => f.description(),
        }
    }

    /// executable function
    pub fn execute(&self, args: &[Value]) -> Result<Value, ExpressionError> {
        match self {
            BuiltinFunction::Math(f) => f.execute(args),
            BuiltinFunction::String(f) => f.execute(args),
            BuiltinFunction::Regex(f) => f.execute(args),
            BuiltinFunction::Aggregate(_) => Err(ExpressionError::new(
                ExpressionErrorType::InvalidOperation,
                "Aggregation functions need to be executed within the aggregation context"
                    .to_string(),
            )),
            BuiltinFunction::Conversion(f) => f.execute(args),
            BuiltinFunction::DateTime(f) => f.execute(args),
            BuiltinFunction::Geography(f) => f.execute(args),
            BuiltinFunction::Utility(f) => f.execute(args),
            BuiltinFunction::Graph(f) => f.execute(args),
            BuiltinFunction::Container(f) => f.execute(args),
            BuiltinFunction::Path(f) => f.execute(args),
            BuiltinFunction::Fulltext(_f) => {
                // Fulltext functions require execution context
                // This is a placeholder - actual execution happens in the executor
                Err(ExpressionError::new(
                    ExpressionErrorType::InvalidOperation,
                    "The full-text search function needs to be executed within the context of full-text search".to_string(),
                ))
            }
            BuiltinFunction::Vector(f) => f.execute(args),
        }
    }

    /// Execution function (with cache)
    ///
    /// The caching function has been removed; this method directly calls `execute`.
    pub fn execute_with_cache(
        &self,
        args: &[Value],
        _cache: &mut (),
    ) -> Result<Value, ExpressionError> {
        self.execute(args)
    }
}

impl ExpressionFunction for BuiltinFunction {
    fn name(&self) -> &str {
        self.name()
    }

    fn arity(&self) -> usize {
        self.arity()
    }

    fn is_variadic(&self) -> bool {
        self.is_variadic()
    }

    fn execute(&self, args: &[Value]) -> Result<Value, ExpressionError> {
        self.execute(args)
    }

    fn description(&self) -> &str {
        self.description()
    }
}

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
    pub argv: Vec<crate::core::types::c_api::graphdb_value_t>,
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
    extern "C" fn(*mut CFunctionContext, i32, *mut crate::core::types::c_api::graphdb_value_t);

/// Aggregation step callback type
pub type AggregateStepCallback =
    extern "C" fn(*mut CFunctionContext, i32, *mut crate::core::types::c_api::graphdb_value_t);

/// Aggregate final callback type
pub type AggregateFinalCallback = extern "C" fn(*mut CFunctionContext);

/// Implementation of custom functions and their types
#[derive(Clone, Copy)]
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
}

impl std::fmt::Debug for CustomFunctionImpl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CustomFunctionImpl::Rust(_) => write!(f, "Rust closure"),
            CustomFunctionImpl::C { .. } => write!(f, "C scalar callback"),
            CustomFunctionImpl::Aggregate { .. } => write!(f, "C aggregate callback"),
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

                // Please provide the text you would like to have translated.
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
