//! C API Results Processing Module
//!
//! Provide processing functions for query results

use crate::api::embedded::c_api::error::graphdb_error_code_t;
use crate::api::embedded::c_api::types::graphdb_result_t;
use crate::api::embedded::result::QueryResult;
use std::ffi::{c_char, c_int, CStr, CString};
use std::ptr;

/// Internal structure of result set handles
pub struct GraphDbResultHandle {
    pub(crate) inner: QueryResult,
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
/// - `result` must be a valid result handle created by `graphdb_execute` or `graphdb_execute_params`
/// - After calling this function, the result handle becomes invalid and must not be used
/// - Any string pointers obtained from this result set become invalid after this call
#[no_mangle]
pub unsafe extern "C" fn graphdb_result_free(result: *mut graphdb_result_t) -> c_int {
    if result.is_null() {
        return graphdb_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    let _ = Box::from_raw(result as *mut GraphDbResultHandle);

    graphdb_error_code_t::GRAPHDB_OK as c_int
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
/// - `result` must be a valid result handle created by `graphdb_execute` or `graphdb_execute_params`
#[no_mangle]
pub unsafe extern "C" fn graphdb_column_count(result: *mut graphdb_result_t) -> c_int {
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
/// - `result` must be a valid result handle created by `graphdb_execute` or `graphdb_execute_params`
#[no_mangle]
pub unsafe extern "C" fn graphdb_row_count(result: *mut graphdb_result_t) -> c_int {
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
/// The returned string is dynamically allocated and must be freed by the caller using `graphdb_free_string`
/// to avoid memory leaks.
///
/// # Safety
/// - `result` must be a valid result handle created by `graphdb_execute` or `graphdb_execute_params`
/// - `index` must be a valid column index (0 <= index < column count)
/// - The returned pointer must be freed by the caller to avoid memory leaks
#[no_mangle]
pub unsafe extern "C" fn graphdb_column_name(
    result: *mut graphdb_result_t,
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
/// - `result` must be a valid result handle created by `graphdb_execute` or `graphdb_execute_params`
/// - `col` must be a valid pointer to a null-terminated UTF-8 string
/// - `value` must be a valid pointer to store the result
/// - `row` must be a valid row index (0 <= row < row count)
#[no_mangle]
pub unsafe extern "C" fn graphdb_get_int(
    result: *mut graphdb_result_t,
    row: c_int,
    col: *const c_char,
    value: *mut i64,
) -> c_int {
    if result.is_null() || col.is_null() || value.is_null() {
        return graphdb_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    let col_str = match CStr::from_ptr(col).to_str() {
        Ok(s) => s,
        Err(_) => return graphdb_error_code_t::GRAPHDB_MISUSE as c_int,
    };

    let handle = &*(result as *mut GraphDbResultHandle);

    match handle.inner.get(row as usize) {
        Some(row_data) => match row_data.get(col_str) {
            Some(crate::core::Value::Int(i)) => {
                *value = *i as i64;
                graphdb_error_code_t::GRAPHDB_OK as c_int
            }
            Some(_) => graphdb_error_code_t::GRAPHDB_MISMATCH as c_int,
            None => graphdb_error_code_t::GRAPHDB_NOTFOUND as c_int,
        },
        None => graphdb_error_code_t::GRAPHDB_NOTFOUND as c_int,
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
/// The returned string is dynamically allocated and must be freed by the caller using `graphdb_free_string`
/// to avoid memory leaks.
///
/// # Safety
/// - `result` must be a valid result handle created by `graphdb_execute` or `graphdb_execute_params`
/// - `col` must be a valid pointer to a null-terminated UTF-8 string
/// - `len` must be a valid pointer to store the string length, or NULL if not needed
/// - `row` must be a valid row index (0 <= row < row count)
/// - The returned pointer must be freed by the caller to avoid memory leaks
#[no_mangle]
pub unsafe extern "C" fn graphdb_get_string(
    result: *mut graphdb_result_t,
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
            Some(crate::core::Value::String(s)) => {
                if !len.is_null() {
                    *len = s.len() as c_int;
                }
                match CString::new(s.as_str()) {
                    Ok(c_str) => c_str.into_raw(),
                    Err(_) => ptr::null_mut(),
                }
            }
            Some(_) => {
                if !len.is_null() {
                    *len = -1;
                }
                ptr::null_mut()
            }
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
/// - `result` must be a valid result handle created by `graphdb_execute` or `graphdb_execute_params`
/// - `col` must be a valid pointer to a null-terminated UTF-8 string
/// - `len` must be a valid pointer to store the data length, or NULL if not needed
/// - `row` must be a valid row index (0 <= row < row count)
/// - The returned pointer is only valid as long as the result set is not freed
#[no_mangle]
pub unsafe extern "C" fn graphdb_get_blob(
    result: *mut graphdb_result_t,
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
            Some(crate::core::Value::Blob(blob)) => {
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
/// - `result` must be a valid result handle created by `graphdb_execute` or `graphdb_execute_params`
/// - `value` must be a valid pointer to store the result
/// - `row` must be a valid row index (0 <= row < row count)
/// - `col` must be a valid column index (0 <= col < column count)
#[no_mangle]
pub unsafe extern "C" fn graphdb_get_int_by_index(
    result: *mut graphdb_result_t,
    row: c_int,
    col: c_int,
    value: *mut i64,
) -> c_int {
    if result.is_null() || value.is_null() || col < 0 {
        return graphdb_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    let handle = &*(result as *mut GraphDbResultHandle);

    // Getting Column Names
    let columns = handle.inner.columns();
    let col_name = match columns.get(col as usize) {
        Some(name) => name.as_str(),
        None => return graphdb_error_code_t::GRAPHDB_NOTFOUND as c_int,
    };

    match handle.inner.get(row as usize) {
        Some(row_data) => match row_data.get(col_name) {
            Some(crate::core::Value::Int(i)) => {
                *value = *i as i64;
                graphdb_error_code_t::GRAPHDB_OK as c_int
            }
            Some(_) => graphdb_error_code_t::GRAPHDB_MISMATCH as c_int,
            None => graphdb_error_code_t::GRAPHDB_NOTFOUND as c_int,
        },
        None => graphdb_error_code_t::GRAPHDB_NOTFOUND as c_int,
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
/// The returned string is dynamically allocated and must be freed by the caller using `graphdb_free_string`
/// to avoid memory leaks.
///
/// # Safety
/// - `result` must be a valid result handle created by `graphdb_execute` or `graphdb_execute_params`
/// - `len` must be a valid pointer to store the string length, or NULL if not needed
/// - `row` must be a valid row index (0 <= row < row count)
/// - `col` must be a valid column index (0 <= col < column count)
/// - The returned pointer must be freed by the caller to avoid memory leaks
#[no_mangle]
pub unsafe extern "C" fn graphdb_get_string_by_index(
    result: *mut graphdb_result_t,
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
            Some(crate::core::Value::String(s)) => {
                if !len.is_null() {
                    *len = s.len() as c_int;
                }
                match CString::new(s.as_str()) {
                    Ok(c_str) => c_str.into_raw(),
                    Err(_) => ptr::null_mut(),
                }
            }
            Some(_) => {
                if !len.is_null() {
                    *len = -1;
                }
                ptr::null_mut()
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
/// - `result` must be a valid result handle created by `graphdb_execute` or `graphdb_execute_params`
/// - `value` must be a valid pointer to store the result
/// - `row` must be a valid row index (0 <= row < row count)
/// - `col` must be a valid column index (0 <= col < column count)
#[no_mangle]
pub unsafe extern "C" fn graphdb_get_bool_by_index(
    result: *mut graphdb_result_t,
    row: c_int,
    col: c_int,
    value: *mut bool,
) -> c_int {
    if result.is_null() || value.is_null() || col < 0 {
        return graphdb_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    let handle = &*(result as *mut GraphDbResultHandle);

    let columns = handle.inner.columns();
    let col_name = match columns.get(col as usize) {
        Some(name) => name.as_str(),
        None => return graphdb_error_code_t::GRAPHDB_NOTFOUND as c_int,
    };

    match handle.inner.get(row as usize) {
        Some(row_data) => match row_data.get(col_name) {
            Some(crate::core::Value::Bool(b)) => {
                *value = *b;
                graphdb_error_code_t::GRAPHDB_OK as c_int
            }
            Some(_) => graphdb_error_code_t::GRAPHDB_MISMATCH as c_int,
            None => graphdb_error_code_t::GRAPHDB_NOTFOUND as c_int,
        },
        None => graphdb_error_code_t::GRAPHDB_NOTFOUND as c_int,
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
/// - `result` must be a valid result handle created by `graphdb_execute` or `graphdb_execute_params`
/// - `value` must be a valid pointer to store the result
/// - `row` must be a valid row index (0 <= row < row count)
/// - `col` must be a valid column index (0 <= col < column count)
#[no_mangle]
pub unsafe extern "C" fn graphdb_get_float_by_index(
    result: *mut graphdb_result_t,
    row: c_int,
    col: c_int,
    value: *mut f64,
) -> c_int {
    if result.is_null() || value.is_null() || col < 0 {
        return graphdb_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    let handle = &*(result as *mut GraphDbResultHandle);

    let columns = handle.inner.columns();
    let col_name = match columns.get(col as usize) {
        Some(name) => name.as_str(),
        None => return graphdb_error_code_t::GRAPHDB_NOTFOUND as c_int,
    };

    match handle.inner.get(row as usize) {
        Some(row_data) => match row_data.get(col_name) {
            Some(crate::core::Value::Float(f)) => {
                *value = *f as f64;
                graphdb_error_code_t::GRAPHDB_OK as c_int
            }
            Some(_) => graphdb_error_code_t::GRAPHDB_MISMATCH as c_int,
            None => graphdb_error_code_t::GRAPHDB_NOTFOUND as c_int,
        },
        None => graphdb_error_code_t::GRAPHDB_NOTFOUND as c_int,
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
/// - `result` must be a valid result handle created by `graphdb_execute` or `graphdb_execute_params`
/// - `len` must be a valid pointer to store the data length, or NULL if not needed
/// - `row` must be a valid row index (0 <= row < row count)
/// - `col` must be a valid column index (0 <= col < column count)
/// - The returned pointer is only valid as long as the result set is not freed
#[no_mangle]
pub unsafe extern "C" fn graphdb_get_blob_by_index(
    result: *mut graphdb_result_t,
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
            Some(crate::core::Value::Blob(blob)) => {
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
/// - `result` must be a valid result handle created by `graphdb_execute` or `graphdb_execute_params`
/// - `col` must be a valid column index (0 <= col < column count)
#[no_mangle]
pub unsafe extern "C" fn graphdb_column_type(
    result: *mut graphdb_result_t,
    col: c_int,
) -> crate::api::embedded::c_api::types::graphdb_value_type_t {
    use crate::api::embedded::c_api::types::graphdb_value_type_t;

    if result.is_null() || col < 0 {
        return graphdb_value_type_t::GRAPHDB_NULL;
    }

    let handle = &*(result as *mut GraphDbResultHandle);

    // Get the first line to determine the type
    match handle.inner.first() {
        Some(row) => {
            let columns = handle.inner.columns();
            let col_name = match columns.get(col as usize) {
                Some(name) => name.as_str(),
                None => return graphdb_value_type_t::GRAPHDB_NULL,
            };

            match row.get(col_name) {
                Some(value) => match value {
                    crate::core::Value::Null(_) => graphdb_value_type_t::GRAPHDB_NULL,
                    crate::core::Value::Bool(_) => graphdb_value_type_t::GRAPHDB_BOOL,
                    crate::core::Value::Int(_) => graphdb_value_type_t::GRAPHDB_INT,
                    crate::core::Value::Float(_) => graphdb_value_type_t::GRAPHDB_FLOAT,
                    crate::core::Value::String(_) => graphdb_value_type_t::GRAPHDB_STRING,
                    crate::core::Value::Blob(_) => graphdb_value_type_t::GRAPHDB_BLOB,
                    crate::core::Value::List(_) => graphdb_value_type_t::GRAPHDB_LIST,
                    crate::core::Value::Map(_) => graphdb_value_type_t::GRAPHDB_MAP,
                    crate::core::Value::Vertex(_) => graphdb_value_type_t::GRAPHDB_VERTEX,
                    crate::core::Value::Edge(_) => graphdb_value_type_t::GRAPHDB_EDGE,
                    crate::core::Value::Path(_) => graphdb_value_type_t::GRAPHDB_PATH,
                    _ => graphdb_value_type_t::GRAPHDB_NULL,
                },
                None => graphdb_value_type_t::GRAPHDB_NULL,
            }
        }
        None => graphdb_value_type_t::GRAPHDB_NULL,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_result_null_params() {
        let rc = unsafe { graphdb_result_free(ptr::null_mut()) };
        assert_eq!(rc, graphdb_error_code_t::GRAPHDB_MISUSE as c_int);

        let count = unsafe { graphdb_column_count(ptr::null_mut()) };
        assert_eq!(count, -1);

        let count = unsafe { graphdb_row_count(ptr::null_mut()) };
        assert_eq!(count, -1);

        let name = unsafe { graphdb_column_name(ptr::null_mut(), 0) };
        assert!(name.is_null());
    }
}
