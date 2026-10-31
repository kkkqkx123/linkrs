//! C API Session Variables Module
//!
//! Provides session-scoped user variable (`$name`) management: assignment,
//! lookup, removal, and full snapshot export. Values are marshalled with the
//! same `linkrs_value_t` convention as `linkrs_execute_params`.

use crate::embedded::c_api::error::{linkrs_error_code_t, set_last_error_message};
use crate::embedded::c_api::query::convert_c_value_to_rust;
use crate::embedded::c_api::session::LinkrsSessionHandle;
use crate::embedded::c_api::types::{linkrs_session_t, linkrs_value_t};
use std::ffi::{c_char, c_int, CStr, CString};
use std::ptr;

/// Assign a session variable (`$name`).
///
/// The value is converted from `linkrs_value_t` exactly like the parameter
/// values of `linkrs_execute_params`. Inside a text-begun transaction the
/// assignment is transaction-aware: ROLLBACK restores the previous value.
///
/// # Arguments
/// - `session`: Session handle
/// - `name`: Variable name (UTF-8 encoded, without the `$` sigil)
/// - `value`: Value to assign
///
/// # Returns
/// - Success: LINKRS_OK
/// - Failure: Error code
///
/// # Safety
/// - `session` must be a valid session handle created by `linkrs_session_create`
/// - `name` must be a valid pointer to a null-terminated UTF-8 string
/// - `value` must be a valid pointer to a properly initialized `linkrs_value_t`
#[no_mangle]
pub unsafe extern "C" fn linkrs_session_set_variable(
    session: *mut linkrs_session_t,
    name: *const c_char,
    value: *const linkrs_value_t,
) -> c_int {
    if session.is_null() || name.is_null() || value.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    let name_str = match CStr::from_ptr(name).to_str() {
        Ok(s) => s,
        Err(_) => return linkrs_error_code_t::LINKRS_MISUSE as c_int,
    };

    let handle = &mut *(session as *mut LinkrsSessionHandle);
    let value = convert_c_value_to_rust(&*value);
    handle.inner.set_variable(name_str.to_string(), value);
    handle.clear_error();
    linkrs_error_code_t::LINKRS_OK as c_int
}

/// Remove a session variable (`$name`).
///
/// Transaction-aware like `linkrs_session_set_variable`: inside a text-begun
/// transaction the removal is undone by ROLLBACK.
///
/// # Arguments
/// - `session`: Session handle
/// - `name`: Variable name (UTF-8 encoded, without the `$` sigil)
///
/// # Returns
/// - Success: LINKRS_OK (the variable was present and removed)
/// - LINKRS_NOTFOUND: no variable with this name
///
/// # Safety
/// - `session` must be a valid session handle created by `linkrs_session_create`
/// - `name` must be a valid pointer to a null-terminated UTF-8 string
#[no_mangle]
pub unsafe extern "C" fn linkrs_session_remove_variable(
    session: *mut linkrs_session_t,
    name: *const c_char,
) -> c_int {
    if session.is_null() || name.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    let name_str = match CStr::from_ptr(name).to_str() {
        Ok(s) => s,
        Err(_) => return linkrs_error_code_t::LINKRS_MISUSE as c_int,
    };

    let handle = &mut *(session as *mut LinkrsSessionHandle);
    if handle.inner.remove_variable(name_str) {
        handle.clear_error();
        linkrs_error_code_t::LINKRS_OK as c_int
    } else {
        linkrs_error_code_t::LINKRS_NOTFOUND as c_int
    }
}

/// Read one session variable (`$name`).
///
/// The value is written as a JSON string using the engine's serde encoding
/// of `linkrs_core::Value` (the same encoding the embedded result rows use
/// in JSON exports), for example `{"BigInt":42}` or `{"String":"hi"}`.
///
/// # Arguments
/// - `session`: Session handle
/// - `name`: Variable name (UTF-8 encoded, without the `$` sigil)
/// - `out_value`: Output parameter, receives the heap JSON string
///
/// # Returns
/// - Success: LINKRS_OK (`*out_value` holds the JSON string)
/// - LINKRS_NOTFOUND: no variable with this name (`*out_value` is NULL)
/// - Failure: Error code
///
/// # Memory Management
/// The returned string is dynamically allocated and must be freed by the
/// caller using `linkrs_free_string` to avoid memory leaks.
///
/// # Safety
/// - `session` must be a valid session handle created by `linkrs_session_create`
/// - `name` must be a valid pointer to a null-terminated UTF-8 string
/// - `out_value` must be a valid pointer to store the result
/// - The returned pointer must be freed by the caller to avoid memory leaks
#[no_mangle]
pub unsafe extern "C" fn linkrs_session_get_variable(
    session: *mut linkrs_session_t,
    name: *const c_char,
    out_value: *mut *mut c_char,
) -> c_int {
    if session.is_null() || name.is_null() || out_value.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    let name_str = match CStr::from_ptr(name).to_str() {
        Ok(s) => s,
        Err(_) => return linkrs_error_code_t::LINKRS_MISUSE as c_int,
    };

    let handle = &*(session as *mut LinkrsSessionHandle);
    match handle.inner.variable_value(name_str) {
        Some(value) => match serde_json::to_string(&value) {
            Ok(json) => match CString::new(json) {
                Ok(c_json) => {
                    *out_value = c_json.into_raw();
                    linkrs_error_code_t::LINKRS_OK as c_int
                }
                Err(_) => {
                    *out_value = ptr::null_mut();
                    linkrs_error_code_t::LINKRS_ERROR as c_int
                }
            },
            Err(e) => {
                *out_value = ptr::null_mut();
                set_last_error_message(format!("failed to serialize variable value: {e}"));
                linkrs_error_code_t::LINKRS_ERROR as c_int
            }
        },
        None => {
            *out_value = ptr::null_mut();
            linkrs_error_code_t::LINKRS_NOTFOUND as c_int
        }
    }
}

/// Snapshot all session variables.
///
/// Returns a heap JSON object mapping every variable name to its value in
/// the engine's serde encoding of `linkrs_core::Value` (the same encoding
/// the embedded result rows use in JSON exports).
///
/// # Arguments
/// - `session`: Session handle
///
/// # Returns
/// - JSON object string (UTF-8 encoded), returns NULL on error
///
/// # Memory Management
/// The returned string is dynamically allocated and must be freed by the
/// caller using `linkrs_free_string` to avoid memory leaks.
///
/// # Safety
/// - `session` must be a valid session handle created by `linkrs_session_create`
/// - The returned pointer must be freed by the caller to avoid memory leaks
#[no_mangle]
pub unsafe extern "C" fn linkrs_session_variables_snapshot(
    session: *mut linkrs_session_t,
) -> *mut c_char {
    if session.is_null() {
        return ptr::null_mut();
    }

    let handle = &*(session as *mut LinkrsSessionHandle);
    let snapshot = handle.inner.variables_snapshot();
    match serde_json::to_string(&snapshot) {
        Ok(json) => match CString::new(json) {
            Ok(c_json) => c_json.into_raw(),
            Err(_) => ptr::null_mut(),
        },
        Err(e) => {
            set_last_error_message(format!("failed to serialize variable snapshot: {e}"));
            ptr::null_mut()
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
    use crate::embedded::c_api::types::{linkrs_t, linkrs_value_data_t, linkrs_value_type_t};
    use linkrs_core::Value;

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

    fn string_value(s: &str) -> linkrs_value_t {
        linkrs_value_t {
            type_: linkrs_value_type_t::LINKRS_STRING,
            data: linkrs_value_data_t {
                string: crate::embedded::c_api::types::linkrs_string_t {
                    data: s.as_ptr() as *const c_char,
                    len: s.len(),
                },
            },
        }
    }

    #[test]
    fn test_session_variables_set_get_roundtrip() {
        unsafe {
            let (db, session) = create_test_session();

            let name = CString::new("x").expect("name CString");
            let value = int_value(41);
            assert_eq!(
                linkrs_session_set_variable(session, name.as_ptr(), &value),
                linkrs_error_code_t::LINKRS_OK as c_int
            );

            let mut out: *mut c_char = ptr::null_mut();
            assert_eq!(
                linkrs_session_get_variable(session, name.as_ptr(), &mut out),
                linkrs_error_code_t::LINKRS_OK as c_int
            );
            assert!(!out.is_null());
            let json = CStr::from_ptr(out).to_str().expect("utf8").to_string();
            linkrs_free_string(out);
            let decoded: Value = serde_json::from_str(&json).expect("value json");
            assert_eq!(decoded, Value::BigInt(41));

            linkrs_session_close(session);
            linkrs_close(db);
        }
    }

    #[test]
    fn test_session_variables_set_string() {
        unsafe {
            let (db, session) = create_test_session();

            let name = CString::new("greeting").expect("name CString");
            let value = string_value("hi");
            assert_eq!(
                linkrs_session_set_variable(session, name.as_ptr(), &value),
                linkrs_error_code_t::LINKRS_OK as c_int
            );

            let mut out: *mut c_char = ptr::null_mut();
            assert_eq!(
                linkrs_session_get_variable(session, name.as_ptr(), &mut out),
                linkrs_error_code_t::LINKRS_OK as c_int
            );
            let json = CStr::from_ptr(out).to_str().expect("utf8").to_string();
            linkrs_free_string(out);
            let decoded: Value = serde_json::from_str(&json).expect("value json");
            assert_eq!(decoded, Value::string("hi"));

            linkrs_session_close(session);
            linkrs_close(db);
        }
    }

    #[test]
    fn test_session_variables_get_missing_returns_notfound() {
        unsafe {
            let (db, session) = create_test_session();

            let name = CString::new("missing").expect("name CString");
            let mut out: *mut c_char = ptr::null_mut();
            assert_eq!(
                linkrs_session_get_variable(session, name.as_ptr(), &mut out),
                linkrs_error_code_t::LINKRS_NOTFOUND as c_int
            );
            assert!(out.is_null());

            linkrs_session_close(session);
            linkrs_close(db);
        }
    }

    #[test]
    fn test_session_variables_remove() {
        unsafe {
            let (db, session) = create_test_session();

            let name = CString::new("x").expect("name CString");
            let value = int_value(1);
            assert_eq!(
                linkrs_session_set_variable(session, name.as_ptr(), &value),
                linkrs_error_code_t::LINKRS_OK as c_int
            );
            assert_eq!(
                linkrs_session_remove_variable(session, name.as_ptr()),
                linkrs_error_code_t::LINKRS_OK as c_int
            );
            assert_eq!(
                linkrs_session_remove_variable(session, name.as_ptr()),
                linkrs_error_code_t::LINKRS_NOTFOUND as c_int
            );

            let mut out: *mut c_char = ptr::null_mut();
            assert_eq!(
                linkrs_session_get_variable(session, name.as_ptr(), &mut out),
                linkrs_error_code_t::LINKRS_NOTFOUND as c_int
            );

            linkrs_session_close(session);
            linkrs_close(db);
        }
    }

    #[test]
    fn test_session_variables_snapshot() {
        unsafe {
            let (db, session) = create_test_session();

            let name_a = CString::new("a").expect("name CString");
            let name_b = CString::new("b").expect("name CString");
            let value_a = int_value(1);
            let value_b = string_value("two");
            assert_eq!(
                linkrs_session_set_variable(session, name_a.as_ptr(), &value_a),
                linkrs_error_code_t::LINKRS_OK as c_int
            );
            assert_eq!(
                linkrs_session_set_variable(session, name_b.as_ptr(), &value_b),
                linkrs_error_code_t::LINKRS_OK as c_int
            );

            let out = linkrs_session_variables_snapshot(session);
            assert!(!out.is_null());
            let json = CStr::from_ptr(out).to_str().expect("utf8").to_string();
            linkrs_free_string(out);
            let snapshot: std::collections::HashMap<String, Value> =
                serde_json::from_str(&json).expect("snapshot json");
            assert_eq!(snapshot.len(), 2);
            assert_eq!(snapshot.get("a"), Some(&Value::BigInt(1)));
            assert_eq!(snapshot.get("b"), Some(&Value::string("two")));

            linkrs_session_close(session);
            linkrs_close(db);
        }
    }

    #[test]
    fn test_session_variables_null_args() {
        unsafe {
            assert_eq!(
                linkrs_session_set_variable(ptr::null_mut(), ptr::null(), ptr::null()),
                linkrs_error_code_t::LINKRS_MISUSE as c_int
            );
            assert_eq!(
                linkrs_session_remove_variable(ptr::null_mut(), ptr::null()),
                linkrs_error_code_t::LINKRS_MISUSE as c_int
            );
            assert_eq!(
                linkrs_session_get_variable(ptr::null_mut(), ptr::null(), ptr::null_mut()),
                linkrs_error_code_t::LINKRS_MISUSE as c_int
            );
            assert!(linkrs_session_variables_snapshot(ptr::null_mut()).is_null());
        }
    }
}
