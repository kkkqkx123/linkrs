//! C API Prepared Statements Module
//!
//! Provides prepared-statement handles over the embedded session API: the
//! statement text is validated once at `linkrs_prepare` time and parameter
//! bindings can be reused across executions.

use crate::embedded::c_api::error::{
    error_code_from_core_error, extended_error_code_from_core_error, linkrs_error_code_t,
    set_last_error_message,
};
use crate::embedded::c_api::query::convert_c_value_to_rust;
use crate::embedded::c_api::result::LinkrsResultHandle;
use crate::embedded::c_api::session::LinkrsSessionHandle;
use crate::embedded::c_api::types::{
    linkrs_result_t, linkrs_session_t, linkrs_stmt_t, linkrs_value_t,
};
use linkrs_core::Value;
use std::collections::HashMap;
use std::ffi::{c_char, c_int, CStr};
use std::ptr;

/// Internal structure of prepared statement handles.
///
/// The embedded `PreparedStatement` borrows its `Session`, which cannot
/// live in a self-referential FFI handle, so this handle stores the
/// equivalent parts: a non-owning session pointer (same reference-session
/// contract as `LinkrsTxnHandle`), the validated query text, and the
/// current parameter bindings.
pub struct LinkrsStmtHandle {
    pub(crate) session: *mut LinkrsSessionHandle,
    pub(crate) query: String,
    pub(crate) params: HashMap<String, Value>,
}

/// Prepare a statement for repeated execution with different bindings.
///
/// Validates the query text once (same checks as `linkrs_validate_query`),
/// then returns a handle whose `@name` parameters can be bound via
/// `linkrs_stmt_bind` and run via `linkrs_stmt_execute` /
/// `linkrs_stmt_execute_update`. Session variables (`$name`) resolve from
/// the live session snapshot at each execution.
///
/// # Arguments
/// - `session`: Session handle
/// - `sql`: Query statement (UTF-8 encoded)
/// - `out_stmt`: Output parameter, prepared statement handle
///
/// # Returns
/// - Success: LINKRS_OK
/// - Failure: Error code (details in the session error slot)
///
/// # Lifetime
/// The statement handle holds a non-owning reference to the session and
/// does not keep it alive: the caller must free every statement with
/// `linkrs_stmt_free` before closing the session.
///
/// # Safety
/// - `session` must be a valid session handle created by `linkrs_session_create`
/// - `sql` must be a valid pointer to a null-terminated UTF-8 string
/// - `out_stmt` must be a valid pointer to store the statement handle
/// - The caller is responsible for freeing the statement using `linkrs_stmt_free`
/// - The statement handle must not be used after freeing
#[no_mangle]
pub unsafe extern "C" fn linkrs_prepare(
    session: *mut linkrs_session_t,
    sql: *const c_char,
    out_stmt: *mut *mut linkrs_stmt_t,
) -> c_int {
    if session.is_null() || sql.is_null() || out_stmt.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    let query_str = match CStr::from_ptr(sql).to_str() {
        Ok(s) => s,
        Err(_) => return linkrs_error_code_t::LINKRS_MISUSE as c_int,
    };

    let handle = &mut *(session as *mut LinkrsSessionHandle);
    match handle.inner.validate_query(query_str) {
        Ok(validation) if validation.valid => {
            handle.clear_error();
            let stmt = Box::new(LinkrsStmtHandle {
                session: session as *mut LinkrsSessionHandle,
                query: query_str.to_string(),
                params: HashMap::new(),
            });
            *out_stmt = Box::into_raw(stmt) as *mut linkrs_stmt_t;
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
            let error =
                crate::api_core::error::CoreError::detailed_query_error(error_msg, extended, None);
            let (error_code, _) = error_code_from_core_error(&error);
            let extended_code = Some(extended_error_code_from_core_error(&error));
            handle.set_error(format!("{error}"), None, extended_code);
            *out_stmt = ptr::null_mut();
            error_code
        }
        Err(e) => {
            let (error_code, _) = error_code_from_core_error(&e);
            let error_msg = format!("{e}");
            let offset = e.error_offset();
            let extended_code = Some(extended_error_code_from_core_error(&e));
            handle.set_error(error_msg, offset, extended_code);
            *out_stmt = ptr::null_mut();
            error_code
        }
    }
}

/// Bind one `@name` parameter.
///
/// The value is converted from `linkrs_value_t` exactly like the parameter
/// values of `linkrs_execute_params`. Rebinding a name replaces the
/// previous value.
///
/// # Arguments
/// - `stmt`: Prepared statement handle
/// - `name`: Parameter name (UTF-8 encoded, without the `@` sigil)
/// - `value`: Value to bind
///
/// # Returns
/// - Success: LINKRS_OK
/// - Failure: Error code
///
/// # Safety
/// - `stmt` must be a valid statement handle created by `linkrs_prepare`
/// - `name` must be a valid pointer to a null-terminated UTF-8 string
/// - `value` must be a valid pointer to a properly initialized `linkrs_value_t`
/// - The associated session must still be valid
#[no_mangle]
pub unsafe extern "C" fn linkrs_stmt_bind(
    stmt: *mut linkrs_stmt_t,
    name: *const c_char,
    value: *const linkrs_value_t,
) -> c_int {
    if stmt.is_null() || name.is_null() || value.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    let name_str = match CStr::from_ptr(name).to_str() {
        Ok(s) => s,
        Err(_) => return linkrs_error_code_t::LINKRS_MISUSE as c_int,
    };

    if name_str.trim().is_empty() {
        let e = crate::api_core::error::CoreError::InvalidParameter(
            "parameter name must not be empty".to_string(),
        );
        let (error_code, _) = error_code_from_core_error(&e);
        set_last_error_message(format!("{e}"));
        return error_code;
    }

    let handle = &mut *(stmt as *mut LinkrsStmtHandle);
    let value = convert_c_value_to_rust(&*value);
    handle.params.insert(name_str.to_string(), value);
    linkrs_error_code_t::LINKRS_OK as c_int
}

/// Clear all parameter bindings.
///
/// # Arguments
/// - `stmt`: Prepared statement handle
///
/// # Returns
/// - Success: LINKRS_OK
/// - Failure: Error code
///
/// # Safety
/// - `stmt` must be a valid statement handle created by `linkrs_prepare`
#[no_mangle]
pub unsafe extern "C" fn linkrs_stmt_clear_bindings(stmt: *mut linkrs_stmt_t) -> c_int {
    if stmt.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    let handle = &mut *(stmt as *mut LinkrsStmtHandle);
    handle.params.clear();
    linkrs_error_code_t::LINKRS_OK as c_int
}

/// Reset the statement for re-execution (clears all bindings).
///
/// Mirrors the embedded `PreparedStatement::reset`, which is currently
/// equivalent to clearing the bindings.
///
/// # Arguments
/// - `stmt`: Prepared statement handle
///
/// # Returns
/// - Success: LINKRS_OK
/// - Failure: Error code
///
/// # Safety
/// - `stmt` must be a valid statement handle created by `linkrs_prepare`
#[no_mangle]
pub unsafe extern "C" fn linkrs_stmt_reset(stmt: *mut linkrs_stmt_t) -> c_int {
    linkrs_stmt_clear_bindings(stmt)
}

/// Execute the statement with the current bindings.
///
/// Runs the same path as `linkrs_execute_params` (including the SQL trace
/// and update-hook notifications) and returns a result handle with the
/// identical lifecycle.
///
/// # Arguments
/// - `stmt`: Prepared statement handle
/// - `out_result`: Output parameter, result set handle
///
/// # Returns
/// - Success: LINKRS_OK
/// - Failure: Error code
///
/// # Safety
/// - `stmt` must be a valid statement handle created by `linkrs_prepare`
/// - `out_result` must be a valid pointer to store the result handle
/// - The associated session must still be valid
/// - The caller is responsible for freeing the result handle using `linkrs_result_free`
#[no_mangle]
pub unsafe extern "C" fn linkrs_stmt_execute(
    stmt: *mut linkrs_stmt_t,
    out_result: *mut *mut linkrs_result_t,
) -> c_int {
    if stmt.is_null() || out_result.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    let handle = &mut *(stmt as *mut LinkrsStmtHandle);
    if handle.session.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    let query_str = handle.query.clone();
    let params = handle.params.clone();
    let session = &mut *handle.session;

    // Calling the SQL tracing callback (matches `linkrs_execute`).
    session.trace(&query_str);

    match session.inner.execute_with_params(&query_str, params) {
        Ok(query_result) => {
            session.clear_error();

            // Statement-level DML notification (bus) + legacy update hook,
            // matching `linkrs_execute_params`.
            if session.has_update_hook() || session.inner.has_dml_observers() {
                let rows = query_result.metadata().rows_returned as u64;
                if let Some(op) = session.inner.notify_dml(&query_str, rows) {
                    if session.has_update_hook() {
                        let operation = match op {
                            linkrs_query::DmlOp::Insert => 1,
                            linkrs_query::DmlOp::Update => 2,
                            linkrs_query::DmlOp::Delete => 3,
                        };
                        let space_name_owned = session.inner.current_space();
                        let space_name = space_name_owned.as_deref().unwrap_or("default");
                        session.invoke_update_hook(operation, space_name, rows as i64);
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
            let error_msg = format!("{e}");
            let offset = e.error_offset();
            let extended_code = Some(extended_error_code_from_core_error(&e));
            session.set_error(error_msg, offset, extended_code);
            *out_result = ptr::null_mut();
            error_code
        }
    }
}

/// Execute an updating statement, returning the number of rows returned.
///
/// Mirrors the embedded `PreparedStatement::execute_update`.
///
/// # Arguments
/// - `stmt`: Prepared statement handle
/// - `out_changes`: Output parameter, number of rows returned
///
/// # Returns
/// - Success: LINKRS_OK
/// - Failure: Error code
///
/// # Safety
/// - `stmt` must be a valid statement handle created by `linkrs_prepare`
/// - `out_changes` must be a valid pointer to store the result
/// - The associated session must still be valid
#[no_mangle]
pub unsafe extern "C" fn linkrs_stmt_execute_update(
    stmt: *mut linkrs_stmt_t,
    out_changes: *mut u64,
) -> c_int {
    if stmt.is_null() || out_changes.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    let mut result: *mut linkrs_result_t = ptr::null_mut();
    let rc = linkrs_stmt_execute(stmt, &mut result);
    if rc != linkrs_error_code_t::LINKRS_OK as c_int {
        return rc;
    }

    let result_handle = Box::from_raw(result as *mut LinkrsResultHandle);
    *out_changes = result_handle.inner.len() as u64;
    linkrs_error_code_t::LINKRS_OK as c_int
}

/// Free a prepared statement handle.
///
/// # Arguments
/// - `stmt`: Prepared statement handle (may be NULL, in which case this
///   function does nothing)
///
/// # Safety
/// - `stmt` must be a valid statement handle created by `linkrs_prepare` or NULL
/// - All statements must be freed before the associated session is closed
/// - After calling this function, the handle is invalid and must not be used again
#[no_mangle]
pub unsafe extern "C" fn linkrs_stmt_free(stmt: *mut linkrs_stmt_t) {
    if !stmt.is_null() {
        let _ = Box::from_raw(stmt as *mut LinkrsStmtHandle);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedded::c_api::config::linkrs_config_memory;
    use crate::embedded::c_api::database::{linkrs_close, linkrs_open_with_config};
    use crate::embedded::c_api::result::{linkrs_get_int_by_index, linkrs_result_free};
    use crate::embedded::c_api::session::{linkrs_session_close, linkrs_session_create};
    use crate::embedded::c_api::types::{linkrs_t, linkrs_value_data_t, linkrs_value_type_t};
    use std::ffi::CString;

    fn create_test_session() -> (*mut linkrs_t, *mut linkrs_session_t) {
        unsafe {
            let config = linkrs_config_memory();
            let mut db: *mut linkrs_t = ptr::null_mut();
            let rc = linkrs_open_with_config(config, &mut db);
            assert_eq!(rc, linkrs_error_code_t::LINKRS_OK as c_int);
            assert!(!db.is_null());

            let mut session: *mut linkrs_session_t = ptr::null_mut();
            let rc = linkrs_session_create(db, &mut session);
            assert_eq!(rc, linkrs_error_code_t::LINKRS_OK as c_int);
            assert!(!session.is_null());

            (db, session)
        }
    }

    fn int_value(v: i64) -> linkrs_value_t {
        linkrs_value_t {
            type_: linkrs_value_type_t::LINKRS_INT,
            data: linkrs_value_data_t { integer: v },
        }
    }

    #[test]
    fn test_prepare_bind_execute() {
        unsafe {
            let (db, session) = create_test_session();

            let sql = CString::new("RETURN @param_0").expect("sql CString");
            let mut stmt: *mut linkrs_stmt_t = ptr::null_mut();
            assert_eq!(
                linkrs_prepare(session, sql.as_ptr(), &mut stmt),
                linkrs_error_code_t::LINKRS_OK as c_int
            );
            assert!(!stmt.is_null());

            // Bind, execute, then rebind and re-execute the same statement.
            for expected in [7_i64, 9_i64] {
                let name = CString::new("param_0").expect("name CString");
                let value = int_value(expected);
                assert_eq!(
                    linkrs_stmt_bind(stmt, name.as_ptr(), &value),
                    linkrs_error_code_t::LINKRS_OK as c_int
                );

                let mut result: *mut linkrs_result_t = ptr::null_mut();
                assert_eq!(
                    linkrs_stmt_execute(stmt, &mut result),
                    linkrs_error_code_t::LINKRS_OK as c_int
                );
                assert!(!result.is_null());

                let mut out: i64 = 0;
                assert_eq!(
                    linkrs_get_int_by_index(result, 0, 0, &mut out),
                    linkrs_error_code_t::LINKRS_OK as c_int
                );
                assert_eq!(out, expected);
                linkrs_result_free(result);
            }

            assert_eq!(
                linkrs_stmt_clear_bindings(stmt),
                linkrs_error_code_t::LINKRS_OK as c_int
            );
            assert_eq!(
                linkrs_stmt_reset(stmt),
                linkrs_error_code_t::LINKRS_OK as c_int
            );

            linkrs_stmt_free(stmt);
            linkrs_session_close(session);
            linkrs_close(db);
        }
    }

    #[test]
    fn test_stmt_execute_update() {
        unsafe {
            let (db, session) = create_test_session();

            let sql = CString::new("RETURN 1").expect("sql CString");
            let mut stmt: *mut linkrs_stmt_t = ptr::null_mut();
            assert_eq!(
                linkrs_prepare(session, sql.as_ptr(), &mut stmt),
                linkrs_error_code_t::LINKRS_OK as c_int
            );

            let mut changes: u64 = 0;
            assert_eq!(
                linkrs_stmt_execute_update(stmt, &mut changes),
                linkrs_error_code_t::LINKRS_OK as c_int
            );
            assert_eq!(changes, 1);

            linkrs_stmt_free(stmt);
            linkrs_session_close(session);
            linkrs_close(db);
        }
    }

    #[test]
    fn test_prepare_invalid_query_fails() {
        unsafe {
            let (db, session) = create_test_session();

            let sql = CString::new("MATCH (n RETURN n").expect("sql CString");
            let mut stmt: *mut linkrs_stmt_t = ptr::null_mut();
            let rc = linkrs_prepare(session, sql.as_ptr(), &mut stmt);
            assert_ne!(rc, linkrs_error_code_t::LINKRS_OK as c_int);
            assert!(stmt.is_null());

            linkrs_session_close(session);
            linkrs_close(db);
        }
    }

    #[test]
    fn test_stmt_bind_empty_name_fails() {
        unsafe {
            let (db, session) = create_test_session();

            let sql = CString::new("RETURN 1").expect("sql CString");
            let mut stmt: *mut linkrs_stmt_t = ptr::null_mut();
            assert_eq!(
                linkrs_prepare(session, sql.as_ptr(), &mut stmt),
                linkrs_error_code_t::LINKRS_OK as c_int
            );

            let name = CString::new("").expect("name CString");
            let value = int_value(1);
            assert_ne!(
                linkrs_stmt_bind(stmt, name.as_ptr(), &value),
                linkrs_error_code_t::LINKRS_OK as c_int
            );

            linkrs_stmt_free(stmt);
            linkrs_session_close(session);
            linkrs_close(db);
        }
    }

    #[test]
    fn test_stmt_null_params() {
        unsafe {
            let mut stmt: *mut linkrs_stmt_t = ptr::null_mut();
            assert_eq!(
                linkrs_prepare(ptr::null_mut(), ptr::null(), &mut stmt),
                linkrs_error_code_t::LINKRS_MISUSE as c_int
            );
            assert_eq!(
                linkrs_stmt_bind(ptr::null_mut(), ptr::null(), ptr::null()),
                linkrs_error_code_t::LINKRS_MISUSE as c_int
            );
            assert_eq!(
                linkrs_stmt_clear_bindings(ptr::null_mut()),
                linkrs_error_code_t::LINKRS_MISUSE as c_int
            );
            assert_eq!(
                linkrs_stmt_reset(ptr::null_mut()),
                linkrs_error_code_t::LINKRS_MISUSE as c_int
            );
            let mut result: *mut linkrs_result_t = ptr::null_mut();
            assert_eq!(
                linkrs_stmt_execute(ptr::null_mut(), &mut result),
                linkrs_error_code_t::LINKRS_MISUSE as c_int
            );
            let mut changes: u64 = 0;
            assert_eq!(
                linkrs_stmt_execute_update(ptr::null_mut(), &mut changes),
                linkrs_error_code_t::LINKRS_MISUSE as c_int
            );
            linkrs_stmt_free(ptr::null_mut());
        }
    }
}
