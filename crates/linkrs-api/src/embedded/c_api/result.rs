//! C API Results Processing Module
//!
//! Provide processing functions for query results

use crate::embedded::c_api::error::linkrs_error_code_t;
use crate::embedded::c_api::types::linkrs_result_t;
use crate::embedded::result::QueryResult;
use std::ffi::{c_char, c_int, CStr, CString};
use std::ptr;

/// Internal structure of result set handles
pub struct GraphDbResultHandle {
    pub(crate) inner: QueryResult,
}

/// Extract an integer across all engine integer widths.
///
/// Literals and most expressions evaluate to `BigInt`; narrow `SmallInt` /
/// `Int` storage values must read back through the same accessor.
fn value_as_i64(value: &linkrs_core::Value) -> Option<i64> {
    match value {
        linkrs_core::Value::SmallInt(v) => Some(*v as i64),
        linkrs_core::Value::Int(v) => Some(*v as i64),
        linkrs_core::Value::BigInt(v) => Some(*v),
        _ => None,
    }
}

/// Extract a float across both engine float widths.
fn value_as_f64(value: &linkrs_core::Value) -> Option<f64> {
    match value {
        linkrs_core::Value::Float(v) => Some(*v as f64),
        linkrs_core::Value::Double(v) => Some(*v),
        _ => None,
    }
}

/// Extract a string across both engine string variants.
fn value_as_str(value: &linkrs_core::Value) -> Option<&str> {
    match value {
        linkrs_core::Value::String(s) => Some(s.as_str()),
        linkrs_core::Value::FixedString(s) => Some(s.as_str()),
        _ => None,
    }
}

/// Releasing the result set
///
/// # Arguments
/// - `result`: Result set handle
///
/// # Returns
/// - Success: GRAPHDB_OK
/// - Failure: Error code
///
/// # Safety
/// - `result` must be a valid result handle created by `linkrs_execute` or `linkrs_execute_params`
/// - After calling this function, the result handle becomes invalid and must not be used
/// - Any string pointers obtained from this result set become invalid after this call
#[no_mangle]
pub unsafe extern "C" fn linkrs_result_free(result: *mut linkrs_result_t) -> c_int {
    if result.is_null() {
        return linkrs_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    let _ = Box::from_raw(result as *mut GraphDbResultHandle);

    linkrs_error_code_t::GRAPHDB_OK as c_int
}

/// Get the number of columns in the result set
///
/// # Arguments
/// - `result`: Result set handle
///
/// # Returns
/// - Number of columns, returns -1 on error
///
/// # Safety
/// - `result` must be a valid result handle created by `linkrs_execute` or `linkrs_execute_params`
#[no_mangle]
pub unsafe extern "C" fn linkrs_column_count(result: *mut linkrs_result_t) -> c_int {
    if result.is_null() {
        return -1;
    }

    let handle = &*(result as *mut GraphDbResultHandle);
    handle.inner.columns().len() as c_int
}

/// Get the number of rows in the result set
///
/// # Arguments
/// - `result`: Result set handle
///
/// # Returns
/// - Number of rows, returns -1 on error
///
/// # Safety
/// - `result` must be a valid result handle created by `linkrs_execute` or `linkrs_execute_params`
#[no_mangle]
pub unsafe extern "C" fn linkrs_row_count(result: *mut linkrs_result_t) -> c_int {
    if result.is_null() {
        return -1;
    }

    let handle = &*(result as *mut GraphDbResultHandle);
    handle.inner.len() as c_int
}

/// Getting Column Names
///
/// # Arguments
/// - `result`: Result set handle
/// - `index`: Column index (starting from 0)
///
/// # Returns
/// - Column name (UTF-8 encoded), returns NULL on error
///
/// # Memory Management
/// The returned string is dynamically allocated and must be freed by the caller using `linkrs_free_string`
/// to avoid memory leaks.
///
/// # Safety
/// - `result` must be a valid result handle created by `linkrs_execute` or `linkrs_execute_params`
/// - `index` must be a valid column index (0 <= index < column count)
/// - The returned pointer must be freed by the caller to avoid memory leaks
#[no_mangle]
pub unsafe extern "C" fn linkrs_column_name(
    result: *mut linkrs_result_t,
    index: c_int,
) -> *mut c_char {
    if result.is_null() {
        return ptr::null_mut();
    }

    let handle = &*(result as *mut GraphDbResultHandle);

    match handle.inner.columns().get(index as usize) {
        Some(name) => match CString::new(name.as_str()) {
            Ok(c_name) => c_name.into_raw(),
            Err(_) => ptr::null_mut(),
        },
        None => ptr::null_mut(),
    }
}

/// Get integer value
///
/// # Arguments
/// - `result`: Result set handle
/// - `row`: Row index (starting from 0)
/// - `col`: Column name (UTF-8 encoded)
/// - `value`: Output parameter, integer value
///
/// # Returns
/// - Success: GRAPHDB_OK
/// - Failure: Error code
///
/// # Safety
/// - `result` must be a valid result handle created by `linkrs_execute` or `linkrs_execute_params`
/// - `col` must be a valid pointer to a null-terminated UTF-8 string
/// - `value` must be a valid pointer to store the result
/// - `row` must be a valid row index (0 <= row < row count)
#[no_mangle]
pub unsafe extern "C" fn linkrs_get_int(
    result: *mut linkrs_result_t,
    row: c_int,
    col: *const c_char,
    value: *mut i64,
) -> c_int {
    if result.is_null() || col.is_null() || value.is_null() {
        return linkrs_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    let col_str = match CStr::from_ptr(col).to_str() {
        Ok(s) => s,
        Err(_) => return linkrs_error_code_t::GRAPHDB_MISUSE as c_int,
    };

    let handle = &*(result as *mut GraphDbResultHandle);

    match handle.inner.get(row as usize) {
        Some(row_data) => match row_data.get(col_str) {
            Some(v) => match value_as_i64(v) {
                Some(i) => {
                    *value = i;
                    linkrs_error_code_t::GRAPHDB_OK as c_int
                }
                None => linkrs_error_code_t::GRAPHDB_MISMATCH as c_int,
            },
            None => linkrs_error_code_t::GRAPHDB_NOTFOUND as c_int,
        },
        None => linkrs_error_code_t::GRAPHDB_NOTFOUND as c_int,
    }
}

/// Getting String Values
///
/// # Arguments
/// - `result`: Result set handle
/// - `row`: Row index (starting from 0)
/// - `col`: Column name (UTF-8 encoded)
/// - `len`: Output parameter, string length
///
/// # Returns
/// - String value (UTF-8 encoded), returns NULL on error
///
/// # Memory Management
/// The returned string is dynamically allocated and must be freed by the caller using `linkrs_free_string`
/// to avoid memory leaks.
///
/// # Safety
/// - `result` must be a valid result handle created by `linkrs_execute` or `linkrs_execute_params`
/// - `col` must be a valid pointer to a null-terminated UTF-8 string
/// - `len` must be a valid pointer to store the string length, or NULL if not needed
/// - `row` must be a valid row index (0 <= row < row count)
/// - The returned pointer must be freed by the caller to avoid memory leaks
#[no_mangle]
pub unsafe extern "C" fn linkrs_get_string(
    result: *mut linkrs_result_t,
    row: c_int,
    col: *const c_char,
    len: *mut c_int,
) -> *mut c_char {
    if result.is_null() || col.is_null() {
        if !len.is_null() {
            *len = -1;
        }
        return ptr::null_mut();
    }

    let col_str = match CStr::from_ptr(col).to_str() {
        Ok(s) => s,
        Err(_) => {
            if !len.is_null() {
                *len = -1;
            }
            return ptr::null_mut();
        }
    };

    let handle = &*(result as *mut GraphDbResultHandle);

    match handle.inner.get(row as usize) {
        Some(row_data) => match row_data.get(col_str) {
            Some(v) => match value_as_str(v) {
                Some(s) => {
                    if !len.is_null() {
                        *len = s.len() as c_int;
                    }
                    match CString::new(s) {
                        Ok(c_str) => c_str.into_raw(),
                        Err(_) => ptr::null_mut(),
                    }
                }
                None => {
                    if !len.is_null() {
                        *len = -1;
                    }
                    ptr::null_mut()
                }
            },
            None => ptr::null_mut(),
        },
        None => ptr::null_mut(),
    }
}

/// Get Binary Data
///
/// # Arguments
/// - `result`: Result set handle
/// - `row`: Row index (starting from 0)
/// - `col`: Column name (UTF-8 encoded)
/// - `len`: Output parameter, data length (in bytes)
///
/// # Returns
/// - Data pointer, returns NULL on error
///
/// # Note
/// The returned pointer's lifetime is bound to the result set; the caller should not free it
///
/// # Safety
/// - `result` must be a valid result handle created by `linkrs_execute` or `linkrs_execute_params`
/// - `col` must be a valid pointer to a null-terminated UTF-8 string
/// - `len` must be a valid pointer to store the data length, or NULL if not needed
/// - `row` must be a valid row index (0 <= row < row count)
/// - The returned pointer is only valid as long as the result set is not freed
#[no_mangle]
pub unsafe extern "C" fn linkrs_get_blob(
    result: *mut linkrs_result_t,
    row: c_int,
    col: *const c_char,
    len: *mut c_int,
) -> *const u8 {
    if result.is_null() || col.is_null() {
        if !len.is_null() {
            *len = -1;
        }
        return ptr::null();
    }

    let col_str = match CStr::from_ptr(col).to_str() {
        Ok(s) => s,
        Err(_) => {
            if !len.is_null() {
                *len = -1;
            }
            return ptr::null();
        }
    };

    let handle = &*(result as *mut GraphDbResultHandle);

    match handle.inner.get(row as usize) {
        Some(row_data) => match row_data.get(col_str) {
            Some(linkrs_core::Value::Blob(blob)) => {
                if !len.is_null() {
                    *len = blob.len() as c_int;
                }
                blob.as_ptr()
            }
            Some(_) => {
                if !len.is_null() {
                    *len = -1;
                }
                ptr::null()
            }
            None => {
                if !len.is_null() {
                    *len = -1;
                }
                ptr::null()
            }
        },
        None => {
            if !len.is_null() {
                *len = -1;
            }
            ptr::null()
        }
    }
}

/// Get integer values (indexed by column)
///
/// # Arguments
/// - `result`: Result set handle
/// - `row`: Row index (starting from 0)
/// - `col`: Column index (starting from 0)
/// - `value`: Output parameter, integer value
///
/// # Returns
/// - Success: GRAPHDB_OK
/// - Failure: Error code
///
/// # Safety
/// - `result` must be a valid result handle created by `linkrs_execute` or `linkrs_execute_params`
/// - `value` must be a valid pointer to store the result
/// - `row` must be a valid row index (0 <= row < row count)
/// - `col` must be a valid column index (0 <= col < column count)
#[no_mangle]
pub unsafe extern "C" fn linkrs_get_int_by_index(
    result: *mut linkrs_result_t,
    row: c_int,
    col: c_int,
    value: *mut i64,
) -> c_int {
    if result.is_null() || value.is_null() || col < 0 {
        return linkrs_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    let handle = &*(result as *mut GraphDbResultHandle);

    // Getting Column Names
    let columns = handle.inner.columns();
    let col_name = match columns.get(col as usize) {
        Some(name) => name.as_str(),
        None => return linkrs_error_code_t::GRAPHDB_NOTFOUND as c_int,
    };

    match handle.inner.get(row as usize) {
        Some(row_data) => match row_data.get(col_name) {
            Some(v) => match value_as_i64(v) {
                Some(i) => {
                    *value = i;
                    linkrs_error_code_t::GRAPHDB_OK as c_int
                }
                None => linkrs_error_code_t::GRAPHDB_MISMATCH as c_int,
            },
            None => linkrs_error_code_t::GRAPHDB_NOTFOUND as c_int,
        },
        None => linkrs_error_code_t::GRAPHDB_NOTFOUND as c_int,
    }
}

/// Get string value (indexed by column)
///
/// # Arguments
/// - `result`: Result set handle
/// - `row`: Row index (starting from 0)
/// - `col`: Column index (starting from 0)
/// - `len`: Output parameter, string length
///
/// # Returns
/// - String value (UTF-8 encoded), returns NULL on error
///
/// # Memory Management
/// The returned string is dynamically allocated and must be freed by the caller using `linkrs_free_string`
/// to avoid memory leaks.
///
/// # Safety
/// - `result` must be a valid result handle created by `linkrs_execute` or `linkrs_execute_params`
/// - `len` must be a valid pointer to store the string length, or NULL if not needed
/// - `row` must be a valid row index (0 <= row < row count)
/// - `col` must be a valid column index (0 <= col < column count)
/// - The returned pointer must be freed by the caller to avoid memory leaks
#[no_mangle]
pub unsafe extern "C" fn linkrs_get_string_by_index(
    result: *mut linkrs_result_t,
    row: c_int,
    col: c_int,
    len: *mut c_int,
) -> *mut c_char {
    if result.is_null() || col < 0 {
        if !len.is_null() {
            *len = -1;
        }
        return ptr::null_mut();
    }

    let handle = &*(result as *mut GraphDbResultHandle);

    let columns = handle.inner.columns();
    let col_name = match columns.get(col as usize) {
        Some(name) => name.as_str(),
        None => {
            if !len.is_null() {
                *len = -1;
            }
            return ptr::null_mut();
        }
    };

    match handle.inner.get(row as usize) {
        Some(row_data) => match row_data.get(col_name) {
            Some(v) => match value_as_str(v) {
                Some(s) => {
                    if !len.is_null() {
                        *len = s.len() as c_int;
                    }
                    match CString::new(s) {
                        Ok(c_str) => c_str.into_raw(),
                        Err(_) => ptr::null_mut(),
                    }
                }
                None => {
                    if !len.is_null() {
                        *len = -1;
                    }
                    ptr::null_mut()
                }
            },
            None => {
                if !len.is_null() {
                    *len = -1;
                }
                ptr::null_mut()
            }
        },
        None => {
            if !len.is_null() {
                *len = -1;
            }
            ptr::null_mut()
        }
    }
}

/// Get Boolean value (indexed by column)
///
/// # Arguments
/// - `result`: Result set handle
/// - `row`: Row index (starting from 0)
/// - `col`: Column index (starting from 0)
/// - `value`: Output parameter, boolean value
///
/// # Returns
/// - Success: GRAPHDB_OK
/// - Failure: Error code
///
/// # Safety
/// - `result` must be a valid result handle created by `linkrs_execute` or `linkrs_execute_params`
/// - `value` must be a valid pointer to store the result
/// - `row` must be a valid row index (0 <= row < row count)
/// - `col` must be a valid column index (0 <= col < column count)
#[no_mangle]
pub unsafe extern "C" fn linkrs_get_bool_by_index(
    result: *mut linkrs_result_t,
    row: c_int,
    col: c_int,
    value: *mut bool,
) -> c_int {
    if result.is_null() || value.is_null() || col < 0 {
        return linkrs_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    let handle = &*(result as *mut GraphDbResultHandle);

    let columns = handle.inner.columns();
    let col_name = match columns.get(col as usize) {
        Some(name) => name.as_str(),
        None => return linkrs_error_code_t::GRAPHDB_NOTFOUND as c_int,
    };

    match handle.inner.get(row as usize) {
        Some(row_data) => match row_data.get(col_name) {
            Some(linkrs_core::Value::Bool(b)) => {
                *value = *b;
                linkrs_error_code_t::GRAPHDB_OK as c_int
            }
            Some(_) => linkrs_error_code_t::GRAPHDB_MISMATCH as c_int,
            None => linkrs_error_code_t::GRAPHDB_NOTFOUND as c_int,
        },
        None => linkrs_error_code_t::GRAPHDB_NOTFOUND as c_int,
    }
}

/// Get floating point values (indexed by column)
///
/// # Arguments
/// - `result`: Result set handle
/// - `row`: Row index (starting from 0)
/// - `col`: Column index (starting from 0)
/// - `value`: Output parameter, float value
///
/// # Returns
/// - Success: GRAPHDB_OK
/// - Failure: Error code
///
/// # Safety
/// - `result` must be a valid result handle created by `linkrs_execute` or `linkrs_execute_params`
/// - `value` must be a valid pointer to store the result
/// - `row` must be a valid row index (0 <= row < row count)
/// - `col` must be a valid column index (0 <= col < column count)
#[no_mangle]
pub unsafe extern "C" fn linkrs_get_float_by_index(
    result: *mut linkrs_result_t,
    row: c_int,
    col: c_int,
    value: *mut f64,
) -> c_int {
    if result.is_null() || value.is_null() || col < 0 {
        return linkrs_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    let handle = &*(result as *mut GraphDbResultHandle);

    let columns = handle.inner.columns();
    let col_name = match columns.get(col as usize) {
        Some(name) => name.as_str(),
        None => return linkrs_error_code_t::GRAPHDB_NOTFOUND as c_int,
    };

    match handle.inner.get(row as usize) {
        Some(row_data) => match row_data.get(col_name) {
            Some(v) => match value_as_f64(v) {
                Some(f) => {
                    *value = f;
                    linkrs_error_code_t::GRAPHDB_OK as c_int
                }
                None => linkrs_error_code_t::GRAPHDB_MISMATCH as c_int,
            },
            None => linkrs_error_code_t::GRAPHDB_NOTFOUND as c_int,
        },
        None => linkrs_error_code_t::GRAPHDB_NOTFOUND as c_int,
    }
}

/// Get binary data (indexed by column)
///
/// # Arguments
/// - `result`: Result set handle
/// - `row`: Row index (starting from 0)
/// - `col`: Column index (starting from 0)
/// - `len`: Output parameter, data length (in bytes)
///
/// # Returns
/// - Data pointer, returns NULL on error
///
/// # Note
/// The returned pointer's lifetime is bound to the result set; the caller should not free it
///
/// # Safety
/// - `result` must be a valid result handle created by `linkrs_execute` or `linkrs_execute_params`
/// - `len` must be a valid pointer to store the data length, or NULL if not needed
/// - `row` must be a valid row index (0 <= row < row count)
/// - `col` must be a valid column index (0 <= col < column count)
/// - The returned pointer is only valid as long as the result set is not freed
#[no_mangle]
pub unsafe extern "C" fn linkrs_get_blob_by_index(
    result: *mut linkrs_result_t,
    row: c_int,
    col: c_int,
    len: *mut c_int,
) -> *const u8 {
    if result.is_null() || col < 0 {
        if !len.is_null() {
            *len = -1;
        }
        return ptr::null();
    }

    let handle = &*(result as *mut GraphDbResultHandle);

    let columns = handle.inner.columns();
    let col_name = match columns.get(col as usize) {
        Some(name) => name.as_str(),
        None => {
            if !len.is_null() {
                *len = -1;
            }
            return ptr::null();
        }
    };

    match handle.inner.get(row as usize) {
        Some(row_data) => match row_data.get(col_name) {
            Some(linkrs_core::Value::Blob(blob)) => {
                if !len.is_null() {
                    *len = blob.len() as c_int;
                }
                blob.as_ptr()
            }
            Some(_) => {
                if !len.is_null() {
                    *len = -1;
                }
                ptr::null()
            }
            None => {
                if !len.is_null() {
                    *len = -1;
                }
                ptr::null()
            }
        },
        None => {
            if !len.is_null() {
                *len = -1;
            }
            ptr::null()
        }
    }
}

/// Get column type
///
/// # Arguments
/// - `result`: Result set handle
/// - `col`: Column index (starting from 0)
///
/// # Returns
/// - Column type, returns GRAPHDB_NULL on error
///
/// # Safety
/// - `result` must be a valid result handle created by `linkrs_execute` or `linkrs_execute_params`
/// - `col` must be a valid column index (0 <= col < column count)
#[no_mangle]
pub unsafe extern "C" fn linkrs_column_type(
    result: *mut linkrs_result_t,
    col: c_int,
) -> crate::embedded::c_api::types::linkrs_value_type_t {
    use crate::embedded::c_api::types::linkrs_value_type_t;

    if result.is_null() || col < 0 {
        return linkrs_value_type_t::GRAPHDB_NULL;
    }

    let handle = &*(result as *mut GraphDbResultHandle);

    // Get the first line to determine the type
    match handle.inner.first() {
        Some(row) => {
            let columns = handle.inner.columns();
            let col_name = match columns.get(col as usize) {
                Some(name) => name.as_str(),
                None => return linkrs_value_type_t::GRAPHDB_NULL,
            };

            match row.get(col_name) {
                Some(value) => match value {
                    linkrs_core::Value::Null(_) => linkrs_value_type_t::GRAPHDB_NULL,
                    linkrs_core::Value::Bool(_) => linkrs_value_type_t::GRAPHDB_BOOL,
                    linkrs_core::Value::SmallInt(_)
                    | linkrs_core::Value::Int(_)
                    | linkrs_core::Value::BigInt(_) => linkrs_value_type_t::GRAPHDB_INT,
                    linkrs_core::Value::Float(_) | linkrs_core::Value::Double(_) => {
                        linkrs_value_type_t::GRAPHDB_FLOAT
                    }
                    linkrs_core::Value::String(_) | linkrs_core::Value::FixedString(_) => {
                        linkrs_value_type_t::GRAPHDB_STRING
                    }
                    linkrs_core::Value::Blob(_) => linkrs_value_type_t::GRAPHDB_BLOB,
                    linkrs_core::Value::List(_) => linkrs_value_type_t::GRAPHDB_LIST,
                    linkrs_core::Value::Map(_) => linkrs_value_type_t::GRAPHDB_MAP,
                    linkrs_core::Value::Vertex(_) => linkrs_value_type_t::GRAPHDB_VERTEX,
                    linkrs_core::Value::Edge(_) => linkrs_value_type_t::GRAPHDB_EDGE,
                    linkrs_core::Value::Path(_) => linkrs_value_type_t::GRAPHDB_PATH,
                    _ => linkrs_value_type_t::GRAPHDB_NULL,
                },
                None => linkrs_value_type_t::GRAPHDB_NULL,
            }
        }
        None => linkrs_value_type_t::GRAPHDB_NULL,
    }
}

/// Get the query execution time in milliseconds.
///
/// # Arguments
/// - `result`: Result set handle
/// - `out_ms`: Output parameter, execution time in milliseconds
///
/// # Returns
/// - Success: GRAPHDB_OK
/// - Failure: Error code
///
/// # Safety
/// - `result` must be a valid result handle created by `linkrs_execute` or `linkrs_execute_params`
/// - `out_ms` must be a valid pointer to store the result
#[no_mangle]
pub unsafe extern "C" fn linkrs_result_execution_time_ms(
    result: *mut linkrs_result_t,
    out_ms: *mut u64,
) -> c_int {
    if result.is_null() || out_ms.is_null() {
        return linkrs_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    let handle = &*(result as *mut GraphDbResultHandle);
    *out_ms = handle.inner.metadata().execution_time.as_millis() as u64;
    linkrs_error_code_t::GRAPHDB_OK as c_int
}

/// Get the number of rows scanned while producing the result set.
///
/// # Arguments
/// - `result`: Result set handle
/// - `out_rows`: Output parameter, scanned row count
///
/// # Returns
/// - Success: GRAPHDB_OK
/// - Failure: Error code
///
/// # Safety
/// - `result` must be a valid result handle created by `linkrs_execute` or `linkrs_execute_params`
/// - `out_rows` must be a valid pointer to store the result
#[no_mangle]
pub unsafe extern "C" fn linkrs_result_rows_scanned(
    result: *mut linkrs_result_t,
    out_rows: *mut u64,
) -> c_int {
    if result.is_null() || out_rows.is_null() {
        return linkrs_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    let handle = &*(result as *mut GraphDbResultHandle);
    *out_rows = handle.inner.metadata().rows_scanned;
    linkrs_error_code_t::GRAPHDB_OK as c_int
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_result_null_params() {
        let rc = unsafe { linkrs_result_free(ptr::null_mut()) };
        assert_eq!(rc, linkrs_error_code_t::GRAPHDB_MISUSE as c_int);

        let count = unsafe { linkrs_column_count(ptr::null_mut()) };
        assert_eq!(count, -1);

        let count = unsafe { linkrs_row_count(ptr::null_mut()) };
        assert_eq!(count, -1);

        let name = unsafe { linkrs_column_name(ptr::null_mut(), 0) };
        assert!(name.is_null());
    }
}
