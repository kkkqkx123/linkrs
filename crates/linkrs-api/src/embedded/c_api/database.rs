//! C API Database Management Module
//!
//! Provide database opening, closing and basic management functions

use crate::embedded::c_api::config::GraphDbConfigHandle;
use crate::embedded::c_api::error::{
    error_code_from_core_error, linkrs_error_code_t, set_last_error_message,
};
use crate::embedded::c_api::types::{linkrs_config_t, linkrs_t};
use crate::embedded::{DatabaseConfig, GraphDatabase};
use crate::storage::GraphStorage;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::ptr;
use std::sync::Arc;

/// Database handle internal structure
pub struct GraphDbHandle {
    pub(crate) inner: Arc<GraphDatabase<GraphStorage>>,
    pub(crate) last_error: Option<CString>,
}

/// Open database
///
/// # Arguments
/// - `path`: Database file path (UTF-8 encoded)
/// - `db`: Output parameter, database handle
///
/// # Returns
/// - Success: GRAPHDB_OK
/// - Failure: Error code
///
/// # Safety
/// - `path` must be a valid pointer to a null-terminated UTF-8 string
/// - `db` must be a valid pointer to store the database handle
/// - The caller is responsible for closing the database using `linkrs_close` when done
/// - The database handle must not be used after closing
#[no_mangle]
pub unsafe extern "C" fn linkrs_open(path: *const c_char, db: *mut *mut linkrs_t) -> c_int {
    // parameter verification
    if path.is_null() || db.is_null() {
        return linkrs_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    // Converting path strings
    let path_str = unsafe {
        match CStr::from_ptr(path).to_str() {
            Ok(s) => s,
            Err(_) => return linkrs_error_code_t::GRAPHDB_MISUSE as c_int,
        }
    };

    // Open database
    match GraphDatabase::open(path_str) {
        Ok(linkrs) => {
            let handle = Box::new(GraphDbHandle {
                inner: Arc::new(linkrs),
                last_error: None,
            });
            unsafe {
                *db = Box::into_raw(handle) as *mut linkrs_t;
            }
            linkrs_error_code_t::GRAPHDB_OK as c_int
        }
        Err(e) => {
            let (error_code, _) = error_code_from_core_error(&e);
            let error_msg = format!("{}", e);
            set_last_error_message(error_msg);
            unsafe {
                *db = ptr::null_mut();
            }
            error_code
        }
    }
}

/// Open the database with a configuration handle.
///
/// The configuration handle stays valid after the call: it can be reused
/// and must still be released with `linkrs_config_free`.
///
/// # Arguments
/// - `config`: Configuration handle from `linkrs_config_new/_file/_memory`
/// - `db`: Output parameter, database handle
///
/// # Returns
/// - Success: GRAPHDB_OK
/// - Failure: Error code
///
/// # Safety
/// - `config` must be a valid configuration handle
/// - `db` must be a valid pointer to store the database handle
/// - The caller is responsible for closing the database using `linkrs_close` when done
#[no_mangle]
pub unsafe extern "C" fn linkrs_open_with_config(
    config: *mut linkrs_config_t,
    db: *mut *mut linkrs_t,
) -> c_int {
    if config.is_null() || db.is_null() {
        return linkrs_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    let config_handle = &*(config as *mut GraphDbConfigHandle);
    match GraphDatabase::open_with_config(config_handle.inner.clone()) {
        Ok(linkrs) => {
            let handle = Box::new(GraphDbHandle {
                inner: Arc::new(linkrs),
                last_error: None,
            });
            unsafe {
                *db = Box::into_raw(handle) as *mut linkrs_t;
            }
            linkrs_error_code_t::GRAPHDB_OK as c_int
        }
        Err(e) => {
            let (error_code, _) = error_code_from_core_error(&e);
            let error_msg = format!("{}", e);
            set_last_error_message(error_msg);
            unsafe {
                *db = ptr::null_mut();
            }
            error_code
        }
    }
}

/// Closing the database
///
/// # Arguments
/// - `db`: Database handle
///
/// # Returns
/// - Success: GRAPHDB_OK
/// - Failure: Error code
///
/// # Safety
/// - `db` must be a valid database handle created by `linkrs_open` or `linkrs_open_with_config`
/// - After calling this function, the database handle becomes invalid and must not be used
/// - All sessions associated with this database must be closed before calling this function
#[no_mangle]
pub unsafe extern "C" fn linkrs_close(db: *mut linkrs_t) -> c_int {
    if db.is_null() {
        return linkrs_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    unsafe {
        // Converts the original pointer back to a Box, which is automatically released at the end of the function.
        let _ = Box::from_raw(db as *mut GraphDbHandle);
    }

    linkrs_error_code_t::GRAPHDB_OK as c_int
}

/// Get Error Code
///
/// # Arguments
/// - `db`: Database handle
///
/// # Returns
/// - Error code, returns GRAPHDB_OK if no error
///
/// # Safety
/// - `db` must be a valid database handle created by `linkrs_open` or `linkrs_open_with_config`
#[no_mangle]
pub unsafe extern "C" fn linkrs_errcode(db: *mut linkrs_t) -> c_int {
    if db.is_null() {
        return linkrs_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    unsafe {
        let handle = &*(db as *mut GraphDbHandle);
        if handle.last_error.is_some() {
            linkrs_error_code_t::GRAPHDB_ERROR as c_int
        } else {
            linkrs_error_code_t::GRAPHDB_OK as c_int
        }
    }
}

/// Getting the library version
///
/// # Back
/// - revision string (computing)
#[no_mangle]
pub extern "C" fn linkrs_libversion() -> *const c_char {
    static VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), "\0");
    VERSION.as_ptr() as *const c_char
}

/// Release strings (strings allocated by GraphDB)
///
/// # Arguments
/// - `str`: String pointer
///
/// # Safety
/// - `str` must be a valid pointer to a string allocated by GraphDB
/// - After calling this function, the pointer becomes invalid and must not be used
/// - This function should only be called on strings that were allocated by GraphDB C API functions
#[no_mangle]
pub unsafe extern "C" fn linkrs_free_string(str: *mut c_char) {
    if !str.is_null() {
        unsafe {
            let _ = CString::from_raw(str);
        }
    }
}

/// Freeing memory (memory allocated by GraphDB)
///
/// # Arguments
/// - `ptr`: Memory pointer
///
/// # Safety
/// - `ptr` must be a valid pointer to memory allocated by GraphDB
/// - After calling this function, the pointer becomes invalid and must not be used
/// - This function should only be called on memory that was allocated by GraphDB C API functions
#[no_mangle]
pub unsafe extern "C" fn linkrs_free(ptr: *mut c_void) {
    if !ptr.is_null() {
        unsafe {
            let _ = Box::from_raw(ptr as *mut u8);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn get_test_db_path() -> std::path::PathBuf {
        let counter = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let temp_dir = std::env::temp_dir().join("linkrs_c_api_test");
        std::fs::create_dir_all(&temp_dir).ok();
        let db_path = temp_dir.join(format!("test_db_{}_{}.db", std::process::id(), counter));

        // Ensure that the database file does not exist
        if db_path.exists() {
            std::fs::remove_file(&db_path).ok();
            // Wait for the file system to complete the deletion operation
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        db_path
    }

    #[test]
    fn test_linkrs_libversion() {
        let version = unsafe {
            CStr::from_ptr(linkrs_libversion())
                .to_str()
                .expect("Failed to convert version to str")
        };
        assert!(!version.is_empty());
    }

    #[test]
    fn test_linkrs_open_close_file() {
        let db_path = get_test_db_path();

        let path_cstring = CString::new(db_path.to_str().expect("Invalid path"))
            .expect("Failed to create CString");
        let mut db: *mut linkrs_t = ptr::null_mut();

        let rc = unsafe { linkrs_open(path_cstring.as_ptr(), &mut db) };
        if rc != linkrs_error_code_t::GRAPHDB_OK as c_int {
            panic!(
                "Failed to open database, error code: {}, path: {:?}",
                rc, db_path
            );
        }
        assert!(!db.is_null());

        let rc = unsafe { linkrs_close(db) };
        assert_eq!(rc, linkrs_error_code_t::GRAPHDB_OK as c_int);
    }

    #[test]
    fn test_linkrs_null_params() {
        let rc = unsafe { linkrs_open(ptr::null(), ptr::null_mut()) };
        assert_eq!(rc, linkrs_error_code_t::GRAPHDB_MISUSE as c_int);

        let rc = unsafe { linkrs_close(ptr::null_mut()) };
        assert_eq!(rc, linkrs_error_code_t::GRAPHDB_MISUSE as c_int);
    }
}
