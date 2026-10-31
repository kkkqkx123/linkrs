//! C API Database Management Module
//!
//! Provide database opening, closing and basic management functions

use crate::embedded::c_api::config::LinkrsConfigHandle;
use crate::embedded::c_api::error::{
    error_code_from_core_error, linkrs_error_code_t, set_last_error_message,
};
use crate::embedded::c_api::types::{linkrs_config_t, linkrs_t};
use crate::embedded::GraphDatabase;
use linkrs_storage::GraphStorage;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::ptr;
use std::sync::Arc;

/// Database handle internal structure
pub struct LinkrsHandle {
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
/// - Success: LINKRS_OK
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
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    // Converting path strings
    let path_str = unsafe {
        match CStr::from_ptr(path).to_str() {
            Ok(s) => s,
            Err(_) => return linkrs_error_code_t::LINKRS_MISUSE as c_int,
        }
    };

    // Open database
    match GraphDatabase::open(path_str) {
        Ok(linkrs) => {
            let handle = Box::new(LinkrsHandle {
                inner: Arc::new(linkrs),
                last_error: None,
            });
            unsafe {
                *db = Box::into_raw(handle) as *mut linkrs_t;
            }
            linkrs_error_code_t::LINKRS_OK as c_int
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
/// - Success: LINKRS_OK
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
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    let config_handle = &*(config as *mut LinkrsConfigHandle);
    match GraphDatabase::open_with_config(config_handle.inner.clone()) {
        Ok(linkrs) => {
            let handle = Box::new(LinkrsHandle {
                inner: Arc::new(linkrs),
                last_error: None,
            });
            unsafe {
                *db = Box::into_raw(handle) as *mut linkrs_t;
            }
            linkrs_error_code_t::LINKRS_OK as c_int
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
/// - Success: LINKRS_OK
/// - Failure: Error code
///
/// # Safety
/// - `db` must be a valid database handle created by `linkrs_open` or `linkrs_open_with_config`
/// - After calling this function, the database handle becomes invalid and must not be used
/// - All sessions associated with this database must be closed before calling this function
#[no_mangle]
pub unsafe extern "C" fn linkrs_close(db: *mut linkrs_t) -> c_int {
    if db.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    unsafe {
        // Converts the original pointer back to a Box, which is automatically released at the end of the function.
        let _ = Box::from_raw(db as *mut LinkrsHandle);
    }

    linkrs_error_code_t::LINKRS_OK as c_int
}

/// Get Error Code
///
/// # Arguments
/// - `db`: Database handle
///
/// # Returns
/// - Error code, returns LINKRS_OK if no error
///
/// # Safety
/// - `db` must be a valid database handle created by `linkrs_open` or `linkrs_open_with_config`
#[no_mangle]
pub unsafe extern "C" fn linkrs_errcode(db: *mut linkrs_t) -> c_int {
    if db.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    unsafe {
        let handle = &*(db as *mut LinkrsHandle);
        if handle.last_error.is_some() {
            linkrs_error_code_t::LINKRS_ERROR as c_int
        } else {
            linkrs_error_code_t::LINKRS_OK as c_int
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

/// Release strings (strings allocated by Linkrs)
///
/// # Arguments
/// - `str`: String pointer
///
/// # Safety
/// - `str` must be a valid pointer to a string allocated by Linkrs
/// - After calling this function, the pointer becomes invalid and must not be used
/// - This function should only be called on strings that were allocated by Linkrs C API functions
#[no_mangle]
pub unsafe extern "C" fn linkrs_free_string(str: *mut c_char) {
    if !str.is_null() {
        unsafe {
            let _ = CString::from_raw(str);
        }
    }
}

/// Abort another transaction by id (`GraphDatabase::kill_transaction` over FFI).
///
/// # Safety
/// - `db` must be a valid database handle
#[no_mangle]
pub unsafe extern "C" fn linkrs_kill_transaction(db: *mut linkrs_t, txn_id: u64) -> c_int {
    if db.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }
    unsafe {
        let handle = &*(db as *mut LinkrsHandle);
        match handle.inner.kill_transaction(txn_id) {
            Ok(()) => linkrs_error_code_t::LINKRS_OK as c_int,
            Err(e) => {
                let (error_code, _) = error_code_from_core_error(&e);
                set_last_error_message(format!("{e}"));
                error_code
            }
        }
    }
}

/// Number of currently active transactions (`list_transactions().len()`).
///
/// # Safety
/// - `db` must be a valid database handle
#[no_mangle]
pub unsafe extern "C" fn linkrs_transaction_count(db: *mut linkrs_t) -> c_int {
    if db.is_null() {
        return -1;
    }
    unsafe {
        let handle = &*(db as *mut LinkrsHandle);
        handle.inner.list_transactions().len() as c_int
    }
}

/// Database overview (`Database::database_overview` over FFI).
///
/// Returns a heap JSON object combining the space count, live transaction
/// metrics, shared query counters, and the in-memory flag, mirroring the
/// embedded `database_overview` result.
///
/// # Arguments
/// - `db`: Database handle
///
/// # Returns
/// - JSON object string (UTF-8 encoded), returns NULL on error
///
/// # Memory Management
/// The returned string is dynamically allocated and must be freed by the
/// caller using `linkrs_free_string` to avoid memory leaks.
///
/// # Safety
/// - `db` must be a valid database handle created by `linkrs_open` or `linkrs_open_with_config`
/// - The returned pointer must be freed by the caller to avoid memory leaks
#[no_mangle]
pub unsafe extern "C" fn linkrs_database_overview(db: *mut linkrs_t) -> *mut c_char {
    if db.is_null() {
        return ptr::null_mut();
    }
    unsafe {
        let handle = &*(db as *mut LinkrsHandle);
        let overview = handle.inner.database_overview();
        match serde_json::to_string(&overview) {
            Ok(json) => match CString::new(json) {
                Ok(c_json) => c_json.into_raw(),
                Err(_) => ptr::null_mut(),
            },
            Err(e) => {
                set_last_error_message(format!("failed to serialize database overview: {e}"));
                ptr::null_mut()
            }
        }
    }
}

/// Execute `query` and write the result to `path`
/// (`Database::export_query_to_file` over FFI).
///
/// # Arguments
/// - `db`: Database handle
/// - `query`: Query statement (UTF-8 encoded)
/// - `path`: Output file path (UTF-8 encoded)
/// - `format`: Export format: 0 = CSV, 1 = JSONL
/// - `out_rows`: Output parameter, number of rows written (excluding the
///   CSV header row)
///
/// # Returns
/// - Success: LINKRS_OK
/// - Failure: Error code (an out-of-range `format` fails with LINKRS_MISUSE)
///
/// # Safety
/// - `db` must be a valid database handle created by `linkrs_open` or `linkrs_open_with_config`
/// - `query` and `path` must be valid pointers to null-terminated UTF-8 strings
/// - `out_rows` must be a valid pointer to store the result
#[no_mangle]
pub unsafe extern "C" fn linkrs_export_to_file(
    db: *mut linkrs_t,
    query: *const c_char,
    path: *const c_char,
    format: c_int,
    out_rows: *mut u64,
) -> c_int {
    if db.is_null() || query.is_null() || path.is_null() || out_rows.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    // Integer-to-enum marshalling mirrors `linkrs_config_set_sync_mode`.
    let export_format = match format {
        0 => crate::embedded::session::export::ExportFormat::Csv,
        1 => crate::embedded::session::export::ExportFormat::Jsonl,
        _ => return linkrs_error_code_t::LINKRS_MISUSE as c_int,
    };

    let query_str = unsafe {
        match CStr::from_ptr(query).to_str() {
            Ok(s) => s,
            Err(_) => return linkrs_error_code_t::LINKRS_MISUSE as c_int,
        }
    };
    let path_str = unsafe {
        match CStr::from_ptr(path).to_str() {
            Ok(s) => s,
            Err(_) => return linkrs_error_code_t::LINKRS_MISUSE as c_int,
        }
    };

    unsafe {
        let handle = &*(db as *mut LinkrsHandle);
        match handle
            .inner
            .export_query_to_file(query_str, path_str, export_format)
        {
            Ok(rows) => {
                *out_rows = rows as u64;
                linkrs_error_code_t::LINKRS_OK as c_int
            }
            Err(e) => {
                let (error_code, _) = error_code_from_core_error(&e);
                set_last_error_message(format!("{e}"));
                error_code
            }
        }
    }
}

/// Freeing memory (memory allocated by Linkrs)
///
/// # Arguments
/// - `ptr`: Memory pointer
///
/// # Safety
/// - `ptr` must be a valid pointer to memory allocated by Linkrs
/// - After calling this function, the pointer becomes invalid and must not be used
/// - This function should only be called on memory that was allocated by Linkrs C API functions
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
        if rc != linkrs_error_code_t::LINKRS_OK as c_int {
            panic!(
                "Failed to open database, error code: {}, path: {:?}",
                rc, db_path
            );
        }
        assert!(!db.is_null());

        let rc = unsafe { linkrs_close(db) };
        assert_eq!(rc, linkrs_error_code_t::LINKRS_OK as c_int);
    }

    #[test]
    fn test_linkrs_null_params() {
        let rc = unsafe { linkrs_open(ptr::null(), ptr::null_mut()) };
        assert_eq!(rc, linkrs_error_code_t::LINKRS_MISUSE as c_int);

        let rc = unsafe { linkrs_close(ptr::null_mut()) };
        assert_eq!(rc, linkrs_error_code_t::LINKRS_MISUSE as c_int);
    }

    fn open_memory_db() -> *mut linkrs_t {
        use crate::embedded::c_api::config::{linkrs_config_free, linkrs_config_memory};

        unsafe {
            let config = linkrs_config_memory();
            let mut db: *mut linkrs_t = ptr::null_mut();
            assert_eq!(linkrs_open_with_config(config, &mut db), 0);
            assert!(!db.is_null());
            assert_eq!(linkrs_config_free(config), 0);
            db
        }
    }

    #[test]
    fn test_database_overview() {
        unsafe {
            let db = open_memory_db();

            let json_ptr = linkrs_database_overview(db);
            assert!(!json_ptr.is_null());
            let json_str = CStr::from_ptr(json_ptr)
                .to_str()
                .expect("overview JSON is not valid UTF-8");
            let overview: serde_json::Value =
                serde_json::from_str(json_str).expect("overview JSON does not parse");
            linkrs_free_string(json_ptr);

            assert!(overview.get("spaces").is_some());
            let transactions = overview
                .get("transactions")
                .expect("overview has no transactions section");
            assert!(transactions.get("begun").is_some());
            assert!(transactions.get("active").is_some());
            assert_eq!(
                overview.get("in_memory"),
                Some(&serde_json::Value::Bool(true))
            );

            assert!(linkrs_database_overview(ptr::null_mut()).is_null());

            assert_eq!(linkrs_close(db), linkrs_error_code_t::LINKRS_OK as c_int);
        }
    }

    #[test]
    fn test_export_to_file() {
        let query = CString::new("RETURN 1").expect("query CString");
        let counter = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let temp_dir = std::env::temp_dir().join("linkrs_c_api_test");
        std::fs::create_dir_all(&temp_dir).ok();

        let csv_path = temp_dir.join(format!("export_{}_{}.csv", std::process::id(), counter));
        let jsonl_path = temp_dir.join(format!("export_{}_{}.jsonl", std::process::id(), counter));
        let csv_c =
            CString::new(csv_path.to_str().expect("invalid csv path")).expect("csv path CString");
        let jsonl_c = CString::new(jsonl_path.to_str().expect("invalid jsonl path"))
            .expect("jsonl path CString");

        unsafe {
            let db = open_memory_db();

            let mut rows: u64 = 0;
            let rc = linkrs_export_to_file(db, query.as_ptr(), csv_c.as_ptr(), 0, &mut rows);
            assert_eq!(rc, linkrs_error_code_t::LINKRS_OK as c_int);
            assert_eq!(rows, 1);
            let csv_content =
                std::fs::read_to_string(&csv_path).expect("csv export file was not written");
            assert!(!csv_content.trim().is_empty());

            rows = 0;
            let rc = linkrs_export_to_file(db, query.as_ptr(), jsonl_c.as_ptr(), 1, &mut rows);
            assert_eq!(rc, linkrs_error_code_t::LINKRS_OK as c_int);
            assert_eq!(rows, 1);
            let jsonl_content =
                std::fs::read_to_string(&jsonl_path).expect("jsonl export file was not written");
            assert!(!jsonl_content.trim().is_empty());

            let rc = linkrs_export_to_file(db, query.as_ptr(), csv_c.as_ptr(), 7, &mut rows);
            assert_eq!(rc, linkrs_error_code_t::LINKRS_MISUSE as c_int);

            let rc = linkrs_export_to_file(
                ptr::null_mut(),
                query.as_ptr(),
                csv_c.as_ptr(),
                0,
                &mut rows,
            );
            assert_eq!(rc, linkrs_error_code_t::LINKRS_MISUSE as c_int);
            let rc = linkrs_export_to_file(db, ptr::null(), csv_c.as_ptr(), 0, &mut rows);
            assert_eq!(rc, linkrs_error_code_t::LINKRS_MISUSE as c_int);
            let rc = linkrs_export_to_file(db, query.as_ptr(), csv_c.as_ptr(), 0, ptr::null_mut());
            assert_eq!(rc, linkrs_error_code_t::LINKRS_MISUSE as c_int);

            assert_eq!(linkrs_close(db), linkrs_error_code_t::LINKRS_OK as c_int);
        }

        std::fs::remove_file(&csv_path).ok();
        std::fs::remove_file(&jsonl_path).ok();
    }
}
