//! C API Configuration Management Module
//!
//! Provides configuration management functions for database opening

use crate::embedded::c_api::database::LinkrsHandle;
use crate::embedded::c_api::error::{
    error_code_from_core_error, linkrs_error_code_t, set_last_error_message,
};
use crate::embedded::c_api::types::{linkrs_config_t, linkrs_t};
use crate::embedded::{DatabaseConfig, SyncMode};
use std::ffi::{c_char, c_int, CStr, CString};
use std::time::Duration;

/// Internal structure of configuration handles
pub struct LinkrsConfigHandle {
    pub(crate) inner: DatabaseConfig,
}

impl LinkrsConfigHandle {
    pub fn new(inner: DatabaseConfig) -> Self {
        Self { inner }
    }
}

/// Create a new configuration (default configuration)
///
/// # Returns
/// - Configuration handle
///
/// # Memory Management
/// The returned configuration must be freed using `linkrs_config_free` when done
///
/// # Safety
/// This function uses FFI and returns a raw pointer. The returned pointer must be freed
/// using `linkrs_config_free` to avoid memory leaks.
#[no_mangle]
pub unsafe extern "C" fn linkrs_config_new() -> *mut linkrs_config_t {
    let config = DatabaseConfig::memory();
    let handle = Box::new(LinkrsConfigHandle::new(config));
    Box::into_raw(handle) as *mut linkrs_config_t
}

/// Create a file database configuration
///
/// # Arguments
/// - `path`: Database file path (UTF-8 encoded)
///
/// # Returns
/// - Configuration handle
///
/// # Safety
/// - `path` must be a valid pointer to a null-terminated UTF-8 string
/// - The returned configuration must be freed using `linkrs_config_free` when done
#[no_mangle]
pub unsafe extern "C" fn linkrs_config_file(path: *const c_char) -> *mut linkrs_config_t {
    if path.is_null() {
        return std::ptr::null_mut();
    }

    let path_str = match CStr::from_ptr(path).to_str() {
        Ok(s) => s,
        Err(_) => return std::ptr::null_mut(),
    };

    let config = DatabaseConfig::file(path_str);
    let handle = Box::new(LinkrsConfigHandle::new(config));
    Box::into_raw(handle) as *mut linkrs_config_t
}

/// Create an in-memory database configuration
///
/// # Returns
/// - Configuration handle
///
/// # Memory Management
/// The returned configuration must be freed using `linkrs_config_free` when done
///
/// # Safety
/// This function uses FFI and returns a raw pointer. The returned pointer must be freed
/// using `linkrs_config_free` to avoid memory leaks.
#[no_mangle]
pub unsafe extern "C" fn linkrs_config_memory() -> *mut linkrs_config_t {
    let config = DatabaseConfig::memory();
    let handle = Box::new(LinkrsConfigHandle::new(config));
    Box::into_raw(handle) as *mut linkrs_config_t
}

/// Free configuration handle
///
/// # Arguments
/// - `config`: Configuration handle
///
/// # Returns
/// - Success: LINKRS_OK
/// - Failure: Error code
///
/// # Safety
/// - `config` must be a valid configuration handle created by linkrs_config_new,
///   linkrs_config_file, or linkrs_config_memory
#[no_mangle]
pub unsafe extern "C" fn linkrs_config_free(config: *mut linkrs_config_t) -> c_int {
    if config.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    let _ = Box::from_raw(config as *mut LinkrsConfigHandle);
    linkrs_error_code_t::LINKRS_OK as c_int
}

/// Set cache size
///
/// # Arguments
/// - `config`: Configuration handle
/// - `size_mb`: Cache size in MB
///
/// # Returns
/// - Success: LINKRS_OK
/// - Failure: Error code
///
/// # Safety
/// - `config` must be a valid configuration handle
#[no_mangle]
pub unsafe extern "C" fn linkrs_config_set_cache_size(
    config: *mut linkrs_config_t,
    size_mb: c_int,
) -> c_int {
    if config.is_null() || size_mb <= 0 {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    let handle = &mut *(config as *mut LinkrsConfigHandle);
    handle.inner = handle.inner.clone().with_cache_size(size_mb as usize);
    linkrs_error_code_t::LINKRS_OK as c_int
}

/// Set timeout
///
/// # Arguments
/// - `config`: Configuration handle
/// - `timeout_ms`: Timeout in milliseconds
///
/// # Returns
/// - Success: LINKRS_OK
/// - Failure: Error code
///
/// # Safety
/// - `config` must be a valid configuration handle
#[no_mangle]
pub unsafe extern "C" fn linkrs_config_set_timeout(
    config: *mut linkrs_config_t,
    timeout_ms: c_int,
) -> c_int {
    if config.is_null() || timeout_ms < 0 {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    let handle = &mut *(config as *mut LinkrsConfigHandle);
    handle.inner = handle
        .inner
        .clone()
        .with_timeout(Duration::from_millis(timeout_ms as u64));
    linkrs_error_code_t::LINKRS_OK as c_int
}

/// Set read-only mode
///
/// # Arguments
/// - `config`: Configuration handle
/// - `read_only`: Read-only flag
///
/// # Returns
/// - Success: LINKRS_OK
/// - Failure: Error code
///
/// # Safety
/// - `config` must be a valid configuration handle
#[no_mangle]
pub unsafe extern "C" fn linkrs_config_set_read_only(
    config: *mut linkrs_config_t,
    read_only: c_int,
) -> c_int {
    if config.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    let handle = &mut *(config as *mut LinkrsConfigHandle);
    handle.inner = handle.inner.clone().with_read_only(read_only != 0);
    linkrs_error_code_t::LINKRS_OK as c_int
}

/// Set create-if-missing flag
///
/// # Arguments
/// - `config`: Configuration handle
/// - `create`: Create-if-missing flag
///
/// # Returns
/// - Success: LINKRS_OK
/// - Failure: Error code
///
/// # Safety
/// - `config` must be a valid configuration handle
#[no_mangle]
pub unsafe extern "C" fn linkrs_config_set_create_if_missing(
    config: *mut linkrs_config_t,
    create: c_int,
) -> c_int {
    if config.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    let handle = &mut *(config as *mut LinkrsConfigHandle);
    handle.inner = handle.inner.clone().with_create_if_missing(create != 0);
    linkrs_error_code_t::LINKRS_OK as c_int
}

/// Set WAL (Write-Ahead Logging) enabled
///
/// # Arguments
/// - `config`: Configuration handle
/// - `enable`: Enable flag
///
/// # Returns
/// - Success: LINKRS_OK
/// - Failure: Error code
///
/// # Safety
/// - `config` must be a valid configuration handle
#[no_mangle]
pub unsafe extern "C" fn linkrs_config_set_enable_wal(
    config: *mut linkrs_config_t,
    enable: c_int,
) -> c_int {
    if config.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    let handle = &mut *(config as *mut LinkrsConfigHandle);
    handle.inner = handle.inner.clone().with_wal(enable != 0);
    linkrs_error_code_t::LINKRS_OK as c_int
}

/// Set the synchronization mode (mirrors `SyncMode`).
///
/// # Arguments
/// - `config`: Configuration handle
/// - `mode`: 0 = Full (every write synced), 1 = Normal (default), 2 = Off
///
/// # Returns
/// - Success: LINKRS_OK
/// - Failure: LINKRS_MISUSE for null handles or out-of-range modes
///
/// # Safety
/// - `config` must be a valid configuration handle
#[no_mangle]
pub unsafe extern "C" fn linkrs_config_set_sync_mode(
    config: *mut linkrs_config_t,
    mode: c_int,
) -> c_int {
    if config.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }
    let sync_mode = match mode {
        0 => SyncMode::Full,
        1 => SyncMode::Normal,
        2 => SyncMode::Off,
        _ => return linkrs_error_code_t::LINKRS_MISUSE as c_int,
    };

    let handle = &mut *(config as *mut LinkrsConfigHandle);
    handle.inner = handle.inner.clone().with_sync_mode(sync_mode);
    linkrs_error_code_t::LINKRS_OK as c_int
}

/// Read one runtime `section.key` configuration value
/// (`Database::get_config_value` over FFI).
///
/// Supported keys: `cache.size_mb`, `timeout.default_ms`, `wal.enabled`,
/// `sync.mode`, `storage.read_only`, `storage.create_if_missing`,
/// `storage.checkpoint_on_close`.
///
/// # Arguments
/// - `db`: Database handle
/// - `section`: Config section name (UTF-8 encoded)
/// - `key`: Config key name (UTF-8 encoded)
///
/// # Returns
/// - Current value as a heap string, NULL for unknown keys or on error
///
/// # Memory Management
/// The returned string is dynamically allocated and must be freed by the
/// caller using `linkrs_free_string` to avoid memory leaks.
///
/// # Safety
/// - `db` must be a valid database handle created by `linkrs_open` or `linkrs_open_with_config`
/// - `section` and `key` must be valid pointers to null-terminated UTF-8 strings
/// - The returned pointer must be freed by the caller to avoid memory leaks
#[no_mangle]
pub unsafe extern "C" fn linkrs_config_get_value(
    db: *mut linkrs_t,
    section: *const c_char,
    key: *const c_char,
) -> *mut c_char {
    if db.is_null() || section.is_null() || key.is_null() {
        return std::ptr::null_mut();
    }

    let section_str = match CStr::from_ptr(section).to_str() {
        Ok(s) => s,
        Err(_) => return std::ptr::null_mut(),
    };
    let key_str = match CStr::from_ptr(key).to_str() {
        Ok(s) => s,
        Err(_) => return std::ptr::null_mut(),
    };

    let handle = &*(db as *mut LinkrsHandle);
    match handle.inner.get_config_value(section_str, key_str) {
        Some(value) => match CString::new(value) {
            Ok(c_value) => c_value.into_raw(),
            Err(_) => std::ptr::null_mut(),
        },
        None => std::ptr::null_mut(),
    }
}

/// Update one runtime `section.key` configuration value from its string form
/// (`Database::update_config_value` over FFI).
///
/// # Arguments
/// - `db`: Database handle
/// - `section`: Config section name (UTF-8 encoded)
/// - `key`: Config key name (UTF-8 encoded)
/// - `value`: New value (UTF-8 encoded)
///
/// # Returns
/// - Success: LINKRS_OK
/// - Failure: Error code (unknown keys and unparsable values fail with
///   details in the thread-local error message)
///
/// # Safety
/// - `db` must be a valid database handle created by `linkrs_open` or `linkrs_open_with_config`
/// - `section`, `key`, and `value` must be valid pointers to null-terminated UTF-8 strings
#[no_mangle]
pub unsafe extern "C" fn linkrs_config_update_value(
    db: *mut linkrs_t,
    section: *const c_char,
    key: *const c_char,
    value: *const c_char,
) -> c_int {
    if db.is_null() || section.is_null() || key.is_null() || value.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    let section_str = match CStr::from_ptr(section).to_str() {
        Ok(s) => s,
        Err(_) => return linkrs_error_code_t::LINKRS_MISUSE as c_int,
    };
    let key_str = match CStr::from_ptr(key).to_str() {
        Ok(s) => s,
        Err(_) => return linkrs_error_code_t::LINKRS_MISUSE as c_int,
    };
    let value_str = match CStr::from_ptr(value).to_str() {
        Ok(s) => s,
        Err(_) => return linkrs_error_code_t::LINKRS_MISUSE as c_int,
    };

    let handle = &*(db as *mut LinkrsHandle);
    match handle
        .inner
        .update_config_value(section_str, key_str, value_str)
    {
        Ok(()) => linkrs_error_code_t::LINKRS_OK as c_int,
        Err(e) => {
            let (error_code, _) = error_code_from_core_error(&e);
            set_last_error_message(format!("{e}"));
            error_code
        }
    }
}

/// Reset one runtime `section.key` configuration value to its compiled
/// default (`Database::reset_config_value` over FFI).
///
/// # Arguments
/// - `db`: Database handle
/// - `section`: Config section name (UTF-8 encoded)
/// - `key`: Config key name (UTF-8 encoded)
///
/// # Returns
/// - Success: LINKRS_OK
/// - Failure: Error code (unknown keys fail with details in the
///   thread-local error message)
///
/// # Safety
/// - `db` must be a valid database handle created by `linkrs_open` or `linkrs_open_with_config`
/// - `section` and `key` must be valid pointers to null-terminated UTF-8 strings
#[no_mangle]
pub unsafe extern "C" fn linkrs_config_reset_value(
    db: *mut linkrs_t,
    section: *const c_char,
    key: *const c_char,
) -> c_int {
    if db.is_null() || section.is_null() || key.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    let section_str = match CStr::from_ptr(section).to_str() {
        Ok(s) => s,
        Err(_) => return linkrs_error_code_t::LINKRS_MISUSE as c_int,
    };
    let key_str = match CStr::from_ptr(key).to_str() {
        Ok(s) => s,
        Err(_) => return linkrs_error_code_t::LINKRS_MISUSE as c_int,
    };

    let handle = &*(db as *mut LinkrsHandle);
    match handle.inner.reset_config_value(section_str, key_str) {
        Ok(()) => linkrs_error_code_t::LINKRS_OK as c_int,
        Err(e) => {
            let (error_code, _) = error_code_from_core_error(&e);
            set_last_error_message(format!("{e}"));
            error_code
        }
    }
}

/// Apply many runtime configuration updates at once
/// (`Database::bulk_update_config` over FFI).
///
/// # Arguments
/// - `db`: Database handle
/// - `updates_json`: JSON object mapping `"section.key"` to the new value
///   string, e.g. `{"timeout.default_ms":"2000","cache.size_mb":"64"}`
///
/// # Returns
/// - Success: LINKRS_OK
/// - Failure: Error code (malformed JSON, unknown keys, and unparsable
///   values fail with details in the thread-local error message)
///
/// # Safety
/// - `db` must be a valid database handle created by `linkrs_open` or `linkrs_open_with_config`
/// - `updates_json` must be a valid pointer to a null-terminated UTF-8 string
#[no_mangle]
pub unsafe extern "C" fn linkrs_config_bulk_update(
    db: *mut linkrs_t,
    updates_json: *const c_char,
) -> c_int {
    if db.is_null() || updates_json.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    let json_str = match CStr::from_ptr(updates_json).to_str() {
        Ok(s) => s,
        Err(_) => return linkrs_error_code_t::LINKRS_MISUSE as c_int,
    };

    let updates: std::collections::HashMap<String, String> = match serde_json::from_str(json_str) {
        Ok(u) => u,
        Err(e) => {
            let err = crate::api_core::error::CoreError::InvalidParameter(format!(
                "invalid updates_json: {e}"
            ));
            let (error_code, _) = error_code_from_core_error(&err);
            set_last_error_message(format!("{err}"));
            return error_code;
        }
    };

    let handle = &*(db as *mut LinkrsHandle);
    match handle.inner.bulk_update_config(updates) {
        Ok(()) => linkrs_error_code_t::LINKRS_OK as c_int,
        Err(e) => {
            let (error_code, _) = error_code_from_core_error(&e);
            set_last_error_message(format!("{e}"));
            error_code
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn test_config_new() {
        unsafe {
            let config = linkrs_config_new();
            assert!(!config.is_null());
            assert_eq!(linkrs_config_free(config), 0);
        }
    }

    #[test]
    fn test_config_file() {
        unsafe {
            let path = CString::new("test.db").unwrap();
            let config = linkrs_config_file(path.as_ptr());
            assert!(!config.is_null());
            assert_eq!(linkrs_config_free(config), 0);
        }
    }

    #[test]
    fn test_config_memory() {
        unsafe {
            let config = linkrs_config_memory();
            assert!(!config.is_null());
            assert_eq!(linkrs_config_free(config), 0);
        }
    }

    #[test]
    fn test_config_set_cache_size() {
        unsafe {
            let config = linkrs_config_memory();
            assert_eq!(linkrs_config_set_cache_size(config, 128), 0);
            assert_eq!(linkrs_config_free(config), 0);
        }
    }

    #[test]
    fn test_config_set_timeout() {
        unsafe {
            let config = linkrs_config_memory();
            assert_eq!(linkrs_config_set_timeout(config, 5000), 0);
            assert_eq!(linkrs_config_free(config), 0);
        }
    }

    #[test]
    fn test_config_sync_mode_values() {
        unsafe {
            let config = linkrs_config_memory();
            assert_eq!(linkrs_config_set_sync_mode(config, 0), 0);
            assert_eq!(linkrs_config_set_sync_mode(config, 1), 0);
            assert_eq!(linkrs_config_set_sync_mode(config, 2), 0);
            assert_eq!(
                linkrs_config_set_sync_mode(config, 7),
                linkrs_error_code_t::LINKRS_MISUSE as c_int
            );
            assert_eq!(
                linkrs_config_set_sync_mode(std::ptr::null_mut(), 1),
                linkrs_error_code_t::LINKRS_MISUSE as c_int
            );
            assert_eq!(linkrs_config_free(config), 0);
        }
    }

    #[test]
    fn test_open_with_config_memory() {
        use crate::embedded::c_api::database::{linkrs_close, linkrs_open_with_config};
        use crate::embedded::c_api::types::linkrs_t;

        unsafe {
            let config = linkrs_config_memory();
            assert!(!config.is_null());
            let mut db: *mut linkrs_t = std::ptr::null_mut();
            assert_eq!(linkrs_open_with_config(config, &mut db), 0);
            assert!(!db.is_null());
            assert_eq!(linkrs_close(db), 0);
            assert_eq!(linkrs_config_free(config), 0);
            let mut db: *mut linkrs_t = std::ptr::null_mut();
            assert_eq!(
                linkrs_open_with_config(std::ptr::null_mut(), &mut db),
                linkrs_error_code_t::LINKRS_MISUSE as c_int
            );
        }
    }

    fn open_memory_db() -> *mut linkrs_t {
        use crate::embedded::c_api::database::linkrs_open_with_config;
        use crate::embedded::c_api::types::linkrs_t;

        unsafe {
            let config = linkrs_config_memory();
            let mut db: *mut linkrs_t = std::ptr::null_mut();
            assert_eq!(linkrs_open_with_config(config, &mut db), 0);
            assert!(!db.is_null());
            assert_eq!(linkrs_config_free(config), 0);
            db
        }
    }

    #[test]
    fn test_runtime_config_get_update() {
        use crate::embedded::c_api::database::{linkrs_close, linkrs_free_string};
        use crate::embedded::c_api::types::linkrs_t;

        unsafe {
            let db = open_memory_db();

            let section = CString::new("cache").unwrap();
            let key = CString::new("size_mb").unwrap();
            let out = linkrs_config_get_value(db, section.as_ptr(), key.as_ptr());
            assert!(!out.is_null());
            linkrs_free_string(out);

            let timeout_section = CString::new("timeout").unwrap();
            let timeout_key = CString::new("default_ms").unwrap();
            let value = CString::new("1234").unwrap();
            assert_eq!(
                linkrs_config_update_value(
                    db,
                    timeout_section.as_ptr(),
                    timeout_key.as_ptr(),
                    value.as_ptr()
                ),
                linkrs_error_code_t::LINKRS_OK as c_int
            );
            let out = linkrs_config_get_value(db, timeout_section.as_ptr(), timeout_key.as_ptr());
            assert!(!out.is_null());
            let text = std::ffi::CStr::from_ptr(out).to_str().unwrap().to_string();
            linkrs_free_string(out);
            assert_eq!(text, "1234");

            // Unknown keys read back as NULL.
            let unknown_section = CString::new("nope").unwrap();
            let unknown_key = CString::new("nothing").unwrap();
            let out = linkrs_config_get_value(db, unknown_section.as_ptr(), unknown_key.as_ptr());
            assert!(out.is_null());

            assert_eq!(linkrs_close(db), 0);
        }
    }

    #[test]
    fn test_runtime_config_bulk_and_reset() {
        use crate::embedded::c_api::database::{linkrs_close, linkrs_free_string};
        use crate::embedded::c_api::types::linkrs_t;

        unsafe {
            let db = open_memory_db();

            let updates =
                CString::new(r#"{"timeout.default_ms":"2500","cache.size_mb":"64"}"#).unwrap();
            assert_eq!(
                linkrs_config_bulk_update(db, updates.as_ptr()),
                linkrs_error_code_t::LINKRS_OK as c_int
            );

            let timeout_section = CString::new("timeout").unwrap();
            let timeout_key = CString::new("default_ms").unwrap();
            let out = linkrs_config_get_value(db, timeout_section.as_ptr(), timeout_key.as_ptr());
            let text = std::ffi::CStr::from_ptr(out).to_str().unwrap().to_string();
            linkrs_free_string(out);
            assert_eq!(text, "2500");

            let cache_section = CString::new("cache").unwrap();
            let cache_key = CString::new("size_mb").unwrap();
            let out = linkrs_config_get_value(db, cache_section.as_ptr(), cache_key.as_ptr());
            let text = std::ffi::CStr::from_ptr(out).to_str().unwrap().to_string();
            linkrs_free_string(out);
            assert_eq!(text, "64");

            // Reset restores the compiled default.
            assert_eq!(
                linkrs_config_reset_value(db, timeout_section.as_ptr(), timeout_key.as_ptr()),
                linkrs_error_code_t::LINKRS_OK as c_int
            );
            let out = linkrs_config_get_value(db, timeout_section.as_ptr(), timeout_key.as_ptr());
            let text = std::ffi::CStr::from_ptr(out).to_str().unwrap().to_string();
            linkrs_free_string(out);
            let expected = DatabaseConfig::default()
                .default_timeout
                .as_millis()
                .to_string();
            assert_eq!(text, expected);

            assert_eq!(linkrs_close(db), 0);
        }
    }

    #[test]
    fn test_runtime_config_errors() {
        use crate::embedded::c_api::database::linkrs_close;
        use crate::embedded::c_api::types::linkrs_t;

        unsafe {
            let db = open_memory_db();

            let section = CString::new("cache").unwrap();
            let key = CString::new("size_mb").unwrap();
            let bad_value = CString::new("not_a_number").unwrap();
            assert_ne!(
                linkrs_config_update_value(db, section.as_ptr(), key.as_ptr(), bad_value.as_ptr()),
                linkrs_error_code_t::LINKRS_OK as c_int
            );

            let bad_json = CString::new("{not json").unwrap();
            assert_ne!(
                linkrs_config_bulk_update(db, bad_json.as_ptr()),
                linkrs_error_code_t::LINKRS_OK as c_int
            );

            let unknown_section = CString::new("nope").unwrap();
            let unknown_key = CString::new("nothing").unwrap();
            assert_ne!(
                linkrs_config_reset_value(db, unknown_section.as_ptr(), unknown_key.as_ptr()),
                linkrs_error_code_t::LINKRS_OK as c_int
            );

            assert_eq!(
                linkrs_config_update_value(
                    std::ptr::null_mut(),
                    section.as_ptr(),
                    key.as_ptr(),
                    bad_value.as_ptr()
                ),
                linkrs_error_code_t::LINKRS_MISUSE as c_int
            );
            assert_eq!(
                linkrs_config_reset_value(std::ptr::null_mut(), section.as_ptr(), key.as_ptr()),
                linkrs_error_code_t::LINKRS_MISUSE as c_int
            );
            assert_eq!(
                linkrs_config_bulk_update(std::ptr::null_mut(), bad_json.as_ptr()),
                linkrs_error_code_t::LINKRS_MISUSE as c_int
            );

            assert_eq!(linkrs_close(db), 0);
        }
    }
}
