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

                // Check whether it is a data modification operation, and then call the update hook.
                if let Some((operation, rowid)) = detect_data_modification(query_str, &query_result)
                {
                    let space_name_owned = handle.inner.current_space();
                    let space_name = space_name_owned.as_deref().unwrap_or("default");
                    handle.invoke_update_hook(operation, space_name, rowid);
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

                // Check whether it is a data modification operation, and then call the update hook.
                if let Some((operation, rowid)) = detect_data_modification(query_str, &query_result)
                {
                    let space_name_owned = handle.inner.current_space();
                    let space_name = space_name_owned.as_deref().unwrap_or("default");
                    handle.invoke_update_hook(operation, space_name, rowid);
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
/// # Safety
///
/// `c_value` must be a valid pointer to a properly initialized `graphdb_value_t` struct.
/// The caller must ensure that string pointers within the value are valid and properly aligned.
pub unsafe fn convert_c_value_to_rust(c_value: &graphdb_value_t) -> Value {
    use crate::embedded::c_api::types::graphdb_value_type_t;

    match c_value.type_ {
        graphdb_value_type_t::GRAPHDB_NULL => Value::Null(graphdb_core::value::NullType::Null),
        graphdb_value_type_t::GRAPHDB_BOOL => Value::Bool(c_value.data.boolean),
        graphdb_value_type_t::GRAPHDB_INT => Value::Int(c_value.data.integer as i32),
        graphdb_value_type_t::GRAPHDB_FLOAT => Value::Float(c_value.data.floating as f32),
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
        _ => Value::Null(graphdb_core::value::NullType::Null),
    }
}

/// Check whether the query represents a data modification operation.
///
/// Return a tuple of (operation type, affected rows). If it is not a data
/// modification operation, return None.
/// Operation type: 1=INSERT, 2=UPDATE, 3=DELETE
///
/// This is a best-effort heuristic over the leading keyword (after skipping
/// whitespace and `--` / `//` / `#` line comments and `/* */` block comments).
/// The affected-rows count comes from the result metadata, so embedded
/// observers should treat it as a row count rather than a stable row id.
fn detect_data_modification(
    query: &str,
    result: &crate::embedded::result::QueryResult,
) -> Option<(i32, i64)> {
    let keyword = leading_keyword(query);
    let operation = match keyword {
        // Row creation.
        "INSERT" | "CREATE" | "MERGE" => 1,
        // Property / structure modification.
        "UPDATE" | "SET" | "REMOVE" | "ALTER" => 2,
        // Removal.
        "DELETE" | "DETACH" | "DROP" => 3,
        _ => return None,
    };
    // `MATCH ... DELETE/SET/REMOVE/MERGE/CREATE` carries the write in its
    // trailing clause rather than the leading keyword.
    if keyword == "MATCH" || keyword == "WITH" || keyword == "UNWIND" || keyword == "OPTIONAL" {
        let upper = query.to_uppercase();
        if upper.contains("DELETE") || upper.contains("DETACH DELETE") {
            return Some((3, result.metadata().rows_returned as i64));
        }
        if upper.contains("SET")
            || upper.contains("REMOVE")
            || upper.contains("MERGE")
            || upper.contains("CREATE")
        {
            let op = if upper.contains("MERGE") || upper.contains("CREATE") {
                1
            } else {
                2
            };
            return Some((op, result.metadata().rows_returned as i64));
        }
        return None;
    }
    Some((operation, result.metadata().rows_returned as i64))
}

/// Extract the first keyword token, skipping whitespace and SQL/Cypher comments.
fn leading_keyword(query: &str) -> &str {
    let bytes = query.as_bytes();
    let mut pos = 0;
    while pos < bytes.len() {
        // Skip whitespace.
        while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }
        if pos >= bytes.len() {
            break;
        }
        // Line comments: `--`, `//`, `#`.
        if query[pos..].starts_with("--")
            || query[pos..].starts_with("//")
            || query[pos..].starts_with('#')
        {
            while pos < bytes.len() && bytes[pos] != b'\n' {
                pos += 1;
            }
            continue;
        }
        // Block comments: `/* ... */`.
        if query[pos..].starts_with("/*") {
            if let Some(end) = query[pos..].find("*/") {
                pos += end + 2;
                continue;
            }
            return "";
        }
        break;
    }
    let rest = &query[pos..];
    let end = rest
        .find(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != ':')
        .unwrap_or(rest.len());
    let mut token = &rest[..end];
    // Handle `OPTIONAL MATCH` / `DETACH DELETE` two-word openers.
    if token.eq_ignore_ascii_case("OPTIONAL") || token.eq_ignore_ascii_case("DETACH") {
        let after: &str = rest[end..].trim_start();
        let after_end = after
            .find(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != ':')
            .unwrap_or(after.len());
        if !after[..after_end].is_empty() {
            token = &after[..after_end];
        }
    }
    token
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
    fn test_leading_keyword_skips_comments() {
        assert_eq!(leading_keyword("  -- comment\nCREATE (n)"), "CREATE");
        assert_eq!(leading_keyword("/* block */ MERGE (n)"), "MERGE");
        assert_eq!(leading_keyword("# hash\n// slash\nSET n.x = 1"), "SET");
        assert_eq!(leading_keyword("  match (n) return n"), "match");
        assert_eq!(leading_keyword("OPTIONAL MATCH (n) RETURN n"), "MATCH");
        assert_eq!(leading_keyword("DETACH DELETE n"), "DELETE");
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
}
