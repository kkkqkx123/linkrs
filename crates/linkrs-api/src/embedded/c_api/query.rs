//! C API Query Execution Module
//!
//! Provides the functionality to execute queries, including both simple queries and parameterized queries.

use crate::embedded::c_api::error::{
    error_code_from_core_error, extended_error_code_from_core_error, linkrs_error_code_t,
};
use crate::embedded::c_api::result::LinkrsResultHandle;
use crate::embedded::c_api::session::LinkrsSessionHandle;
use crate::embedded::c_api::types::{linkrs_result_t, linkrs_session_t, linkrs_value_t};
use linkrs_core::Value;
use std::collections::HashMap;
use std::ffi::{c_char, c_int, CStr};
use std::ptr;

/// Perform a simple query
///
/// # Arguments
/// - `session`: Session handle
/// - `query`: Query statement (UTF-8 encoded)
/// - `result`: Output parameter, result set handle
///
/// # Returns
/// - Success: LINKRS_OK
/// - Failure: Error code
///
/// # Safety
/// - `session` must be a valid session handle created by `linkrs_session_create`
/// - `query` must be a valid pointer to a null-terminated UTF-8 string
/// - `result` must be a valid pointer to store the result handle
/// - The caller is responsible for freeing the result handle using `linkrs_result_free` when done
#[no_mangle]
pub unsafe extern "C" fn linkrs_execute(
    session: *mut linkrs_session_t,
    query: *const c_char,
    result: *mut *mut linkrs_result_t,
) -> c_int {
    if session.is_null() || query.is_null() || result.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    let query_str = unsafe {
        match CStr::from_ptr(query).to_str() {
            Ok(s) => s,
            Err(_) => return linkrs_error_code_t::LINKRS_MISUSE as c_int,
        }
    };

    unsafe {
        let handle = &mut *(session as *mut LinkrsSessionHandle);

        // Calling the SQL tracing callback
        handle.trace(query_str);

        match handle.inner.execute(query_str) {
            Ok(query_result) => {
                handle.clear_error();

                // Statement-level DML notification (bus) + legacy update hook.
                // Classification does a full-string uppercase scan for
                // MATCH-family queries, so it is skipped entirely when
                // neither side listens.
                if handle.has_update_hook() || handle.inner.has_dml_observers() {
                    let rows = query_result.metadata().rows_returned as u64;
                    if let Some(op) = handle.inner.notify_dml(query_str, rows) {
                        if handle.has_update_hook() {
                            let operation = match op {
                                linkrs_query::DmlOp::Insert => 1,
                                linkrs_query::DmlOp::Update => 2,
                                linkrs_query::DmlOp::Delete => 3,
                            };
                            let space_name_owned = handle.inner.current_space();
                            let space_name = space_name_owned.as_deref().unwrap_or("default");
                            handle.invoke_update_hook(operation, space_name, rows as i64);
                        }
                    }
                }

                let result_handle = Box::new(LinkrsResultHandle {
                    inner: query_result,
                });
                *result = Box::into_raw(result_handle) as *mut linkrs_result_t;
                linkrs_error_code_t::LINKRS_OK as c_int
            }
            Err(e) => {
                let (error_code, _) = error_code_from_core_error(&e);
                let error_msg = format!("{}", e);
                let offset = e.error_offset();
                let extended_code = Some(extended_error_code_from_core_error(&e));
                handle.set_error(error_msg, offset, extended_code);
                *result = ptr::null_mut();
                error_code
            }
        }
    }
}

/// Execute a parameterized query
///
/// Positional binding: `params[i]` binds to the `@param_{i}` query
/// parameter (e.g. `params[0]` fills `@param_0`). This mirrors the Rust
/// `Session::execute_with_params` named-parameter map with synthesized
/// `param_{i}` keys.
///
/// # Arguments
/// - `session`: Session handle
/// - `query`: Query statement (UTF-8 encoded)
/// - `params`: Parameter array
/// - `param_count`: Number of parameters
/// - `result`: Output parameter, result set handle
///
/// # Returns
/// - Success: LINKRS_OK
/// - Failure: Error code
///
/// # Safety
/// - `session` must be a valid session handle created by `linkrs_session_create`
/// - `query` must be a valid pointer to a null-terminated UTF-8 string
/// - `result` must be a valid pointer to store the result handle
/// - If `params` is not NULL, it must point to at least `param_count` valid `linkrs_value_t` elements
/// - The caller is responsible for freeing the result handle using `linkrs_result_free` when done
#[no_mangle]
pub unsafe extern "C" fn linkrs_execute_params(
    session: *mut linkrs_session_t,
    query: *const c_char,
    params: *const linkrs_value_t,
    param_count: usize,
    result: *mut *mut linkrs_result_t,
) -> c_int {
    if session.is_null() || query.is_null() || result.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    let query_str = unsafe {
        match CStr::from_ptr(query).to_str() {
            Ok(s) => s,
            Err(_) => return linkrs_error_code_t::LINKRS_MISUSE as c_int,
        }
    };

    let mut params_map = HashMap::new();

    if !params.is_null() && param_count > 0 {
        for i in 0..param_count {
            unsafe {
                let param = &*params.add(i);
                let param_name = format!("param_{}", i);
                let value = convert_c_value_to_rust(param);
                params_map.insert(param_name, value);
            }
        }
    }

    unsafe {
        let handle = &mut *(session as *mut LinkrsSessionHandle);

        // Calling the SQL tracing callback (matches `linkrs_execute`).
        handle.trace(query_str);

        match handle.inner.execute_with_params(query_str, params_map) {
            Ok(query_result) => {
                handle.clear_error();

                // Statement-level DML notification (bus) + legacy update hook.
                // Classification does a full-string uppercase scan for
                // MATCH-family queries, so it is skipped entirely when
                // neither side listens.
                if handle.has_update_hook() || handle.inner.has_dml_observers() {
                    let rows = query_result.metadata().rows_returned as u64;
                    if let Some(op) = handle.inner.notify_dml(query_str, rows) {
                        if handle.has_update_hook() {
                            let operation = match op {
                                linkrs_query::DmlOp::Insert => 1,
                                linkrs_query::DmlOp::Update => 2,
                                linkrs_query::DmlOp::Delete => 3,
                            };
                            let space_name_owned = handle.inner.current_space();
                            let space_name = space_name_owned.as_deref().unwrap_or("default");
                            handle.invoke_update_hook(operation, space_name, rows as i64);
                        }
                    }
                }

                let result_handle = Box::new(LinkrsResultHandle {
                    inner: query_result,
                });
                *result = Box::into_raw(result_handle) as *mut linkrs_result_t;
                linkrs_error_code_t::LINKRS_OK as c_int
            }
            Err(e) => {
                let (error_code, _) = error_code_from_core_error(&e);
                let error_msg = format!("{}", e);
                let offset = e.error_offset();
                let extended_code = Some(extended_error_code_from_core_error(&e));
                handle.set_error(error_msg, offset, extended_code);
                *result = ptr::null_mut();
                error_code
            }
        }
    }
}

/// Validate a query without executing it (`Session::validate_query` over FFI).
///
/// # Returns
/// - `LINKRS_OK` when the statement parses and binds; otherwise a normal
///   error code with details in the session error slot.
///
/// # Safety
/// - `session` must be a valid session handle
/// - `query` must be a null-terminated UTF-8 string
#[no_mangle]
pub unsafe extern "C" fn linkrs_validate_query(
    session: *mut linkrs_session_t,
    query: *const c_char,
) -> c_int {
    if session.is_null() || query.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }
    let query_str = unsafe {
        match CStr::from_ptr(query).to_str() {
            Ok(s) => s,
            Err(_) => return linkrs_error_code_t::LINKRS_MISUSE as c_int,
        }
    };
    unsafe {
        let handle = &mut *(session as *mut LinkrsSessionHandle);
        match handle.inner.validate_query(query_str) {
            // The conclusion travels inside the success value: an invalid
            // statement must not report success.
            Ok(validation) if validation.valid => {
                handle.clear_error();
                linkrs_error_code_t::LINKRS_OK as c_int
            }
            Ok(validation) => {
                let error_msg = validation
                    .error
                    .unwrap_or_else(|| "query failed validation".to_string());
                let extended = if error_msg.starts_with("semantic error") {
                    crate::api_core::error::ExtendedErrorCode::SemanticError
                } else {
                    crate::api_core::error::ExtendedErrorCode::SyntaxError
                };
                let error = crate::api_core::error::CoreError::detailed_query_error(
                    error_msg, extended, None,
                );
                let (error_code, _) = error_code_from_core_error(&error);
                let extended_code = Some(extended_error_code_from_core_error(&error));
                handle.set_error(format!("{error}"), None, extended_code);
                error_code
            }
            Err(e) => {
                let (error_code, _) = error_code_from_core_error(&e);
                let error_msg = format!("{e}");
                let offset = e.error_offset();
                let extended_code = Some(extended_error_code_from_core_error(&e));
                handle.set_error(error_msg, offset, extended_code);
                error_code
            }
        }
    }
}

/// Explain a query without executing it (`Session::explain_query` over FFI).
///
/// # Safety
/// - `session` must be a valid session handle
/// - `query` must be a null-terminated UTF-8 string
/// - `result` must be a valid out-pointer; free with `linkrs_result_free`
#[no_mangle]
pub unsafe extern "C" fn linkrs_explain_query(
    session: *mut linkrs_session_t,
    query: *const c_char,
    result: *mut *mut linkrs_result_t,
) -> c_int {
    if session.is_null() || query.is_null() || result.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }
    let query_str = unsafe {
        match CStr::from_ptr(query).to_str() {
            Ok(s) => s,
            Err(_) => return linkrs_error_code_t::LINKRS_MISUSE as c_int,
        }
    };
    unsafe {
        let handle = &mut *(session as *mut LinkrsSessionHandle);
        match handle.inner.explain_query(query_str) {
            Ok(query_result) => {
                handle.clear_error();
                let result_handle = Box::new(LinkrsResultHandle {
                    inner: query_result,
                });
                *result = Box::into_raw(result_handle) as *mut linkrs_result_t;
                linkrs_error_code_t::LINKRS_OK as c_int
            }
            Err(e) => {
                let (error_code, _) = error_code_from_core_error(&e);
                let error_msg = format!("{e}");
                let offset = e.error_offset();
                let extended_code = Some(extended_error_code_from_core_error(&e));
                handle.set_error(error_msg, offset, extended_code);
                *result = ptr::null_mut();
                error_code
            }
        }
    }
}

/// Convert a C value to a Rust value.
///
/// Integer inputs map to `BigInt` and float inputs to `Double`: these are
/// the engine's canonical numeric types (integer literals parse as
/// `BigInt`), so narrowing to `Int`/`Float` would break round-trips.
/// Blob inputs copy the referenced bytes. Complex types (list/map/
/// vertex/edge/path) have no C representation and fall back to Null.
///
/// # Safety
///
/// `c_value` must be a valid pointer to a properly initialized `linkrs_value_t` struct.
/// The caller must ensure that string/blob pointers within the value are valid and properly aligned.
pub unsafe fn convert_c_value_to_rust(c_value: &linkrs_value_t) -> Value {
    use crate::embedded::c_api::types::linkrs_value_type_t;

    match c_value.type_ {
        linkrs_value_type_t::LINKRS_NULL => Value::Null(linkrs_core::value::NullType::Null),
        linkrs_value_type_t::LINKRS_BOOL => Value::Bool(c_value.data.boolean),
        linkrs_value_type_t::LINKRS_INT => Value::BigInt(c_value.data.integer),
        linkrs_value_type_t::LINKRS_FLOAT => Value::Double(c_value.data.floating),
        linkrs_value_type_t::LINKRS_STRING => {
            if c_value.data.string.data.is_null() || c_value.data.string.len == 0 {
                Value::string("")
            } else {
                let slice = std::slice::from_raw_parts(
                    c_value.data.string.data as *const u8,
                    c_value.data.string.len,
                );
                let s = String::from_utf8_unchecked(slice.to_vec());
                Value::string(s)
            }
        }
        linkrs_value_type_t::LINKRS_BLOB => {
            if c_value.data.blob.data.is_null() || c_value.data.blob.len == 0 {
                Value::Blob(Vec::new())
            } else {
                let slice =
                    std::slice::from_raw_parts(c_value.data.blob.data, c_value.data.blob.len);
                Value::Blob(slice.to_vec())
            }
        }
        _ => Value::Null(linkrs_core::value::NullType::Null),
    }
}

/// Collect parallel name/value arrays into a parameter map.
///
/// Shared by the named-parameter entry points on sessions, transactions
/// and prepared statements. A zero `count` yields an empty map without
/// touching the arrays; any null name pointer, non-UTF-8 name or empty
/// name fails the whole collection.
///
/// # Safety
/// - When `count` is nonzero, `names` must point to at least `count`
///   valid null-terminated UTF-8 string pointers and `values` to at least
///   `count` valid `linkrs_value_t` elements.
/// - When `count` is zero both pointers may be NULL.
pub(crate) unsafe fn collect_named_params(
    names: *const *const c_char,
    values: *const linkrs_value_t,
    count: usize,
) -> Option<HashMap<String, Value>> {
    let mut map = HashMap::with_capacity(count);
    if count == 0 {
        return Some(map);
    }
    if names.is_null() || values.is_null() {
        return None;
    }
    for i in 0..count {
        let name_ptr = *names.add(i);
        if name_ptr.is_null() {
            return None;
        }
        let name = match CStr::from_ptr(name_ptr).to_str() {
            Ok(s) if !s.is_empty() => s.to_string(),
            _ => return None,
        };
        let value = convert_c_value_to_rust(&*values.add(i));
        map.insert(name, value);
    }
    Some(map)
}

/// Run a session query with named parameters and optional session variables.
///
/// Trace, error-slot and update-hook handling match `linkrs_execute_params`;
/// only the parameter source differs. `variables` selects between
/// `execute_with_params` and `execute_with_params_and_variables`.
unsafe fn run_session_named_execution(
    handle: &mut LinkrsSessionHandle,
    query_str: &str,
    params: HashMap<String, Value>,
    variables: Option<HashMap<String, Value>>,
    out_result: *mut *mut linkrs_result_t,
) -> c_int {
    handle.trace(query_str);

    let outcome = match variables {
        Some(vars) => handle
            .inner
            .execute_with_params_and_variables(query_str, params, vars),
        None => handle.inner.execute_with_params(query_str, params),
    };
    match outcome {
        Ok(query_result) => {
            handle.clear_error();

            if handle.has_update_hook() || handle.inner.has_dml_observers() {
                let rows = query_result.metadata().rows_returned as u64;
                if let Some(op) = handle.inner.notify_dml(query_str, rows) {
                    if handle.has_update_hook() {
                        let operation = match op {
                            linkrs_query::DmlOp::Insert => 1,
                            linkrs_query::DmlOp::Update => 2,
                            linkrs_query::DmlOp::Delete => 3,
                        };
                        let space_name_owned = handle.inner.current_space();
                        let space_name = space_name_owned.as_deref().unwrap_or("default");
                        handle.invoke_update_hook(operation, space_name, rows as i64);
                    }
                }
            }

            let result_handle = Box::new(LinkrsResultHandle {
                inner: query_result,
            });
            *out_result = Box::into_raw(result_handle) as *mut linkrs_result_t;
            linkrs_error_code_t::LINKRS_OK as c_int
        }
        Err(e) => {
            let (error_code, _) = error_code_from_core_error(&e);
            let error_msg = format!("{}", e);
            let offset = e.error_offset();
            let extended_code = Some(extended_error_code_from_core_error(&e));
            handle.set_error(error_msg, offset, extended_code);
            *out_result = ptr::null_mut();
            error_code
        }
    }
}

/// Execute a query with named parameters.
///
/// Unlike the positional `linkrs_execute_params` (`@param_{i}`), each entry
/// binds by name: `names[i]` (without the `@` sigil) fills `@names[i]` in
/// the statement, the same rule `linkrs_stmt_bind` uses.
///
/// # Arguments
/// - `session`: Session handle
/// - `query`: Query statement (UTF-8 encoded)
/// - `names`: Parameter name array (may be NULL when `param_count` is 0)
/// - `values`: Parameter value array (may be NULL when `param_count` is 0)
/// - `param_count`: Number of parameters
/// - `result`: Output parameter, result set handle
///
/// # Returns
/// - Success: LINKRS_OK
/// - Failure: Error code
///
/// # Safety
/// - `session` must be a valid session handle created by `linkrs_session_create`
/// - `query` must be a valid pointer to a null-terminated UTF-8 string
/// - `result` must be a valid pointer to store the result handle
/// - When `param_count` is nonzero, `names` must point to at least
///   `param_count` valid name pointers and `values` to at least
///   `param_count` valid `linkrs_value_t` elements
/// - The caller is responsible for freeing the result handle using `linkrs_result_free` when done
#[no_mangle]
pub unsafe extern "C" fn linkrs_execute_named_params(
    session: *mut linkrs_session_t,
    query: *const c_char,
    names: *const *const c_char,
    values: *const linkrs_value_t,
    param_count: usize,
    result: *mut *mut linkrs_result_t,
) -> c_int {
    if session.is_null() || query.is_null() || result.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    let query_str = match CStr::from_ptr(query).to_str() {
        Ok(s) => s,
        Err(_) => return linkrs_error_code_t::LINKRS_MISUSE as c_int,
    };

    let params = match collect_named_params(names, values, param_count) {
        Some(map) => map,
        None => return linkrs_error_code_t::LINKRS_MISUSE as c_int,
    };

    let handle = &mut *(session as *mut LinkrsSessionHandle);
    run_session_named_execution(handle, query_str, params, None, result)
}

/// Execute a query with named parameters and client-supplied session variables.
///
/// Projection of `Session::execute_with_params_and_variables`: `@name`
/// references resolve from the parameter map, `$name` references from the
/// variable map. The two channels are fully independent, so a parameter
/// and a session variable may share the same name without conflict.
///
/// # Arguments
/// - `session`: Session handle
/// - `query`: Query statement (UTF-8 encoded)
/// - `param_names`: Parameter name array (may be NULL when `param_count` is 0)
/// - `params`: Parameter value array (may be NULL when `param_count` is 0)
/// - `param_count`: Number of parameters
/// - `var_names`: Session variable name array (may be NULL when `var_count` is 0)
/// - `vars`: Session variable value array (may be NULL when `var_count` is 0)
/// - `var_count`: Number of session variables
/// - `result`: Output parameter, result set handle
///
/// # Returns
/// - Success: LINKRS_OK
/// - Failure: Error code
///
/// # Safety
/// - `session` must be a valid session handle created by `linkrs_session_create`
/// - `query` must be a valid pointer to a null-terminated UTF-8 string
/// - `result` must be a valid pointer to store the result handle
/// - Each name array must point to at least its count of valid name
///   pointers (or be NULL when the matching count is 0), and each value
///   array to at least its count of valid `linkrs_value_t` elements
/// - The caller is responsible for freeing the result handle using `linkrs_result_free` when done
#[no_mangle]
pub unsafe extern "C" fn linkrs_execute_with_variables(
    session: *mut linkrs_session_t,
    query: *const c_char,
    param_names: *const *const c_char,
    params: *const linkrs_value_t,
    param_count: usize,
    var_names: *const *const c_char,
    vars: *const linkrs_value_t,
    var_count: usize,
    result: *mut *mut linkrs_result_t,
) -> c_int {
    if session.is_null() || query.is_null() || result.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    let query_str = match CStr::from_ptr(query).to_str() {
        Ok(s) => s,
        Err(_) => return linkrs_error_code_t::LINKRS_MISUSE as c_int,
    };

    let params_map = match collect_named_params(param_names, params, param_count) {
        Some(map) => map,
        None => return linkrs_error_code_t::LINKRS_MISUSE as c_int,
    };
    let vars_map = match collect_named_params(var_names, vars, var_count) {
        Some(map) => map,
        None => return linkrs_error_code_t::LINKRS_MISUSE as c_int,
    };

    let handle = &mut *(session as *mut LinkrsSessionHandle);
    run_session_named_execution(handle, query_str, params_map, Some(vars_map), result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedded::c_api::database::{linkrs_close, linkrs_open};
    use crate::embedded::c_api::result::linkrs_result_free;
    use crate::embedded::c_api::session::{linkrs_session_close, linkrs_session_create};
    use crate::embedded::c_api::types::linkrs_t;
    use std::ffi::CString;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn create_test_db() -> *mut linkrs_t {
        let counter = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let temp_dir = std::env::temp_dir().join("linkrs_c_api_test");
        std::fs::create_dir_all(&temp_dir).ok();
        let db_path = temp_dir.join(format!("test_{}_{}.db", std::process::id(), counter));

        let path_cstring = CString::new(db_path.to_str().expect("Invalid path"))
            .expect("Failed to create CString");
        let mut db: *mut linkrs_t = ptr::null_mut();

        let rc = unsafe { linkrs_open(path_cstring.as_ptr(), &mut db) };
        assert_eq!(rc, linkrs_error_code_t::LINKRS_OK as c_int);
        assert!(!db.is_null());

        db
    }

    #[test]
    fn test_named_params_and_variables() {
        use crate::embedded::c_api::result::{linkrs_get_int_by_index, linkrs_result_free};
        use crate::embedded::c_api::session::{linkrs_session_close, linkrs_session_create};
        use crate::embedded::c_api::types::{
            linkrs_value_data_t, linkrs_value_t, linkrs_value_type_t,
        };

        let db = create_test_db();
        let mut session: *mut linkrs_session_t = ptr::null_mut();
        assert_eq!(
            unsafe { linkrs_session_create(db, &mut session) },
            linkrs_error_code_t::LINKRS_OK as c_int
        );

        unsafe {
            // Named binding: `names[i]` fills `@names[i]`.
            let query = CString::new("RETURN @greeting").unwrap();
            let name = CString::new("greeting").unwrap();
            let names = [name.as_ptr()];
            let value = linkrs_value_t {
                type_: linkrs_value_type_t::LINKRS_INT,
                data: linkrs_value_data_t { integer: 7 },
            };
            let mut result: *mut linkrs_result_t = ptr::null_mut();
            assert_eq!(
                linkrs_execute_named_params(
                    session,
                    query.as_ptr(),
                    names.as_ptr(),
                    &value,
                    1,
                    &mut result
                ),
                linkrs_error_code_t::LINKRS_OK as c_int
            );
            let mut got: i64 = 0;
            assert_eq!(
                linkrs_get_int_by_index(result, 0, 0, &mut got),
                linkrs_error_code_t::LINKRS_OK as c_int
            );
            assert_eq!(got, 7);
            linkrs_result_free(result);

            // Parameters and session variables coexist under one name.
            let query = CString::new("RETURN @x + $v").unwrap();
            let param_name = CString::new("x").unwrap();
            let var_name = CString::new("v").unwrap();
            let param_names = [param_name.as_ptr()];
            let var_names = [var_name.as_ptr()];
            let param = linkrs_value_t {
                type_: linkrs_value_type_t::LINKRS_INT,
                data: linkrs_value_data_t { integer: 40 },
            };
            let var = linkrs_value_t {
                type_: linkrs_value_type_t::LINKRS_INT,
                data: linkrs_value_data_t { integer: 2 },
            };
            let mut result: *mut linkrs_result_t = ptr::null_mut();
            assert_eq!(
                linkrs_execute_with_variables(
                    session,
                    query.as_ptr(),
                    param_names.as_ptr(),
                    &param,
                    1,
                    var_names.as_ptr(),
                    &var,
                    1,
                    &mut result
                ),
                linkrs_error_code_t::LINKRS_OK as c_int
            );
            let mut got: i64 = 0;
            assert_eq!(
                linkrs_get_int_by_index(result, 0, 0, &mut got),
                linkrs_error_code_t::LINKRS_OK as c_int
            );
            assert_eq!(got, 42);
            linkrs_result_free(result);

            // Null or empty names fail the whole call.
            let mut result: *mut linkrs_result_t = ptr::null_mut();
            let null_names: *const *const c_char = ptr::null();
            assert_eq!(
                linkrs_execute_named_params(
                    session,
                    query.as_ptr(),
                    null_names,
                    &param,
                    1,
                    &mut result
                ),
                linkrs_error_code_t::LINKRS_MISUSE as c_int
            );
            let empty = CString::new("").unwrap();
            let empty_names = [empty.as_ptr()];
            assert_eq!(
                linkrs_execute_with_variables(
                    session,
                    query.as_ptr(),
                    empty_names.as_ptr(),
                    &param,
                    1,
                    var_names.as_ptr(),
                    &var,
                    1,
                    &mut result
                ),
                linkrs_error_code_t::LINKRS_MISUSE as c_int
            );

            linkrs_session_close(session);
            linkrs_close(db);
        }
    }

    #[test]
    fn test_dml_classification_skips_comments() {
        use linkrs_query::parser::Parser;
        use linkrs_query::{dml_op_from_stmt, DmlOp};
        fn classify(query: &str) -> Option<DmlOp> {
            let mut parser = Parser::new(query);
            let parsed = parser
                .parse()
                .unwrap_or_else(|e| panic!("parse failed for {query:?}: {e}"));
            assert!(!parser.has_errors(), "parse errors for {query:?}");
            dml_op_from_stmt(parsed.ast.stmt())
        }
        assert_eq!(classify("  -- comment\nCREATE (n)"), Some(DmlOp::Insert));
        assert_eq!(classify("/* block */ MERGE (n)"), Some(DmlOp::Insert));
        assert_eq!(
            classify("// slash\n-- dash\nSET n.x = 1"),
            Some(DmlOp::Update)
        );
        assert_eq!(classify("  match (n) return n"), None);
        assert_eq!(classify("OPTIONAL MATCH (n) RETURN n"), None);
        assert_eq!(
            classify("DELETE VERTEX person FROM \"v1\""),
            Some(DmlOp::Delete)
        );
    }

    #[test]
    fn test_execute_null_params() {
        let rc = unsafe { linkrs_execute(ptr::null_mut(), ptr::null(), ptr::null_mut()) };
        assert_eq!(rc, linkrs_error_code_t::LINKRS_MISUSE as c_int);

        let mut result: *mut linkrs_result_t = ptr::null_mut();
        let rc = unsafe { linkrs_execute(ptr::null_mut(), ptr::null(), &mut result) };
        assert_eq!(rc, linkrs_error_code_t::LINKRS_MISUSE as c_int);
    }

    #[test]
    fn test_execute_params_null_params() {
        let rc = unsafe {
            linkrs_execute_params(
                ptr::null_mut(),
                ptr::null(),
                ptr::null(),
                0,
                ptr::null_mut(),
            )
        };
        assert_eq!(rc, linkrs_error_code_t::LINKRS_MISUSE as c_int);

        let mut result: *mut linkrs_result_t = ptr::null_mut();
        let rc = unsafe {
            linkrs_execute_params(ptr::null_mut(), ptr::null(), ptr::null(), 0, &mut result)
        };
        assert_eq!(rc, linkrs_error_code_t::LINKRS_MISUSE as c_int);
    }

    #[test]
    fn test_execute_simple_query() {
        let db = create_test_db();
        let mut session: *mut linkrs_session_t = ptr::null_mut();

        let rc = unsafe { linkrs_session_create(db, &mut session) };
        assert_eq!(rc, linkrs_error_code_t::LINKRS_OK as c_int);

        let query = CString::new("RETURN 1").expect("Failed to create query CString");
        let mut result: *mut linkrs_result_t = ptr::null_mut();

        let rc = unsafe { linkrs_execute(session, query.as_ptr(), &mut result) };
        assert_eq!(rc, linkrs_error_code_t::LINKRS_OK as c_int);
        assert!(!result.is_null());

        unsafe { linkrs_result_free(result) };
        unsafe { linkrs_session_close(session) };
        unsafe { linkrs_close(db) };
    }

    #[test]
    fn test_literal_int_roundtrip_through_wide_getter() {
        use crate::embedded::c_api::result::{
            linkrs_column_type, linkrs_get_int_by_index, linkrs_result_execution_time_ms,
            linkrs_result_rows_scanned,
        };
        use crate::embedded::c_api::types::linkrs_value_type_t;

        let db = create_test_db();
        let mut session: *mut linkrs_session_t = ptr::null_mut();
        assert_eq!(
            unsafe { linkrs_session_create(db, &mut session) },
            linkrs_error_code_t::LINKRS_OK as c_int
        );

        // Integer literals evaluate to `BigInt`; the wide getter must accept them.
        let query = CString::new("RETURN 1").unwrap();
        let mut result: *mut linkrs_result_t = ptr::null_mut();
        assert_eq!(
            unsafe { linkrs_execute(session, query.as_ptr(), &mut result) },
            linkrs_error_code_t::LINKRS_OK as c_int
        );
        assert_eq!(
            unsafe { linkrs_column_type(result, 0) },
            linkrs_value_type_t::LINKRS_INT
        );
        let mut value: i64 = 0;
        assert_eq!(
            unsafe { linkrs_get_int_by_index(result, 0, 0, &mut value) },
            linkrs_error_code_t::LINKRS_OK as c_int
        );
        assert_eq!(value, 1);

        let mut ms: u64 = 0;
        assert_eq!(
            unsafe { linkrs_result_execution_time_ms(result, &mut ms) },
            linkrs_error_code_t::LINKRS_OK as c_int
        );
        let mut scanned: u64 = 0;
        assert_eq!(
            unsafe { linkrs_result_rows_scanned(result, &mut scanned) },
            linkrs_error_code_t::LINKRS_OK as c_int
        );
        assert_eq!(
            unsafe { linkrs_result_execution_time_ms(ptr::null_mut(), &mut ms) },
            linkrs_error_code_t::LINKRS_MISUSE as c_int
        );

        unsafe { linkrs_result_free(result) };
        unsafe { linkrs_session_close(session) };
        unsafe { linkrs_close(db) };
    }

    #[test]
    fn test_execute_params_positional_binding() {
        use crate::embedded::c_api::result::{linkrs_get_int_by_index, linkrs_result_free};
        use crate::embedded::c_api::types::{
            linkrs_value_data_t, linkrs_value_t, linkrs_value_type_t,
        };

        let db = create_test_db();
        let mut session: *mut linkrs_session_t = ptr::null_mut();
        assert_eq!(
            unsafe { linkrs_session_create(db, &mut session) },
            linkrs_error_code_t::LINKRS_OK as c_int
        );

        // `params[i]` binds `@param_{i}`.
        let query = CString::new("RETURN @param_0").unwrap();
        let param = linkrs_value_t {
            type_: linkrs_value_type_t::LINKRS_INT,
            data: linkrs_value_data_t { integer: 41 },
        };
        let mut result: *mut linkrs_result_t = ptr::null_mut();
        assert_eq!(
            unsafe { linkrs_execute_params(session, query.as_ptr(), &param, 1, &mut result) },
            linkrs_error_code_t::LINKRS_OK as c_int
        );
        let mut value: i64 = 0;
        assert_eq!(
            unsafe { linkrs_get_int_by_index(result, 0, 0, &mut value) },
            linkrs_error_code_t::LINKRS_OK as c_int
        );
        assert_eq!(value, 41);

        unsafe { linkrs_result_free(result) };
        unsafe { linkrs_session_close(session) };
        unsafe { linkrs_close(db) };
    }

    #[test]
    fn test_read_only_write_maps_to_readonly_code() {
        use crate::embedded::c_api::config::{
            linkrs_config_file, linkrs_config_free, linkrs_config_set_read_only,
        };
        use crate::embedded::c_api::database::linkrs_open_with_config;

        let counter = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let temp_dir = std::env::temp_dir().join("linkrs_c_api_test");
        std::fs::create_dir_all(&temp_dir).ok();
        let db_path = temp_dir.join(format!("test_ro_{}_{}.db", std::process::id(), counter));
        let path_cstring = CString::new(db_path.to_str().expect("Invalid path")).unwrap();

        let config = unsafe { linkrs_config_file(path_cstring.as_ptr()) };
        assert!(!config.is_null());
        assert_eq!(
            unsafe { linkrs_config_set_read_only(config, 1) },
            linkrs_error_code_t::LINKRS_OK as c_int
        );
        let mut db: *mut linkrs_t = ptr::null_mut();
        assert_eq!(
            unsafe { linkrs_open_with_config(config, &mut db) },
            linkrs_error_code_t::LINKRS_OK as c_int
        );
        assert!(!db.is_null());

        let mut session: *mut linkrs_session_t = ptr::null_mut();
        assert_eq!(
            unsafe { linkrs_session_create(db, &mut session) },
            linkrs_error_code_t::LINKRS_OK as c_int
        );
        let query = CString::new("DROP SPACE nosuch").unwrap();
        let mut result: *mut linkrs_result_t = ptr::null_mut();
        assert_eq!(
            unsafe { linkrs_execute(session, query.as_ptr(), &mut result) },
            linkrs_error_code_t::LINKRS_READONLY as c_int
        );

        unsafe { linkrs_session_close(session) };
        unsafe { linkrs_close(db) };
        unsafe { linkrs_config_free(config) };
    }
}
