//! C API Query Execution Module
//!
//! Provides the functionality to execute queries, including both simple queries and parameterized queries.

use crate::embedded::c_api::error::{
    error_code_from_core_error, extended_error_code_from_core_error, graphdb_error_code_t,
};
use crate::embedded::c_api::result::GraphDbResultHandle;
use crate::embedded::c_api::session::GraphDbSessionHandle;
use crate::embedded::c_api::types::{graphdb_result_t, graphdb_session_t, graphdb_value_t};
use graphdb_core::Value;
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
/// - Success: GRAPHDB_OK
/// - Failure: Error code
///
/// # Safety
/// - `session` must be a valid session handle created by `graphdb_session_create`
/// - `query` must be a valid pointer to a null-terminated UTF-8 string
/// - `result` must be a valid pointer to store the result handle
/// - The caller is responsible for freeing the result handle using `graphdb_result_free` when done
#[no_mangle]
pub unsafe extern "C" fn graphdb_execute(
    session: *mut graphdb_session_t,
    query: *const c_char,
    result: *mut *mut graphdb_result_t,
) -> c_int {
    if session.is_null() || query.is_null() || result.is_null() {
        return graphdb_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    let query_str = unsafe {
        match CStr::from_ptr(query).to_str() {
            Ok(s) => s,
            Err(_) => return graphdb_error_code_t::GRAPHDB_MISUSE as c_int,
        }
    };

    unsafe {
        let handle = &mut *(session as *mut GraphDbSessionHandle);

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
                                graphdb_query::DmlOp::Insert => 1,
                                graphdb_query::DmlOp::Update => 2,
                                graphdb_query::DmlOp::Delete => 3,
                            };
                            let space_name_owned = handle.inner.current_space();
                            let space_name = space_name_owned.as_deref().unwrap_or("default");
                            handle.invoke_update_hook(operation, space_name, rows as i64);
                        }
                    }
                }

                let result_handle = Box::new(GraphDbResultHandle {
                    inner: query_result,
                });
                *result = Box::into_raw(result_handle) as *mut graphdb_result_t;
                graphdb_error_code_t::GRAPHDB_OK as c_int
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
/// - Success: GRAPHDB_OK
/// - Failure: Error code
///
/// # Safety
/// - `session` must be a valid session handle created by `graphdb_session_create`
/// - `query` must be a valid pointer to a null-terminated UTF-8 string
/// - `result` must be a valid pointer to store the result handle
/// - If `params` is not NULL, it must point to at least `param_count` valid `graphdb_value_t` elements
/// - The caller is responsible for freeing the result handle using `graphdb_result_free` when done
#[no_mangle]
pub unsafe extern "C" fn graphdb_execute_params(
    session: *mut graphdb_session_t,
    query: *const c_char,
    params: *const graphdb_value_t,
    param_count: usize,
    result: *mut *mut graphdb_result_t,
) -> c_int {
    if session.is_null() || query.is_null() || result.is_null() {
        return graphdb_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    let query_str = unsafe {
        match CStr::from_ptr(query).to_str() {
            Ok(s) => s,
            Err(_) => return graphdb_error_code_t::GRAPHDB_MISUSE as c_int,
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
        let handle = &mut *(session as *mut GraphDbSessionHandle);

        // Calling the SQL tracing callback (matches `graphdb_execute`).
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
                                graphdb_query::DmlOp::Insert => 1,
                                graphdb_query::DmlOp::Update => 2,
                                graphdb_query::DmlOp::Delete => 3,
                            };
                            let space_name_owned = handle.inner.current_space();
                            let space_name = space_name_owned.as_deref().unwrap_or("default");
                            handle.invoke_update_hook(operation, space_name, rows as i64);
                        }
                    }
                }

                let result_handle = Box::new(GraphDbResultHandle {
                    inner: query_result,
                });
                *result = Box::into_raw(result_handle) as *mut graphdb_result_t;
                graphdb_error_code_t::GRAPHDB_OK as c_int
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
/// `c_value` must be a valid pointer to a properly initialized `graphdb_value_t` struct.
/// The caller must ensure that string/blob pointers within the value are valid and properly aligned.
pub unsafe fn convert_c_value_to_rust(c_value: &graphdb_value_t) -> Value {
    use crate::embedded::c_api::types::graphdb_value_type_t;

    match c_value.type_ {
        graphdb_value_type_t::GRAPHDB_NULL => Value::Null(graphdb_core::value::NullType::Null),
        graphdb_value_type_t::GRAPHDB_BOOL => Value::Bool(c_value.data.boolean),
        graphdb_value_type_t::GRAPHDB_INT => Value::BigInt(c_value.data.integer),
        graphdb_value_type_t::GRAPHDB_FLOAT => Value::Double(c_value.data.floating),
        graphdb_value_type_t::GRAPHDB_STRING => {
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
        graphdb_value_type_t::GRAPHDB_BLOB => {
            if c_value.data.blob.data.is_null() || c_value.data.blob.len == 0 {
                Value::Blob(Vec::new())
            } else {
                let slice =
                    std::slice::from_raw_parts(c_value.data.blob.data, c_value.data.blob.len);
                Value::Blob(slice.to_vec())
            }
        }
        _ => Value::Null(graphdb_core::value::NullType::Null),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedded::c_api::database::{graphdb_close, graphdb_open};
    use crate::embedded::c_api::result::graphdb_result_free;
    use crate::embedded::c_api::session::{graphdb_session_close, graphdb_session_create};
    use crate::embedded::c_api::types::graphdb_t;
    use std::ffi::CString;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn create_test_db() -> *mut graphdb_t {
        let counter = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let temp_dir = std::env::temp_dir().join("graphdb_c_api_test");
        std::fs::create_dir_all(&temp_dir).ok();
        let db_path = temp_dir.join(format!("test_{}_{}.db", std::process::id(), counter));

        let path_cstring = CString::new(db_path.to_str().expect("Invalid path"))
            .expect("Failed to create CString");
        let mut db: *mut graphdb_t = ptr::null_mut();

        let rc = unsafe { graphdb_open(path_cstring.as_ptr(), &mut db) };
        assert_eq!(rc, graphdb_error_code_t::GRAPHDB_OK as c_int);
        assert!(!db.is_null());

        db
    }

    #[test]
    fn test_dml_classification_skips_comments() {
        use graphdb_query::{classify_dml, DmlOp};
        assert_eq!(
            classify_dml("  -- comment\nCREATE (n)"),
            Some(DmlOp::Insert)
        );
        assert_eq!(classify_dml("/* block */ MERGE (n)"), Some(DmlOp::Insert));
        assert_eq!(
            classify_dml("# hash\n// slash\nSET n.x = 1"),
            Some(DmlOp::Update)
        );
        assert_eq!(classify_dml("  match (n) return n"), None);
        assert_eq!(classify_dml("OPTIONAL MATCH (n) RETURN n"), None);
        assert_eq!(classify_dml("DETACH DELETE n"), Some(DmlOp::Delete));
    }

    #[test]
    fn test_execute_null_params() {
        let rc = unsafe { graphdb_execute(ptr::null_mut(), ptr::null(), ptr::null_mut()) };
        assert_eq!(rc, graphdb_error_code_t::GRAPHDB_MISUSE as c_int);

        let mut result: *mut graphdb_result_t = ptr::null_mut();
        let rc = unsafe { graphdb_execute(ptr::null_mut(), ptr::null(), &mut result) };
        assert_eq!(rc, graphdb_error_code_t::GRAPHDB_MISUSE as c_int);
    }

    #[test]
    fn test_execute_params_null_params() {
        let rc = unsafe {
            graphdb_execute_params(
                ptr::null_mut(),
                ptr::null(),
                ptr::null(),
                0,
                ptr::null_mut(),
            )
        };
        assert_eq!(rc, graphdb_error_code_t::GRAPHDB_MISUSE as c_int);

        let mut result: *mut graphdb_result_t = ptr::null_mut();
        let rc = unsafe {
            graphdb_execute_params(ptr::null_mut(), ptr::null(), ptr::null(), 0, &mut result)
        };
        assert_eq!(rc, graphdb_error_code_t::GRAPHDB_MISUSE as c_int);
    }

    #[test]
    fn test_execute_simple_query() {
        let db = create_test_db();
        let mut session: *mut graphdb_session_t = ptr::null_mut();

        let rc = unsafe { graphdb_session_create(db, &mut session) };
        assert_eq!(rc, graphdb_error_code_t::GRAPHDB_OK as c_int);

        let query = CString::new("RETURN 1").expect("Failed to create query CString");
        let mut result: *mut graphdb_result_t = ptr::null_mut();

        let rc = unsafe { graphdb_execute(session, query.as_ptr(), &mut result) };
        assert_eq!(rc, graphdb_error_code_t::GRAPHDB_OK as c_int);
        assert!(!result.is_null());

        unsafe { graphdb_result_free(result) };
        unsafe { graphdb_session_close(session) };
        unsafe { graphdb_close(db) };
    }

    #[test]
    fn test_literal_int_roundtrip_through_wide_getter() {
        use crate::embedded::c_api::result::{
            graphdb_column_type, graphdb_get_int_by_index, graphdb_result_execution_time_ms,
            graphdb_result_rows_scanned,
        };
        use crate::embedded::c_api::types::graphdb_value_type_t;

        let db = create_test_db();
        let mut session: *mut graphdb_session_t = ptr::null_mut();
        assert_eq!(
            unsafe { graphdb_session_create(db, &mut session) },
            graphdb_error_code_t::GRAPHDB_OK as c_int
        );

        // Integer literals evaluate to `BigInt`; the wide getter must accept them.
        let query = CString::new("RETURN 1").unwrap();
        let mut result: *mut graphdb_result_t = ptr::null_mut();
        assert_eq!(
            unsafe { graphdb_execute(session, query.as_ptr(), &mut result) },
            graphdb_error_code_t::GRAPHDB_OK as c_int
        );
        assert_eq!(
            unsafe { graphdb_column_type(result, 0) },
            graphdb_value_type_t::GRAPHDB_INT
        );
        let mut value: i64 = 0;
        assert_eq!(
            unsafe { graphdb_get_int_by_index(result, 0, 0, &mut value) },
            graphdb_error_code_t::GRAPHDB_OK as c_int
        );
        assert_eq!(value, 1);

        let mut ms: u64 = 0;
        assert_eq!(
            unsafe { graphdb_result_execution_time_ms(result, &mut ms) },
            graphdb_error_code_t::GRAPHDB_OK as c_int
        );
        let mut scanned: u64 = 0;
        assert_eq!(
            unsafe { graphdb_result_rows_scanned(result, &mut scanned) },
            graphdb_error_code_t::GRAPHDB_OK as c_int
        );
        assert_eq!(
            unsafe { graphdb_result_execution_time_ms(ptr::null_mut(), &mut ms) },
            graphdb_error_code_t::GRAPHDB_MISUSE as c_int
        );

        unsafe { graphdb_result_free(result) };
        unsafe { graphdb_session_close(session) };
        unsafe { graphdb_close(db) };
    }

    #[test]
    fn test_execute_params_positional_binding() {
        use crate::embedded::c_api::result::{graphdb_get_int_by_index, graphdb_result_free};
        use crate::embedded::c_api::types::{
            graphdb_value_data_t, graphdb_value_t, graphdb_value_type_t,
        };

        let db = create_test_db();
        let mut session: *mut graphdb_session_t = ptr::null_mut();
        assert_eq!(
            unsafe { graphdb_session_create(db, &mut session) },
            graphdb_error_code_t::GRAPHDB_OK as c_int
        );

        // `params[i]` binds `@param_{i}`.
        let query = CString::new("RETURN @param_0").unwrap();
        let param = graphdb_value_t {
            type_: graphdb_value_type_t::GRAPHDB_INT,
            data: graphdb_value_data_t { integer: 41 },
        };
        let mut result: *mut graphdb_result_t = ptr::null_mut();
        assert_eq!(
            unsafe { graphdb_execute_params(session, query.as_ptr(), &param, 1, &mut result) },
            graphdb_error_code_t::GRAPHDB_OK as c_int
        );
        let mut value: i64 = 0;
        assert_eq!(
            unsafe { graphdb_get_int_by_index(result, 0, 0, &mut value) },
            graphdb_error_code_t::GRAPHDB_OK as c_int
        );
        assert_eq!(value, 41);

        unsafe { graphdb_result_free(result) };
        unsafe { graphdb_session_close(session) };
        unsafe { graphdb_close(db) };
    }

    #[test]
    fn test_read_only_write_maps_to_readonly_code() {
        use crate::embedded::c_api::config::{
            graphdb_config_file, graphdb_config_free, graphdb_config_set_read_only,
        };
        use crate::embedded::c_api::database::graphdb_open_with_config;

        let counter = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let temp_dir = std::env::temp_dir().join("graphdb_c_api_test");
        std::fs::create_dir_all(&temp_dir).ok();
        let db_path = temp_dir.join(format!("test_ro_{}_{}.db", std::process::id(), counter));
        let path_cstring = CString::new(db_path.to_str().expect("Invalid path")).unwrap();

        let config = unsafe { graphdb_config_file(path_cstring.as_ptr()) };
        assert!(!config.is_null());
        assert_eq!(
            unsafe { graphdb_config_set_read_only(config, 1) },
            graphdb_error_code_t::GRAPHDB_OK as c_int
        );
        let mut db: *mut graphdb_t = ptr::null_mut();
        assert_eq!(
            unsafe { graphdb_open_with_config(config, &mut db) },
            graphdb_error_code_t::GRAPHDB_OK as c_int
        );
        assert!(!db.is_null());

        let mut session: *mut graphdb_session_t = ptr::null_mut();
        assert_eq!(
            unsafe { graphdb_session_create(db, &mut session) },
            graphdb_error_code_t::GRAPHDB_OK as c_int
        );
        let query = CString::new("DROP SPACE nosuch").unwrap();
        let mut result: *mut graphdb_result_t = ptr::null_mut();
        assert_eq!(
            unsafe { graphdb_execute(session, query.as_ptr(), &mut result) },
            graphdb_error_code_t::GRAPHDB_READONLY as c_int
        );

        unsafe { graphdb_session_close(session) };
        unsafe { graphdb_close(db) };
        unsafe { graphdb_config_free(config) };
    }
}
