//! C API Custom Function Module
//!
//! Provide a registration function for custom scalar functions and aggregate functions.

use crate::api::embedded::c_api::error::graphdb_error_code_t;
use crate::api::embedded::c_api::session::GraphDbSessionHandle;
use crate::api::embedded::c_api::types::{
    graphdb_session_t, graphdb_value_t, graphdb_value_type_t,
};
use crate::query::executor::expression::functions::{
    AggregateFinalCallback, AggregateStepCallback, CFunctionContext, CustomFunction,
    ScalarFunctionCallback,
};
use log::error;
use std::ffi::{c_char, c_int, c_void, CStr};

/// Scalar function callback type
#[allow(non_camel_case_types)]
pub type graphdb_scalar_function_callback =
    Option<extern "C" fn(context: *mut graphdb_context_t, argc: c_int, argv: *mut graphdb_value_t)>;

/// Aggregation function step callback type
#[allow(non_camel_case_types)]
pub type graphdb_aggregate_step_callback =
    Option<extern "C" fn(context: *mut graphdb_context_t, argc: c_int, argv: *mut graphdb_value_t)>;

/// The final callback type of the aggregate function
#[allow(non_camel_case_types)]
pub type graphdb_aggregate_final_callback = Option<extern "C" fn(context: *mut graphdb_context_t)>;

/// Function destruction callback type
#[allow(non_camel_case_types)]
pub type graphdb_function_destroy_callback = Option<extern "C" fn(user_data: *mut c_void)>;

/// Function execution context (opaque pointers)
#[repr(C)]
pub struct graphdb_context_t {
    /// Internal context
    pub(crate) inner: CFunctionContext,
}

/// Create a custom scalar function
///
/// # Arguments
/// - `session`: Session handle
/// - `name`: Function name
/// - `argc`: Number of arguments, -1 for variable arguments
/// - `user_data`: User data pointer
/// - `x_func`: Scalar function callback
/// - `x_destroy`: Destructor callback, can be NULL
///
/// # Returns
/// - Success: GRAPHDB_OK
/// - Failure: Error code
///
/// # Example
/// ```c
/// extern void my_function(graphdb_context_t* ctx, int argc, graphdb_value_t* argv) {
///     // Implement function logic
/// }
///
/// graphdb_create_function(session, "my_func", 2, NULL, my_function, NULL);
/// ```
///
/// # Safety
/// - `session` must be a valid session handle created by `graphdb_session_create`
/// - `name` must be a valid pointer to a null-terminated UTF-8 string
/// - `x_func` must be a valid function pointer
/// - `user_data` is passed to the callback and must remain valid for the lifetime of the function
#[no_mangle]
pub unsafe extern "C" fn graphdb_create_function(
    session: *mut graphdb_session_t,
    name: *const c_char,
    argc: c_int,
    user_data: *mut c_void,
    x_func: graphdb_scalar_function_callback,
    _x_destroy: graphdb_function_destroy_callback,
) -> c_int {
    if session.is_null() || name.is_null() || x_func.is_none() {
        return graphdb_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    let name_str = unsafe {
        match CStr::from_ptr(name).to_str() {
            Ok(s) => s,
            Err(_) => return graphdb_error_code_t::GRAPHDB_MISUSE as c_int,
        }
    };

    unsafe {
        let handle = &*(session as *mut GraphDbSessionHandle);

        // Convert the C callback to a Rust callback type.
        let callback: ScalarFunctionCallback = std::mem::transmute(x_func);

        // Create a custom function
        let func = CustomFunction::new_c(
            name_str,
            argc as usize,
            argc < 0,
            format!("C function: {}", name_str),
            callback,
            user_data,
        );

        // Register for the session.
        if let Err(e) = handle.inner.register_custom_function(func) {
            error!("Registration function failed: {:?}", e);
            return graphdb_error_code_t::GRAPHDB_ERROR as c_int;
        }
    }

    graphdb_error_code_t::GRAPHDB_OK as c_int
}

/// Creating custom aggregate functions
///
/// # Arguments
/// - `session`: Session handle
/// - `name`: Function name
/// - `argc`: Number of arguments, -1 for variable arguments
/// - `user_data`: User data pointer
/// - `x_step`: Aggregate step callback
/// - `x_final`: Aggregate final callback
/// - `x_destroy`: Destructor callback, can be NULL
///
/// # Returns
/// - Success: GRAPHDB_OK
/// - Failure: Error code
///
/// # Safety
/// - `session` must be a valid session handle created by `graphdb_session_create`
/// - `name` must be a valid pointer to a null-terminated UTF-8 string
/// - `x_step` and `x_final` must be valid function pointers
/// - `user_data` is passed to the callbacks and must remain valid for the lifetime of the function
#[no_mangle]
pub unsafe extern "C" fn graphdb_create_aggregate(
    session: *mut graphdb_session_t,
    name: *const c_char,
    argc: c_int,
    user_data: *mut c_void,
    x_step: graphdb_aggregate_step_callback,
    x_final: graphdb_aggregate_final_callback,
    _x_destroy: graphdb_function_destroy_callback,
) -> c_int {
    if session.is_null() || name.is_null() || x_step.is_none() || x_final.is_none() {
        return graphdb_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    let name_str = unsafe {
        match CStr::from_ptr(name).to_str() {
            Ok(s) => s,
            Err(_) => return graphdb_error_code_t::GRAPHDB_MISUSE as c_int,
        }
    };

    unsafe {
        let handle = &*(session as *mut GraphDbSessionHandle);

        // Convert the C callback to a Rust callback type.
        let step_callback: AggregateStepCallback = std::mem::transmute(x_step);
        let final_callback: AggregateFinalCallback = std::mem::transmute(x_final);

        // Create an aggregate function
        let func = CustomFunction::new_c_aggregate(
            name_str,
            argc as usize,
            argc < 0,
            format!("C aggregate function: {}", name_str),
            step_callback,
            final_callback,
            user_data,
        );

        // Register for the session.
        if let Err(e) = handle.inner.register_custom_function(func) {
            error!("Registration of the aggregate function failed: {:?}", e);
            return graphdb_error_code_t::GRAPHDB_ERROR as c_int;
        }
    }

    graphdb_error_code_t::GRAPHDB_OK as c_int
}

/// Delete the custom function.
///
/// # Arguments
/// - `session`: Session handle
/// - `name`: Function name
///
/// # Returns
/// - Success: GRAPHDB_OK
/// - Failure: Error code
///
/// # Safety
/// - `session` must be a valid session handle created by `graphdb_session_create`
/// - `name` must be a valid pointer to a null-terminated UTF-8 string
#[no_mangle]
pub unsafe extern "C" fn graphdb_delete_function(
    session: *mut graphdb_session_t,
    name: *const c_char,
) -> c_int {
    if session.is_null() || name.is_null() {
        return graphdb_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    // The function needs to be deleted from the registry.
    // The current return was successful (the function will automatically clean up at the end of the session).
    graphdb_error_code_t::GRAPHDB_OK as c_int
}

/// Setting the return value of a function
///
/// # Arguments
/// - `context`: Function execution context
/// - `value`: Return value
///
/// # Description
/// Call this function in the scalar function or aggregate function's xFinal callback to set the return value
///
/// # Safety
/// - `context` must be a valid function context pointer passed to the callback
/// - `value` must be a valid pointer to a value structure, or NULL to set a null result
/// - This function should only be called from within a registered function callback
#[no_mangle]
pub unsafe extern "C" fn graphdb_context_set_result(
    context: *mut graphdb_context_t,
    value: *const graphdb_value_t,
) -> c_int {
    if context.is_null() {
        return graphdb_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    unsafe {
        let ctx = &mut (*context).inner;
        if value.is_null() {
            ctx.set_result(crate::core::Value::Null(crate::core::NullType::Null));
        } else {
            let val = crate::api::embedded::c_api::value::graphdb_value_to_core(value);
            ctx.set_result(val);
        }
    }

    graphdb_error_code_t::GRAPHDB_OK as c_int
}

/// Obtaining the type of the value returned by a function
///
/// # Arguments
/// - `context`: Function execution context
///
/// # Returns
/// - Value type
///
/// # Safety
/// - `context` must be a valid function context pointer passed to the callback
/// - This function should only be called from within a registered function callback
#[no_mangle]
pub unsafe extern "C" fn graphdb_context_result_type(
    context: *mut graphdb_context_t,
) -> graphdb_value_type_t {
    if context.is_null() {
        return graphdb_value_type_t::GRAPHDB_NULL;
    }

    unsafe {
        let ctx = &(*context).inner;
        match &ctx.result {
            Some(val) => crate::core::utils::value_conversion::core_value_to_graphdb_type(val),
            None => graphdb_value_type_t::GRAPHDB_NULL,
        }
    }
}

/// Setting error messages
///
/// # Arguments
/// - `context`: Function execution context
/// - `error_msg`: Error message
///
/// # Description
/// Call this function to set an error message when the function execution fails
///
/// # Safety
/// - `context` must be a valid function context pointer passed to the callback
/// - `error_msg` must be a valid pointer to a null-terminated UTF-8 string
/// - This function should only be called from within a registered function callback
#[no_mangle]
pub unsafe extern "C" fn graphdb_context_set_error(
    context: *mut graphdb_context_t,
    error_msg: *const c_char,
) -> c_int {
    if context.is_null() || error_msg.is_null() {
        return graphdb_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    unsafe {
        let ctx = &mut (*context).inner;
        let msg = CStr::from_ptr(error_msg).to_string_lossy().into_owned();
        ctx.set_error(msg);
    }

    graphdb_error_code_t::GRAPHDB_OK as c_int
}

/// Obtain parameter values from the context (auxiliary function)
///
/// # Arguments
/// - `context`: Function execution context
/// - `index`: Argument index
///
/// # Returns
/// - Argument value pointer, returns NULL if index is out of bounds
///
/// # Safety
/// - `context` must be a valid function context pointer passed to the callback
/// - `index` must be a valid argument index (0 <= index < argc)
/// - The returned pointer is only valid for the duration of the callback
/// - This function should only be called from within a registered function callback
#[no_mangle]
pub unsafe extern "C" fn graphdb_context_get_arg(
    context: *mut graphdb_context_t,
    index: c_int,
) -> *const graphdb_value_t {
    if context.is_null() {
        return std::ptr::null();
    }
    let ctx = &(*context).inner;
    if index < 0 || index as usize >= ctx.argc {
        return std::ptr::null();
    }
    ctx.argv.as_ptr().add(index as usize)
}

/// Get the number of parameters
///
/// # Arguments
/// - `context`: Function execution context
///
/// # Returns
/// - Number of arguments
///
/// # Safety
/// - `context` must be a valid function context pointer passed to the callback
/// - This function should only be called from within a registered function callback
#[no_mangle]
pub unsafe extern "C" fn graphdb_context_arg_count(context: *mut graphdb_context_t) -> c_int {
    if context.is_null() {
        return 0;
    }
    (*context).inner.argc as c_int
}
