//! C API Custom Function Module
//!
//! Provide a registration function for custom scalar functions and aggregate functions.

use crate::embedded::c_api::error::{linkrs_error_code_t, set_last_error_message};
use crate::embedded::c_api::session::LinkrsSessionHandle;
use crate::embedded::c_api::types::{linkrs_session_t, linkrs_value_t, linkrs_value_type_t};
use linkrs_query::executor::expression::functions::{
    AggregateFinalCallback, AggregateStepCallback, CFunctionContext, CustomFunction,
    ScalarFunctionCallback,
};
use log::error;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::ptr;

/// Scalar function callback type
#[allow(non_camel_case_types)]
pub type linkrs_scalar_function_callback =
    Option<extern "C" fn(context: *mut linkrs_context_t, argc: c_int, argv: *mut linkrs_value_t)>;

/// Aggregation function step callback type
#[allow(non_camel_case_types)]
pub type linkrs_aggregate_step_callback =
    Option<extern "C" fn(context: *mut linkrs_context_t, argc: c_int, argv: *mut linkrs_value_t)>;

/// The final callback type of the aggregate function
#[allow(non_camel_case_types)]
pub type linkrs_aggregate_final_callback = Option<extern "C" fn(context: *mut linkrs_context_t)>;

/// Function destruction callback type
#[allow(non_camel_case_types)]
pub type linkrs_function_destroy_callback = Option<extern "C" fn(user_data: *mut c_void)>;

/// Function execution context (opaque pointer).
///
/// The context is passed to registered callbacks as a borrowed pointer to the
/// query engine's `CFunctionContext`; all access goes through the
/// `linkrs_context_*` functions below, so the layout stays private to C.
#[repr(C)]
pub struct linkrs_context_t {
    _dummy: u8,
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
/// - Success: LINKRS_OK
/// - Failure: Error code
///
/// # Example
/// ```c
/// extern void my_function(linkrs_context_t* ctx, int argc, linkrs_value_t* argv) {
///     // Implement function logic
/// }
///
/// linkrs_create_function(session, "my_func", 2, NULL, my_function, NULL);
/// ```
///
/// # Safety
/// - `session` must be a valid session handle created by `linkrs_session_create`
/// - `name` must be a valid pointer to a null-terminated UTF-8 string
/// - `x_func` must be a valid function pointer
/// - `user_data` is passed to the callback and must remain valid for the lifetime of the function
#[no_mangle]
pub unsafe extern "C" fn linkrs_create_function(
    session: *mut linkrs_session_t,
    name: *const c_char,
    argc: c_int,
    user_data: *mut c_void,
    x_func: linkrs_scalar_function_callback,
    _x_destroy: linkrs_function_destroy_callback,
) -> c_int {
    if session.is_null() || name.is_null() || x_func.is_none() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    let name_str = unsafe {
        match CStr::from_ptr(name).to_str() {
            Ok(s) => s,
            Err(_) => return linkrs_error_code_t::LINKRS_MISUSE as c_int,
        }
    };

    unsafe {
        let handle = &*(session as *mut LinkrsSessionHandle);

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
            return linkrs_error_code_t::LINKRS_ERROR as c_int;
        }
    }

    linkrs_error_code_t::LINKRS_OK as c_int
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
/// - Success: LINKRS_OK
/// - Failure: Error code
///
/// # Safety
/// - `session` must be a valid session handle created by `linkrs_session_create`
/// - `name` must be a valid pointer to a null-terminated UTF-8 string
/// - `x_step` and `x_final` must be valid function pointers
/// - `user_data` is passed to the callbacks and must remain valid for the lifetime of the function
#[no_mangle]
pub unsafe extern "C" fn linkrs_create_aggregate(
    session: *mut linkrs_session_t,
    name: *const c_char,
    argc: c_int,
    user_data: *mut c_void,
    x_step: linkrs_aggregate_step_callback,
    x_final: linkrs_aggregate_final_callback,
    _x_destroy: linkrs_function_destroy_callback,
) -> c_int {
    if session.is_null() || name.is_null() || x_step.is_none() || x_final.is_none() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    let name_str = unsafe {
        match CStr::from_ptr(name).to_str() {
            Ok(s) => s,
            Err(_) => return linkrs_error_code_t::LINKRS_MISUSE as c_int,
        }
    };

    unsafe {
        let handle = &*(session as *mut LinkrsSessionHandle);

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
            return linkrs_error_code_t::LINKRS_ERROR as c_int;
        }
    }

    linkrs_error_code_t::LINKRS_OK as c_int
}

/// Delete the custom function.
///
/// # Arguments
/// - `session`: Session handle
/// - `name`: Function name
///
/// # Returns
/// - Success: LINKRS_OK
/// - Failure: Error code
///
/// # Safety
/// - `session` must be a valid session handle created by `linkrs_session_create`
/// - `name` must be a valid pointer to a null-terminated UTF-8 string
#[no_mangle]
pub unsafe extern "C" fn linkrs_delete_function(
    session: *mut linkrs_session_t,
    name: *const c_char,
) -> c_int {
    if session.is_null() || name.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    // The function needs to be deleted from the registry.
    // The current return was successful (the function will automatically clean up at the end of the session).
    linkrs_error_code_t::LINKRS_OK as c_int
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
pub unsafe extern "C" fn linkrs_context_set_result(
    context: *mut linkrs_context_t,
    value: *const linkrs_value_t,
) -> c_int {
    if context.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    unsafe {
        let ctx = &mut *(context as *mut CFunctionContext);
        if value.is_null() {
            ctx.set_result(linkrs_core::Value::Null(linkrs_core::NullType::Null));
        } else {
            let val = crate::embedded::c_api::value::linkrs_value_to_core(value);
            ctx.set_result(val);
        }
    }

    linkrs_error_code_t::LINKRS_OK as c_int
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
pub unsafe extern "C" fn linkrs_context_result_type(
    context: *mut linkrs_context_t,
) -> linkrs_value_type_t {
    if context.is_null() {
        return linkrs_value_type_t::LINKRS_NULL;
    }

    unsafe {
        let ctx = &*(context as *const CFunctionContext);
        match &ctx.result {
            Some(val) => linkrs_core::value_conversion::core_value_to_linkrs_type(val),
            None => linkrs_value_type_t::LINKRS_NULL,
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
pub unsafe extern "C" fn linkrs_context_set_error(
    context: *mut linkrs_context_t,
    error_msg: *const c_char,
) -> c_int {
    if context.is_null() || error_msg.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    unsafe {
        let ctx = &mut *(context as *mut CFunctionContext);
        let msg = CStr::from_ptr(error_msg).to_string_lossy().into_owned();
        ctx.set_error(msg);
    }

    linkrs_error_code_t::LINKRS_OK as c_int
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
pub unsafe extern "C" fn linkrs_context_get_arg(
    context: *mut linkrs_context_t,
    index: c_int,
) -> *const linkrs_value_t {
    if context.is_null() {
        return std::ptr::null();
    }
    let ctx = &*(context as *const CFunctionContext);
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
pub unsafe extern "C" fn linkrs_context_arg_count(context: *mut linkrs_context_t) -> c_int {
    if context.is_null() {
        return 0;
    }
    (*(context as *const CFunctionContext)).argc as c_int
}

/// Load a UDF dynamic library and register the exported function.
///
/// # Arguments
/// - `session`: Session handle
/// - `path`: Null-terminated UTF-8 path to the `.so` / `.dylib` / `.dll` file
///
/// # Returns
/// - Success: LINKRS_OK
/// - Failure: Error code
///
/// # Safety
/// - `session` must be a valid session handle created by `linkrs_session_create`
/// - `path` must be a valid pointer to a null-terminated UTF-8 string
#[no_mangle]
pub unsafe extern "C" fn linkrs_load_extension(
    session: *mut linkrs_session_t,
    path: *const c_char,
) -> c_int {
    if session.is_null() || path.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }
    let path_str = unsafe {
        match CStr::from_ptr(path).to_str() {
            Ok(s) => s,
            Err(_) => return linkrs_error_code_t::LINKRS_MISUSE as c_int,
        }
    };
    unsafe {
        let handle = &*(session as *mut LinkrsSessionHandle);
        if let Err(e) = handle.inner.load_extension(std::path::Path::new(path_str)) {
            error!("Load extension failed: {:?}", e);
            return linkrs_error_code_t::LINKRS_ERROR as c_int;
        }
    }
    linkrs_error_code_t::LINKRS_OK as c_int
}

/// Unload a previously loaded dynamic UDF by function name.
///
/// # Arguments
/// - `session`: Session handle
/// - `name`: Null-terminated UTF-8 function name
///
/// # Returns
/// - Success: LINKRS_OK
/// - Failure: Error code
///
/// # Safety
/// - `session` must be a valid session handle created by `linkrs_session_create`
/// - `name` must be a valid pointer to a null-terminated UTF-8 string
#[no_mangle]
pub unsafe extern "C" fn linkrs_unload_extension(
    session: *mut linkrs_session_t,
    name: *const c_char,
) -> c_int {
    if session.is_null() || name.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }
    let name_str = unsafe {
        match CStr::from_ptr(name).to_str() {
            Ok(s) => s,
            Err(_) => return linkrs_error_code_t::LINKRS_MISUSE as c_int,
        }
    };
    unsafe {
        let handle = &*(session as *mut LinkrsSessionHandle);
        if let Err(e) = handle.inner.unload_extension(name_str) {
            error!("Unload extension failed: {:?}", e);
            return linkrs_error_code_t::LINKRS_ERROR as c_int;
        }
    }
    linkrs_error_code_t::LINKRS_OK as c_int
}

/// Reload a dynamic UDF from its original library path.
///
/// # Arguments
/// - `session`: Session handle
/// - `name`: Null-terminated UTF-8 function name
/// - `reloaded`: Out-parameter set to 1 when the library was reloaded,
///   0 when the file is unchanged and reloading was skipped
///
/// # Returns
/// - Success: LINKRS_OK
/// - Failure: Error code
///
/// # Safety
/// - `session` must be a valid session handle created by `linkrs_session_create`
/// - `name` must be a valid pointer to a null-terminated UTF-8 string
/// - `reloaded` must be a valid writable pointer (may be NULL to ignore)
#[no_mangle]
pub unsafe extern "C" fn linkrs_reload_extension(
    session: *mut linkrs_session_t,
    name: *const c_char,
    reloaded: *mut c_int,
) -> c_int {
    if session.is_null() || name.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }
    let name_str = unsafe {
        match CStr::from_ptr(name).to_str() {
            Ok(s) => s,
            Err(_) => return linkrs_error_code_t::LINKRS_MISUSE as c_int,
        }
    };
    unsafe {
        let handle = &*(session as *mut LinkrsSessionHandle);
        match handle.inner.reload_extension(name_str) {
            Ok(did_reload) => {
                if !reloaded.is_null() {
                    *reloaded = i32::from(did_reload);
                }
            }
            Err(e) => {
                error!("Reload extension failed: {:?}", e);
                return linkrs_error_code_t::LINKRS_ERROR as c_int;
            }
        }
    }
    linkrs_error_code_t::LINKRS_OK as c_int
}

/// List all loaded dynamic UDF extensions as JSON.
///
/// Projection of `Session::list_extensions`: returns a heap JSON array
/// ordered by name; each entry carries `name`, `path`, `description`,
/// `min_arity`, `max_arity` and `is_pure`.
///
/// # Arguments
/// - `session`: Session handle
/// - `out_json`: Output parameter, heap JSON array string
///
/// # Returns
/// - Success: LINKRS_OK (`*out_json` holds the JSON array)
/// - Failure: Error code (`*out_json` is NULL)
///
/// # Memory Management
/// The returned string is dynamically allocated and must be freed by the
/// caller using `linkrs_free_string` to avoid memory leaks.
///
/// # Safety
/// - `session` must be a valid session handle created by `linkrs_session_create`
/// - `out_json` must be a valid pointer to store the result
/// - The returned pointer must be freed by the caller to avoid memory leaks
#[no_mangle]
pub unsafe extern "C" fn linkrs_list_extensions(
    session: *mut linkrs_session_t,
    out_json: *mut *mut c_char,
) -> c_int {
    if session.is_null() || out_json.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }
    *out_json = ptr::null_mut();

    let handle = &*(session as *mut LinkrsSessionHandle);
    let extensions: Vec<serde_json::Value> = handle
        .inner
        .list_extensions()
        .into_iter()
        .map(|info| {
            serde_json::json!({
                "name": info.name,
                "path": info.path,
                "description": info.description,
                "min_arity": info.min_arity,
                "max_arity": info.max_arity,
                "is_pure": info.is_pure,
            })
        })
        .collect();
    match serde_json::to_string(&extensions) {
        Ok(json) => match CString::new(json) {
            Ok(c_json) => {
                *out_json = c_json.into_raw();
                linkrs_error_code_t::LINKRS_OK as c_int
            }
            Err(_) => linkrs_error_code_t::LINKRS_ERROR as c_int,
        },
        Err(e) => {
            set_last_error_message(format!("failed to serialize extension list: {e}"));
            linkrs_error_code_t::LINKRS_ERROR as c_int
        }
    }
}

/// Install a UDF extension from a source string.
///
/// Projection of `Session::install_extension`: local file paths load
/// directly; remote sources follow the embedded semantics (unsupported
/// sources fail with LINKRS_ERROR). On success `*out_name` holds the heap
/// registered extension name.
///
/// # Arguments
/// - `session`: Session handle
/// - `source`: Extension source (UTF-8 encoded)
/// - `out_name`: Output parameter, heap registered name string
///
/// # Returns
/// - Success: LINKRS_OK (`*out_name` holds the name)
/// - Failure: Error code (`*out_name` is NULL)
///
/// # Memory Management
/// The returned string is dynamically allocated and must be freed by the
/// caller using `linkrs_free_string` to avoid memory leaks.
///
/// # Safety
/// - `session` must be a valid session handle created by `linkrs_session_create`
/// - `source` must be a valid pointer to a null-terminated UTF-8 string
/// - `out_name` must be a valid pointer to store the result
/// - The returned pointer must be freed by the caller to avoid memory leaks
#[no_mangle]
pub unsafe extern "C" fn linkrs_install_extension(
    session: *mut linkrs_session_t,
    source: *const c_char,
    out_name: *mut *mut c_char,
) -> c_int {
    if session.is_null() || source.is_null() || out_name.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }
    *out_name = ptr::null_mut();

    let source_str = match CStr::from_ptr(source).to_str() {
        Ok(s) => s,
        Err(_) => return linkrs_error_code_t::LINKRS_MISUSE as c_int,
    };
    let handle = &*(session as *mut LinkrsSessionHandle);
    match handle.inner.install_extension(source_str) {
        Ok(name) => match CString::new(name) {
            Ok(c_name) => {
                *out_name = c_name.into_raw();
                linkrs_error_code_t::LINKRS_OK as c_int
            }
            Err(_) => linkrs_error_code_t::LINKRS_ERROR as c_int,
        },
        Err(e) => {
            error!("Install extension failed: {:?}", e);
            set_last_error_message(format!("{e}"));
            linkrs_error_code_t::LINKRS_ERROR as c_int
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedded::c_api::config::linkrs_config_memory;
    use crate::embedded::c_api::database::{
        linkrs_close, linkrs_free_string, linkrs_open_with_config,
    };
    use crate::embedded::c_api::session::{linkrs_session_close, linkrs_session_create};
    use crate::embedded::c_api::types::{linkrs_session_t, linkrs_t};
    use std::ffi::CString;

    fn create_test_session() -> (*mut linkrs_t, *mut linkrs_session_t) {
        unsafe {
            let config = linkrs_config_memory();
            let mut db: *mut linkrs_t = ptr::null_mut();
            assert_eq!(
                linkrs_open_with_config(config, &mut db),
                linkrs_error_code_t::LINKRS_OK as c_int
            );
            let mut session: *mut linkrs_session_t = ptr::null_mut();
            assert_eq!(
                linkrs_session_create(db, &mut session),
                linkrs_error_code_t::LINKRS_OK as c_int
            );
            (db, session)
        }
    }

    #[test]
    fn test_list_extensions_empty_and_null() {
        unsafe {
            let (db, session) = create_test_session();

            let mut out: *mut c_char = ptr::null_mut();
            assert_eq!(
                linkrs_list_extensions(session, &mut out),
                linkrs_error_code_t::LINKRS_OK as c_int
            );
            assert!(!out.is_null());
            let text = CStr::from_ptr(out).to_str().expect("utf8").to_string();
            linkrs_free_string(out);
            assert_eq!(text, "[]");

            assert_eq!(
                linkrs_list_extensions(ptr::null_mut(), &mut out),
                linkrs_error_code_t::LINKRS_MISUSE as c_int
            );
            assert_eq!(
                linkrs_list_extensions(session, ptr::null_mut()),
                linkrs_error_code_t::LINKRS_MISUSE as c_int
            );

            // A missing library file deterministically fails the install.
            let missing = CString::new("/definitely/not/here.so").unwrap();
            let mut name: *mut c_char = ptr::null_mut();
            assert_eq!(
                linkrs_install_extension(session, missing.as_ptr(), &mut name),
                linkrs_error_code_t::LINKRS_ERROR as c_int
            );
            assert!(name.is_null());
            assert_eq!(
                linkrs_install_extension(ptr::null_mut(), missing.as_ptr(), &mut name),
                linkrs_error_code_t::LINKRS_MISUSE as c_int
            );

            linkrs_session_close(session);
            linkrs_close(db);
        }
    }
}
